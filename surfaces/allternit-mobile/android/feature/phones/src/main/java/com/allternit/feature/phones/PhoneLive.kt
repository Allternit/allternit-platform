package com.allternit.feature.phones

import android.media.MediaCodec
import android.media.MediaFormat
import android.view.Surface
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import com.allternit.feature.chat.RuntimeSocketOpen
import com.allternit.feature.computer.CloudPhone
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

/*
 * The live phone view (iOS PhoneLiveView + PhoneLiveSession + AnnexBDisplayLayer). The same stream API:
 * `POST /api/v1/phones/:serial/ws-token` mints a token, then `/ws/phones/:serial/stream?token=` on the user's
 * computer (through the relay socket) sends packets: [kind:1][length:4 BE][payload]. 0 status, 1 config
 * (`{width,height,rotation}`), 2 one H.264 Annex-B access unit (SPS+PPS in-band on every IDR), 3 ended.
 * Frames go to a MediaCodec decoder on a SurfaceView. Taps and typing go back as `input` messages and only
 * leave the phone after "Allow taps" (a read-only token until then).
 */

internal object PhoneLiveCopy {
    fun waiting(name: String) = "Tap Start on $name to share its screen."
    fun tapsOff(name: String) = "Turn on taps in the Allternit app on $name."
    const val SHARE_STOPPED = "Sharing stopped on the phone."
    const val RELAY_OFFLINE = "The computer that relays your phone went offline. Open Allternit Desktop or start your cloud computer, then Retry."
    const val SIGNED_OUT = "Sign in to Allternit to see your phones."
    fun ended(reason: String, name: String) = when (reason) {
        "token_expired" -> "$name didn’t start sharing. Tap Start on $name, then Retry."
        "desktop_offline" -> RELAY_OFFLINE
        else -> SHARE_STOPPED
    }
}

/** H.264 Annex-B access units to a Surface. */
internal class AnnexBDecoder {
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var size: Pair<Int, Int>? = null

    @Synchronized fun surface(s: Surface?) { surface = s; rebuild() }
    @Synchronized fun config(w: Int, h: Int) { size = w to h; rebuild() }

    private fun rebuild() {
        release()
        val s = surface ?: return
        val (w, h) = size ?: return
        try {
            val c = MediaCodec.createDecoderByType("video/avc")
            c.configure(MediaFormat.createVideoFormat("video/avc", w, h), s, null, 0)
            c.start()
            codec = c
        } catch (e: Exception) {
            codec = null
        }
    }

    @Synchronized fun feed(data: ByteArray) {
        val c = codec ?: return
        try {
            val i = c.dequeueInputBuffer(5_000)
            if (i >= 0) {
                val b = c.getInputBuffer(i) ?: return
                b.clear(); b.put(data)
                c.queueInputBuffer(i, 0, data.size, System.nanoTime() / 1000, 0)
            }
            val info = MediaCodec.BufferInfo()
            while (true) {
                val o = c.dequeueOutputBuffer(info, 0)
                if (o >= 0) c.releaseOutputBuffer(o, true) else break
            }
        } catch (e: Exception) {
            // A dropped access unit; the next IDR recovers.
        }
    }

    @Synchronized fun release() {
        runCatching { codec?.stop() }
        runCatching { codec?.release() }
        codec = null
    }
}

internal sealed interface LiveState {
    data object Connecting : LiveState
    data object Waiting : LiveState
    data object Live : LiveState
    data class Ended(val reason: String) : LiveState
    data class Error(val text: String) : LiveState
    data object RelayOffline : LiveState
    data object SignedOut : LiveState
}

internal class PhoneLiveSession(private val phone: CloudPhone, private val backend: ChatBackend, private val scope: CoroutineScope) {
    val decoder = AnnexBDecoder()
    var state by mutableStateOf<LiveState>(LiveState.Connecting); private set
    var videoW by mutableStateOf(0); private set
    var videoH by mutableStateOf(0); private set
    var hasControl by mutableStateOf(false); private set
    var controlBusy by mutableStateOf(false); private set
    var notice by mutableStateOf<String?>(null)

    private var socket: okhttp3.WebSocket? = null
    private var generation = 0
    private var ping: Job? = null
    private var closed = false
    private var waitRenewals = 0
    val canTap: Boolean get() = phone.canTap == true
    val name: String get() = phone.name.ifEmpty { "your phone" }

    fun start() {
        closed = false; waitRenewals = 0; hasControl = false; videoW = 0; videoH = 0; notice = null; state = LiveState.Connecting
        scope.launch { connect(readOnly = true) }
    }

