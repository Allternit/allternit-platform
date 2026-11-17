package com.allternit.phonelink.core

import org.json.JSONObject
import kotlin.math.max

/** The screen as capture sees it: real pixels in the current orientation, rotation in degrees (0/90/180/270). */
data class DisplayInfo(val width: Int, val height: Int, val densityDpi: Int, val rotation: Int)

/** Encoded video size: upright, a multiple of [ScreenShareSession.ALIGN], long side capped. */
data class VideoSize(val width: Int, val height: Int)

/** What the encoder must do after a node hint or an ingest change. */
enum class EncoderAction { NONE, RESUME, PAUSE, KEYFRAME }

enum class SharePhase(val wire: String) {
    /** No projection. */
    OFF("off"),
    /** Projection kept, encoder paused: nobody is watching or the ingest socket is down. */
    PAUSED("paused"),
    /** A viewer is watching; frames go out. */
    LIVE("live"),
}

/** `screen_service.projection` and the share card's state. */
data class ProjectionReport(
    val phase: SharePhase,
    val consent: Consent,
    val paired: Boolean,
    val controlConnected: Boolean,
    val canTap: Boolean,
    val video: VideoSize?,
    /** How long the projection has gone unwatched (null while live or off). */
    val idleMs: Long?,
    /** Why the last share ended (stopped, locked, idle, shutdown, revoked, error), or null. */
    val lastEnded: String?,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("state", phase.wire)
        .put("consent", consent.wire)
        .put("paired", paired)
        .put("control", controlConnected)
        .put("canTap", canTap)
        .put("width", video?.width ?: JSONObject.NULL)
        .put("height", video?.height ?: JSONObject.NULL)
        .put("idleMs", idleMs ?: JSONObject.NULL)
        .put("lastEnded", lastEnded ?: JSONObject.NULL)
}

/**
 * The decisions behind one MediaProjection share, without Android classes so
 * they are unit-tested (ProjectionEncoder carries them out):
 *
 *  - The capture is upright: the virtual display is sized to the screen in its
 *    current orientation. A rotation that changes the video size means resize
 *    the display, restart the encoder and send a new CONFIG; a 180° turn keeps
 *    the size and needs nothing.
 *  - Nobody watching (node `pause`, or the ingest socket down) pauses the
 *    encoder but keeps the projection. [IDLE_LIMIT_MS] of that stops the share.
 *  - `resume` starts frames (a keyframe when already live); `keyframe` asks for
 *    one while live.
 *
 * Thread-safe: the encoder thread drives it, RPC threads read [report].
 */
class ScreenShareSession(private val idleLimitMs: Long = IDLE_LIMIT_MS) {
    var phase: SharePhase = SharePhase.OFF; private set
    var video: VideoSize? = null; private set
    var display: DisplayInfo? = null; private set
    private var idleSince: Long? = null
    private var denied = false
    var lastEnded: String? = null; private set

    val isLive: Boolean
        @Synchronized get() = phase == SharePhase.LIVE
    val isOn: Boolean
        @Synchronized get() = phase != SharePhase.OFF

    /** Consent given and the projection exists: starts paused until the node says someone watches. */
    @Synchronized
    fun start(d: DisplayInfo, nowMs: Long): VideoSize {
        val v = fit(d.width, d.height)
        phase = SharePhase.PAUSED
        display = d
        video = v
        idleSince = nowMs
        denied = false
        return v
    }

    /** The person declined Android's screen-share prompt. */
    @Synchronized
    fun consentDenied() { if (phase == SharePhase.OFF) denied = true }

    /** Asking again clears an earlier "no". */
    @Synchronized
    fun consentRequested() { denied = false }

    @Synchronized
    fun consent(): Consent = when {
        phase != SharePhase.OFF -> Consent.GRANTED
        denied -> Consent.DENIED
        else -> Consent.NEEDED
    }

    /** Ingest socket down: nobody can watch. */
    @Synchronized
    fun ingestLost(nowMs: Long): EncoderAction {
        if (phase == SharePhase.OFF) return EncoderAction.NONE
        val wasLive = phase == SharePhase.LIVE
        phase = SharePhase.PAUSED
        if (idleSince == null) idleSince = nowMs
        return if (wasLive) EncoderAction.PAUSE else EncoderAction.NONE
    }

    /** A plain or sealed `{"type":…}` from the node on the ingest socket. */
    @Synchronized
    fun control(type: String, nowMs: Long): EncoderAction {
        if (phase == SharePhase.OFF) return EncoderAction.NONE
        return when (type) {
            "resume" -> {
                val was = phase
                phase = SharePhase.LIVE
                idleSince = null
                if (was == SharePhase.LIVE) EncoderAction.KEYFRAME else EncoderAction.RESUME
            }
            "pause" -> {
                if (phase != SharePhase.LIVE) EncoderAction.NONE
                else { phase = SharePhase.PAUSED; idleSince = nowMs; EncoderAction.PAUSE }
            }
            "keyframe" -> if (phase == SharePhase.LIVE) EncoderAction.KEYFRAME else EncoderAction.NONE
            else -> EncoderAction.NONE
        }
    }

    /**
     * The screen changed (rotation, resolution). Returns the new video size when
     * the encoder must restart with a new CONFIG, null when nothing changes.
     */
    @Synchronized
    fun displayChanged(d: DisplayInfo): VideoSize? {
        if (phase == SharePhase.OFF) return null
        display = d
        val v = fit(d.width, d.height)
        if (v == video) return null
        video = v
        return v
    }

    @Synchronized
    fun shouldStopForIdle(nowMs: Long): Boolean {
        val since = idleSince ?: return false
        return phase != SharePhase.OFF && nowMs - since >= idleLimitMs
    }

    @Synchronized
    fun stop(reason: String) {
        if (phase == SharePhase.OFF) return
        phase = SharePhase.OFF
        video = null
        idleSince = null
        lastEnded = reason
    }

    /** Viewer coordinates (video pixels) to screen pixels; null while not sharing. */
    @Synchronized
    fun geometry(): InputGeometry? {
        val v = video ?: return null
        val d = display ?: return null
        return InputGeometry(v.width, v.height, d.width, d.height)
    }

    @Synchronized
    fun report(nowMs: Long, paired: Boolean, controlConnected: Boolean, canTap: Boolean): ProjectionReport =
        ProjectionReport(
            phase = phase,
            consent = consent(),
            paired = paired,
            controlConnected = controlConnected,
            canTap = canTap,
            video = video,
            idleMs = idleSince?.let { max(0L, nowMs - it) },
            lastEnded = lastEnded,
        )

    companion object {
        /** Unwatched this long, the share stops (the person must tap Share again). */
        const val IDLE_LIMIT_MS = 10 * 60_000L
        /** How often the encoder checks the idle limit and sends the STATUS keepalive. */
        const val TICK_MS = 2_000L
        /** Long side of the encoded video. */
        const val MAX_LONG_SIDE = 1280
        /** H.264 encoders want dimensions in multiples of 8 (some of 16; 8 is what scrcpy uses). */
        const val ALIGN = 8

        /** Scales the screen so the long side is at most [maxLong], keeping orientation; both sides a multiple of [ALIGN]. */
        fun fit(width: Int, height: Int, maxLong: Int = MAX_LONG_SIDE): VideoSize {
            val long = max(width, height).coerceAtLeast(1)
            val scale = if (long > maxLong) maxLong.toDouble() / long else 1.0
            fun align(v: Double) = max(ALIGN, (v.toInt() / ALIGN) * ALIGN)
            return VideoSize(align(width * scale), align(height * scale))
        }
    }
}
