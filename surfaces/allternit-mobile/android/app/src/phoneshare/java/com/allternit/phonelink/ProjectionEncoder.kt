package com.allternit.phonelink

import android.app.KeyguardManager
import android.content.Context
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.Display
import android.view.Surface
import com.allternit.phonelink.core.AppPhoneLink
import com.allternit.phonelink.core.BroadcastPacket
import com.allternit.phonelink.core.EncoderAction
import com.allternit.phonelink.core.ScreenShareSession
import com.allternit.phonelink.core.VideoSize

/**
 * One screen share: MediaProjection → VirtualDisplay → MediaCodec H.264
 * (Annex B, SPS+PPS in front of every IDR) → BroadcastPacket FRAMEs on the
 * ingest socket, CONFIG before frames and after every size change, STATUS
 * every 2 s, ENDED last. [session] makes the decisions (pause/resume,
 * rotation, idle stop); this class carries them out on its own thread.
 *
 * Android 14+ allows one createVirtualDisplay per projection, so rotation
 * resizes the same display and swaps in a new encoder surface. Frames are
 * always upright (the display is sized to the screen's current orientation),
 * so CONFIG rotation is 0. CONFIG scale is 1: viewer coordinates are video
 * pixels (InputEvents maps them to the screen).
 */
class ProjectionEncoder(
    private val context: Context,
    private val projection: MediaProjection,
    private val session: ScreenShareSession,
    private val ingestOpener: (AppPhoneLink.IngestListener) -> AppPhoneLink.IngestChannel,
    /** Live (someone watching) or paused; on the encoder thread. */
    private val onChanged: (Boolean) -> Unit,
    /** The share is over (reason as sent in ENDED); on the encoder thread. */
    private val onEnded: (String) -> Unit,
) {
    companion object {
        private const val TAG = "Projection"
        private const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC
        private const val FRAME_RATE = 30
        private const val KEY_REQUEST_GAP_MS = 1_000L

        /** ~0.1 bit per pixel per frame, 1 to 4 Mbit/s. */
        fun bitRate(size: VideoSize): Int = (size.width.toLong() * size.height * FRAME_RATE / 10).coerceIn(1_000_000L, 4_000_000L).toInt()

        /** True when the access unit's first NAL is an SPS (type 7), i.e. the parameter sets are already in front. */
        fun startsWithSps(au: ByteArray): Boolean {
            val i = when {
                au.size > 4 && au[0].toInt() == 0 && au[1].toInt() == 0 && au[2].toInt() == 0 && au[3].toInt() == 1 -> 4
                au.size > 3 && au[0].toInt() == 0 && au[1].toInt() == 0 && au[2].toInt() == 1 -> 3
                else -> return false
            }
            return (au[i].toInt() and 0x1f) == 7
        }
    }

    private val thread = HandlerThread("projection").apply { start() }
    private val handler = Handler(thread.looper)
    private val displays = context.getSystemService(DisplayManager::class.java)
    private val keyguard = context.getSystemService(KeyguardManager::class.java)

    @Volatile private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var drain: Thread? = null
    @Volatile private var generation = 0
    /** SPS+PPS from the encoder's CODEC_CONFIG buffer. */
    @Volatile private var csd: ByteArray? = null
    /** Drop frames until a keyframe gets through (new viewer, dropped frame, new encoder). */
    @Volatile private var needKey = true
    private var lastKeyRequestAt = 0L
    private var virtualDisplay: VirtualDisplay? = null
    @Volatile private var ingest: AppPhoneLink.IngestChannel? = null
    @Volatile private var ended = false

    private fun now() = System.currentTimeMillis()

    private val callback = object : MediaProjection.Callback() {
        // The status-bar chip's Stop, a revoke, the lock screen (Android 15+), another app's projection.
        override fun onStop() { handler.post { finish(if (keyguard.isKeyguardLocked) "locked" else "stopped", sendEnded = true) } }
    }

    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) {}
        override fun onDisplayRemoved(displayId: Int) {}
        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY && !ended) reconfigure()
        }
    }

    private val ingestListener = object : AppPhoneLink.IngestListener {
        override fun onReady() { handler.post { if (!ended) { sendConfig(); send(BroadcastPacket.status(true)) } } }
        override fun onClosed() { handler.post { if (!ended) apply(session.ingestLost(now())) } }
        override fun onControl(type: String) { handler.post { if (!ended) apply(session.control(type, now())) } }
        override fun onFatal(message: String) { handler.post { finish("revoked", sendEnded = false) } }
    }

    private val tick = object : Runnable {
        override fun run() {
            if (ended) return
            if (session.shouldStopForIdle(now())) { finish("idle", sendEnded = true); return }
            send(BroadcastPacket.status(true))
            handler.postDelayed(this, ScreenShareSession.TICK_MS)
        }
    }

    fun start() {
        handler.post {
            try {
                begin()
            } catch (e: Exception) {
                Log.w(TAG, "share failed to start: ${e.javaClass.simpleName}: ${e.message}")
                finish("error", sendEnded = true)
            }
        }
    }

    /** Ends the share from any thread. */
    fun end(reason: String) {
        if (Looper.myLooper() == thread.looper) finish(reason, sendEnded = true)
        else handler.post { finish(reason, sendEnded = true) }
    }

    private fun begin() {
        // Android 14+: the callback must be registered before the virtual display is created.
        projection.registerCallback(callback, handler)
        val info = ScreenMetrics.displayInfo(context)
        val size = session.start(info, now())
        startCodec(size)
        // No surface yet: the encoder stays paused until the node says someone is watching.
        virtualDisplay = projection.createVirtualDisplay(
            "allternit-share", size.width, size.height, info.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, null, null, handler,
        )
        displays.registerDisplayListener(displayListener, handler)
        ingest = ingestOpener(ingestListener)
        handler.postDelayed(tick, ScreenShareSession.TICK_MS)
        onChanged(false)
    }

    private fun apply(action: EncoderAction) {
        when (action) {
            EncoderAction.RESUME -> {
                virtualDisplay?.surface = surface
                needKey = true
                sendConfig()
                requestKeyframe(force = true)
                onChanged(true)
            }
            EncoderAction.PAUSE -> {
                virtualDisplay?.surface = null
                onChanged(false)
            }
            EncoderAction.KEYFRAME -> {
                needKey = true
                sendConfig()
                requestKeyframe(force = true)
            }
            EncoderAction.NONE -> Unit
        }
    }

    /** Rotation or a resolution change: resize the display, restart the encoder, send a new CONFIG. */
    private fun reconfigure() {
        val info = ScreenMetrics.displayInfo(context)
        val size = session.displayChanged(info) ?: return
        val vd = virtualDisplay ?: return
        val live = session.isLive
        try {
            vd.surface = null
            stopCodec()
            startCodec(size)
            vd.resize(size.width, size.height, info.densityDpi)
            if (live) vd.surface = surface
            needKey = true
            sendConfig()
            if (live) requestKeyframe(force = true)
        } catch (e: Exception) {
            Log.w(TAG, "reconfigure failed: ${e.javaClass.simpleName}: ${e.message}")
            finish("error", sendEnded = true)
        }
    }

    // ── Encoder ─────────────────────────────────────────────────────────────

    private fun startCodec(size: VideoSize) {
        val c = configure(size, baseline = true) ?: configure(size, baseline = false)
            ?: throw IllegalStateException("No H.264 encoder for ${size.width}x${size.height}")
        surface = c.createInputSurface()
        csd = null
        c.start()
        codec = c
        val gen = ++generation
        drain = Thread({ drainLoop(c, gen) }, "projection-out").apply { isDaemon = true; start() }
    }

    private fun configure(size: VideoSize, baseline: Boolean): MediaCodec? {
        val c = try { MediaCodec.createEncoderByType(MIME) } catch (e: Exception) { return null }
        val format = MediaFormat.createVideoFormat(MIME, size.width, size.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, bitRate(size))
            setInteger(MediaFormat.KEY_FRAME_RATE, FRAME_RATE)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
            // A still screen still produces frames, so a keyframe request is answered.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000L)
            if (baseline) setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
        }
        return try {
            c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            c
        } catch (e: Exception) {
            c.release()
            null
        }
    }

    private fun stopCodec() {
        val c = codec ?: return
        codec = null
        generation++
        try { c.signalEndOfInputStream() } catch (_: Exception) {}
        try { drain?.join(500) } catch (_: InterruptedException) {}
        drain = null
        try { c.stop() } catch (_: Exception) {}
        try { c.release() } catch (_: Exception) {}
        surface?.release()
        surface = null
    }

    private fun drainLoop(c: MediaCodec, gen: Int) {
        val info = MediaCodec.BufferInfo()
        try {
            while (gen == generation) {
                val idx = c.dequeueOutputBuffer(info, 100_000)
                if (idx < 0) continue
                val buf = c.getOutputBuffer(idx)
                if (buf != null && info.size > 0) {
                    val bytes = ByteArray(info.size)
                    buf.position(info.offset)
                    buf.limit(info.offset + info.size)
                    buf.get(bytes)
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) csd = bytes
                    else onAccessUnit(bytes, info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0)
                }
                c.releaseOutputBuffer(idx, false)
                if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) break
            }
        } catch (e: IllegalStateException) {
            // Stopped under us (reconfigure or end).
        }
    }

    private fun onAccessUnit(au: ByteArray, key: Boolean) {
        if (!session.isLive) return
        if (needKey && !key) { handler.post { requestKeyframe(force = false) }; return }
        val payload = if (key && !startsWithSps(au)) (csd ?: ByteArray(0)) + au else au
        val sent = ingest?.send(BroadcastPacket.frame(payload)) ?: false
        if (sent) {
            if (key) needKey = false
        } else {
            // Not ready or the uplink is backed up: skip to the next keyframe.
            needKey = true
            handler.post { requestKeyframe(force = false) }
        }
    }

    private fun requestKeyframe(force: Boolean) {
        val t = SystemClock.uptimeMillis()
        if (!force && t - lastKeyRequestAt < KEY_REQUEST_GAP_MS) return
        lastKeyRequestAt = t
        try {
            codec?.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        } catch (_: Exception) {}
    }

    // ── Packets ─────────────────────────────────────────────────────────────

    private fun send(packet: ByteArray) { ingest?.send(packet) }

    private fun sendConfig() {
        val v = session.video ?: return
        send(BroadcastPacket.config(v.width, v.height, 1.0, 0))
    }

    // ── End ─────────────────────────────────────────────────────────────────

    private fun finish(reason: String, sendEnded: Boolean) {
        if (ended) return
        ended = true
        handler.removeCallbacks(tick)
        session.stop(reason)
        ingest?.finish(if (sendEnded) BroadcastPacket.ended(reason) else null)
        ingest = null
        try { displays.unregisterDisplayListener(displayListener) } catch (_: Exception) {}
        try { virtualDisplay?.release() } catch (_: Exception) {}
        virtualDisplay = null
        stopCodec()
        try { projection.unregisterCallback(callback) } catch (_: Exception) {}
        try { projection.stop() } catch (_: Exception) {}
        try { onEnded(reason) } catch (_: Exception) {}
        thread.quitSafely()
    }
}