    fun stop() {
        closed = true; generation++
        ping?.cancel(); socket?.close(1001, null); socket = null
        decoder.release()
    }

    private sealed interface Minted {
        data class Token(val value: String) : Minted
        data class Confirmation(val id: String) : Minted
        data class Fail(val state: LiveState) : Minted
    }

    private suspend fun mint(readOnly: Boolean, approvalId: String?): Minted {
        var path = "api/v1/phones/${CloudCall.component(phone.serial)}/ws-token"
        if (approvalId != null) path += "?approval_id=${CloudCall.component(approvalId)}"
        val (status, body) = CloudCall.send("POST", path, buildJsonObject { put("readOnly", readOnly) })
        val err = (body?.get("error") as? kotlinx.serialization.json.JsonPrimitive)?.content
        return when {
            status == 0 -> Minted.Fail(LiveState.Error("You’re offline. Check your connection and try again."))
            status == 403 && err == "confirmation_required" -> (body?.get("approval_id") as? kotlinx.serialization.json.JsonPrimitive)?.content?.let { Minted.Confirmation(it) } ?: Minted.Fail(LiveState.SignedOut)
            status == 401 || status == 403 -> Minted.Fail(LiveState.SignedOut)
            status !in 200..299 -> Minted.Fail(LiveState.Error(listOf("message", "reason", "error").firstNotNullOfOrNull { k -> (body?.get(k) as? kotlinx.serialization.json.JsonPrimitive)?.content } ?: "The phone stream couldn’t start."))
            else -> (body?.get("token") as? kotlinx.serialization.json.JsonPrimitive)?.content?.takeIf { it.isNotEmpty() }?.let { Minted.Token(it) }
                ?: Minted.Fail(LiveState.Error("The phone stream token response was unreadable."))
        }
    }

    private suspend fun connect(readOnly: Boolean) {
        when (val m = mint(readOnly, null)) {
            is Minted.Token -> open(m.value)
            is Minted.Fail -> if (!closed) state = m.state
            is Minted.Confirmation -> if (!closed) state = LiveState.Error("The phone stream couldn’t start.")
        }
    }

    private suspend fun open(token: String): Boolean {
        val my = ++generation
        val path = "/ws/phones/${CloudCall.component(phone.serial)}/stream?token=${CloudCall.component(token)}"
        val opened = backend.openRuntimeSocket(path, onText = { handleText(it, my) }, onBinary = { handlePacket(it, my) }, onEnd = {
            if (my == generation && !closed && state !is LiveState.Ended) state = LiveState.Ended("The phone stream connection closed.")
        })
        when (opened) {
            is RuntimeSocketOpen.Opened -> {
                if (closed) { opened.socket.close(1001, null); return false }
                val old = socket
                socket = opened.socket
                old?.close(1001, null)
                if (videoW == 0) state = LiveState.Waiting
                ping?.cancel()
                ping = scope.launch { while (isActive) { delay(20_000); if (my != generation) return@launch; opened.socket.send("{\"type\":\"ping\"}") } }
                return true
            }
            RuntimeSocketOpen.NoComputer -> state = LiveState.RelayOffline
            is RuntimeSocketOpen.Failed -> state = LiveState.RelayOffline
        }
        return false
    }

    private fun handlePacket(bytes: ByteArray, gen: Int) {
        if (gen != generation || bytes.size < 5) return
        val len = (bytes[1].toInt() and 0xFF shl 24) or (bytes[2].toInt() and 0xFF shl 16) or (bytes[3].toInt() and 0xFF shl 8) or (bytes[4].toInt() and 0xFF)
        if (bytes.size != 5 + len) return
        val payload = bytes.copyOfRange(5, bytes.size)
        fun obj(): JsonObject? = runCatching { Json.parseToJsonElement(String(payload, Charsets.UTF_8)) as? JsonObject }.getOrNull()
        when (bytes[0].toInt()) {
            1 -> {
                val o = obj() ?: return
                val w = (o["width"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toIntOrNull() ?: return
                val h = (o["height"] as? kotlinx.serialization.json.JsonPrimitive)?.content?.toIntOrNull() ?: return
                videoW = w; videoH = h; decoder.config(w, h); state = LiveState.Live
            }
            2 -> if (videoW > 0) decoder.feed(payload)
            3 -> end((obj()?.get("reason") as? kotlinx.serialization.json.JsonPrimitive)?.content.orEmpty())
        }
    }

    private fun handleText(text: String, gen: Int) {
        if (gen != generation) return
        val o = runCatching { Json.parseToJsonElement(text) as? JsonObject }.getOrNull() ?: return
        if ((o["type"] as? kotlinx.serialization.json.JsonPrimitive)?.content != "end") return
        val reason = (o["reason"] as? kotlinx.serialization.json.JsonPrimitive)?.content.orEmpty()
        if (reason == "token_expired" && videoW == 0 && waitRenewals < 3) {
            waitRenewals++; hasControl = false
            scope.launch { connect(readOnly = true) }
            return
        }
        end(reason)
    }

    private fun end(reason: String) {
        if (state is LiveState.Ended) return
        state = LiveState.Ended(reason)
        generation++; ping?.cancel(); socket?.close(1000, null); socket = null
    }

    suspend fun requestControl() {
        if (hasControl || controlBusy) return
        if (!canTap) { notice = PhoneLiveCopy.tapsOff(name); return }
        controlBusy = true; notice = null
        try {
            var m = mint(false, null)
            if (m is Minted.Confirmation) {
                val r = backend.runtimeJson("POST", "/api/aci/handoff/${CloudCall.component(m.id)}/approve", "{}")
                if (r is RuntimeCall.NoComputer || r is RuntimeCall.Failed) { notice = PhoneLiveCopy.RELAY_OFFLINE; return }
                if (r !is RuntimeCall.Ok) { notice = "Taps couldn’t be approved. Try again."; return }
                m = mint(false, m.id)
            }
            val token = (m as? Minted.Token)?.value
            if (token == null) { notice = "Taps couldn’t be turned on."; return }
            if (open(token)) hasControl = true
        } finally {
            controlBusy = false
        }
    }

    fun send(events: List<JsonObject>) {
        val s = socket ?: return
        if (!hasControl || !canTap || events.isEmpty()) return
        s.send(buildJsonObject { put("type", "input"); put("events", JsonArray(events)) }.toString())
    }

    companion object {
        fun touch(action: String, x: Int, y: Int) = buildJsonObject {
            put("type", "touch"); put("action", action); put("pointerId", 0); put("x", x); put("y", y); put("pressure", if (action == "up") 0 else 1)
        }
        fun key(type: String) = buildJsonObject { put("type", type) }
        fun text(t: String) = buildJsonObject { put("type", "text"); put("text", t) }
    }
}

/** The view's point in video pixels when it falls inside the letterboxed picture (iOS PhoneLiveGeometry). */
private fun videoPoint(x: Float, y: Float, vw: Float, vh: Float, w: Float, h: Float, clamp: Boolean): Pair<Int, Int>? {
    if (vw <= 0 || vh <= 0 || w <= 0 || h <= 0) return null
    val scale = minOf(w / vw, h / vh)
    val rw = vw * scale; val rh = vh * scale
    val rx = (w - rw) / 2; val ry = (h - rh) / 2
    if (!clamp && (x < rx || x > rx + rw || y < ry || y > ry + rh)) return null
    val px = ((x - rx) / rw).coerceIn(0f, 1f) * vw
    val py = ((y - ry) / rh).coerceIn(0f, 1f) * vh
    return Math.round(px) to Math.round(py)
}

/** Full-screen live view of one linked phone (iOS PhoneLiveView). */
@Composable
fun PhoneLiveView(phone: CloudPhone, backend: ChatBackend, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val session = remember(phone.serial) { PhoneLiveSession(phone, backend, scope) }
    var typed by remember { mutableStateOf("") }
    LaunchedEffect(session) { session.start() }
    DisposableEffect(session) { onDispose { session.stop() } }
    BackHandler(onBack = onClose)
    val tapsEnabled = session.hasControl && session.canTap

    Column(Modifier.fillMaxSize().background(c.bgPanel).statusBarsPadding().navigationBarsPadding().testTag("phoneLive")) {
        LibraryHeader(title = session.name, onBack = onClose, backTag = "phoneLiveClose")
        if (session.state == LiveState.Live && !session.hasControl) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    if (session.canTap) "Watching only. Allow taps to tap and type on ${session.name}." else "Watching only. ${PhoneLiveCopy.tapsOff(session.name)}",
                    color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f),
                )
                if (session.canTap) Text(
                    if (session.controlBusy) "Allowing…" else "Allow taps", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(CircleShape).background(c.fillNeutral).clickable(enabled = !session.controlBusy, role = Role.Button) { scope.launch { session.requestControl() } }.testTag("phoneLiveAllowTaps").padding(horizontal = 14.dp, vertical = 8.dp),
                )
            }
        }
        session.notice?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp).clip(RoundedCornerShape(12.dp)).background(Color.Black)) {
            val wPx = constraints.maxWidth.toFloat()
            val hPx = constraints.maxHeight.toFloat()
            AndroidView(
                factory = { ctx ->
                    SurfaceView(ctx).apply {
                        holder.addCallback(object : SurfaceHolder.Callback {
                            override fun surfaceCreated(h: SurfaceHolder) = session.decoder.surface(h.surface)
                            override fun surfaceChanged(h: SurfaceHolder, f: Int, w: Int, hh: Int) = Unit
                            override fun surfaceDestroyed(h: SurfaceHolder) = session.decoder.surface(null)
                        })
                    }
                },
                modifier = Modifier.fillMaxSize(),
            )
            if (session.state == LiveState.Live && tapsEnabled) {
                var last = 0L
                Box(
                    Modifier.fillMaxSize().testTag("phoneLiveTouch").pointerInput(session.videoW, session.videoH) {
                        awaitEachGesture {
                            val down = awaitFirstDown()
                            val vw = session.videoW.toFloat(); val vh = session.videoH.toFloat()
                            val p0 = videoPoint(down.position.x, down.position.y, vw, vh, wPx, hPx, false)
                            if (p0 == null) return@awaitEachGesture
                            session.send(listOf(PhoneLiveSession.touch("down", p0.first, p0.second)))
                            var lastPos = down.position
                            while (true) {
                                val ev = awaitPointerEvent()
                                val ch = ev.changes.firstOrNull() ?: break
                                lastPos = ch.position
                                if (!ch.pressed) break
                                val now = System.currentTimeMillis()
                                if (now - last >= 30) {
                                    last = now
                                    videoPoint(ch.position.x, ch.position.y, vw, vh, wPx, hPx, true)?.let { session.send(listOf(PhoneLiveSession.touch("move", it.first, it.second))) }
                                }
                                ch.consume()
                            }
                            videoPoint(lastPos.x, lastPos.y, vw, vh, wPx, hPx, true)?.let { session.send(listOf(PhoneLiveSession.touch("up", it.first, it.second))) }
                        }
                    },
                )
            }
            when (val st = session.state) {
                LiveState.Live -> Unit
                LiveState.Connecting -> LiveMessage("Connecting…", false) { }
                LiveState.Waiting -> LiveMessage(PhoneLiveCopy.waiting(session.name), false) { }
                is LiveState.Ended -> LiveMessage(PhoneLiveCopy.ended(st.reason, session.name), true) { session.start() }
                is LiveState.Error -> LiveMessage(st.text, true) { session.start() }
                LiveState.RelayOffline -> LiveMessage(PhoneLiveCopy.RELAY_OFFLINE, true) { session.start() }
                LiveState.SignedOut -> LiveMessage(PhoneLiveCopy.SIGNED_OUT, false, spinner = false) { }
            }
        }
        if (tapsEnabled) Column(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) { LibraryField("", typed, "Type on ${session.name}", "phoneLiveType") { typed = it } }
                Text("Send", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(CircleShape).background(c.fillNeutral).clickable(enabled = typed.isNotEmpty(), role = Role.Button) { session.send(listOf(PhoneLiveSession.text(typed))); typed = "" }.testTag("phoneLiveSend").padding(horizontal = 16.dp, vertical = 10.dp))
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("Back" to "back", "Home" to "home", "Apps" to "app_switch").forEach { (label, type) ->
                    Text(label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center,
                        modifier = Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).clickable(role = Role.Button) { session.send(listOf(PhoneLiveSession.key(type))) }.testTag("phoneLive$type").padding(vertical = 12.dp))
                }
            }
        }
    }
}

@Composable
private fun LiveMessage(text: String, retry: Boolean, spinner: Boolean = true, onRetry: () -> Unit) {
    Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically), horizontalAlignment = Alignment.CenterHorizontally) {
        if (!retry && spinner) CircularProgressIndicator(color = Color.White, modifier = Modifier.padding(4.dp))
        Text(text, color = Color.White, fontSize = 14.sp, textAlign = TextAlign.Center)
        if (retry) Text("Retry", color = Color.White, fontWeight = FontWeight.SemiBold, fontSize = 14.sp,
            modifier = Modifier.clip(CircleShape).background(Color.White.copy(alpha = 0.18f)).clickable(role = Role.Button, onClick = onRetry).testTag("phoneLiveRetry").padding(horizontal = 18.dp, vertical = 10.dp))
    }
}
