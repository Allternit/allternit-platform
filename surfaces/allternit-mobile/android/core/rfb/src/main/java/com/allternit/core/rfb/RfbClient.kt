package com.allternit.core.rfb

import android.graphics.Bitmap
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiConfig
import com.allternit.core.auth.AllternitAuth
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import java.util.concurrent.TimeUnit
import kotlin.coroutines.cancellation.CancellationException

/*
 * Native RFB (VNC) client + the UI-facing session (iOS RFBClient.swift).
 *
 * `GET /ws/bots/:id/desktop/vnc?…` (and `/ws/computers/:id/vnc?…token=`) on the allternit-api origin
 * blindly proxies RFB 3.8 bytes over BINARY WebSocket frames. The upgrade carries the Bearer token,
 * which is why this is a native client and not a web view.
 *
 * VIEW-ONLY ENFORCEMENT: the server forwards input regardless of control state, so pointer and key
 * events only leave the phone while [RfbSession.allowsInput] is true.
 */
internal class RfbClient(
    private val onFrame: (Bitmap) -> Unit,
    private val onServerInit: (String, Int, Int) -> Unit,
) {
    private val parser = RfbStreamParser()
    private val incoming = Channel<ByteArray>(Channel.UNLIMITED)
    private var socket: WebSocket? = null
    private var framebuffer: RfbFramebuffer? = null
    @Volatile var intentionallyClosed = false
        private set

    suspend fun connect(wsPath: String, credentials: RfbCredentials?) {
        val url = webSocketUrl(wsPath) ?: throw RfbException.InvalidUrl(wsPath)
        val request = Request.Builder().url(url).apply {
            AllternitAuth.tokenProvider.currentToken()?.takeIf { it.isNotEmpty() }?.let { header("Authorization", "Bearer $it") }
        }.build()
        socket = okHttp.newWebSocket(request, object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                incoming.trySend(bytes.toByteArray())
            }
            // The proxy only sends binary frames; a text frame is still RFB bytes on the wire.
            override fun onMessage(webSocket: WebSocket, text: String) {
                incoming.trySend(text.toByteArray(Charsets.UTF_8))
            }
            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                incoming.close()
            }
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                incoming.close(t)
            }
        })
        performHandshake(credentials)
    }

    /** FramebufferUpdate → apply → publish one Bitmap → ask for the next incremental update. */
    suspend fun run() {
        while (true) {
            when (val message = nextMessage()) {
                is RfbServerMessage.FramebufferUpdate -> {
                    val fb = framebuffer ?: throw RfbException.UnexpectedMessage("FramebufferUpdate before ServerInit")
                    message.rects.forEach { fb.apply(it) }
                    onFrame(fb.makeBitmap())
                    send(RfbClientMessage.framebufferUpdateRequest(true, fb.width, fb.height))
                }
                RfbServerMessage.SetColourMapEntries, RfbServerMessage.Bell, RfbServerMessage.ServerCutText -> Unit
                else -> throw RfbException.UnexpectedMessage("handshake message after ServerInit")
            }
        }
    }

    fun sendPointer(x: Int, y: Int, buttonMask: Int) = send(RfbClientMessage.pointerEvent(buttonMask, x, y))

    fun sendKey(keysym: Long, down: Boolean) = send(RfbClientMessage.keyEvent(keysym, down))

    /** User-initiated close: no reconnect. */
    fun disconnect() {
        intentionallyClosed = true
        socket?.close(1000, null)
        socket = null
        incoming.close()
        framebuffer?.release()
    }

    private suspend fun performHandshake(credentials: RfbCredentials?) {
        val version = nextMessage()
        if (version !is RfbServerMessage.ProtocolVersion) throw RfbException.UnexpectedMessage("expected ProtocolVersion, got $version")
        validateVersion(version.line)
        send(RfbClientMessage.protocolVersion())

        when (val security = nextMessage()) {
            is RfbServerMessage.SecurityFailure -> throw RfbException.SecurityHandshakeFailed(security.reason)
            is RfbServerMessage.SecurityTypes -> {
                val type = RfbAuth.supportedTypes.firstOrNull { it in security.types }
                    ?: throw RfbException.NoSupportedSecurityType(security.types)
                authenticate(type, credentials)
            }
            else -> throw RfbException.UnexpectedMessage("expected Security, got $security")
        }
        send(RfbClientMessage.clientInit())

        val init = nextMessage()
        if (init !is RfbServerMessage.ServerInit) throw RfbException.UnexpectedMessage("expected ServerInit, got $init")
        framebuffer = RfbFramebuffer(init.info.width, init.info.height)
        onServerInit(init.info.name, init.info.width, init.info.height)
        send(RfbClientMessage.setPixelFormat())
        send(RfbClientMessage.setEncodings())
        send(RfbClientMessage.framebufferUpdateRequest(false, init.info.width, init.info.height))
    }

    /**
     * Answers the chosen type, then reads SecurityResult. Without credentials for a type that needs
     * them this throws [RfbException.CredentialsRequired] before selecting it, so no failed attempt counts.
     */
    private suspend fun authenticate(type: Int, credentials: RfbCredentials?) {
        when (type) {
            30 -> {
                if (credentials == null || credentials.username.isEmpty()) throw RfbException.CredentialsRequired(true)
                send(RfbClientMessage.securitySelection(30))
                val header = nextBytes(4)
                val generator = header.copyOfRange(0, 2)
                val keyLength = ((header[2].toInt() and 0xFF) shl 8) or (header[3].toInt() and 0xFF)
                val prime = nextBytes(keyLength)
                val serverKey = nextBytes(keyLength)
                val response = RfbAuth.ardResponse(generator, prime, serverKey, credentials)
                    ?: throw RfbException.SecurityHandshakeFailed("the computer sent an unusable key")
                send(response)
            }
            2 -> {
                if (credentials == null || credentials.password.isEmpty()) throw RfbException.CredentialsRequired(false)
                send(RfbClientMessage.securitySelection(2))
                val challenge = nextBytes(16)
                val response = RfbAuth.vncResponse(challenge, credentials.password)
                    ?: throw RfbException.SecurityHandshakeFailed("couldn't answer the password challenge")
                send(response)
            }
            else -> send(RfbClientMessage.securitySelection(1))
        }
        val result = nextBytes(4)
        if (result.all { it.toInt() == 0 }) return
        // 3.8 carries a reason string on failure (some servers close instead).
        var reason = if (type == 1) "The computer refused the connection." else "Wrong user name or password."
        runCatching {
            val lengthBytes = nextBytes(4)
            val length = lengthBytes.fold(0) { acc, b -> (acc shl 8) or (b.toInt() and 0xFF) }
            if (length in 1 until 4096) String(nextBytes(length), Charsets.UTF_8).takeIf { it.isNotEmpty() }?.let { reason = it }
        }
        if (type == 1) throw RfbException.SecurityHandshakeFailed(reason)
        throw RfbException.AuthenticationFailed(reason)
    }

    /** "RFB 003.008\n": answer 3.8, refuse anything older. */
    private fun validateVersion(line: String) {
        val parts = line.drop(4).dropLast(1).split(".")
        val major = parts.getOrNull(0)?.toIntOrNull()
        val minor = parts.getOrNull(1)?.toIntOrNull()
        if (parts.size != 2 || major == null || minor == null) throw RfbException.InvalidProtocolVersion(line)
        if (major != 3 || minor < 8) throw RfbException.UnsupportedProtocolVersion(line)
    }

    private suspend fun nextMessage(): RfbServerMessage {
        while (true) {
            parser.nextMessage()?.let { return it }
            parser.append(receive())
        }
    }

    private suspend fun nextBytes(count: Int): ByteArray {
        while (true) {
            parser.takeBytes(count)?.let { return it }
            parser.append(receive())
        }
    }

    private suspend fun receive(): ByteArray {
        val r = incoming.receiveCatching()
        return r.getOrNull() ?: throw (r.exceptionOrNull() ?: RfbException.NotConnected())
    }

    private fun send(data: ByteArray) {
        val s = socket ?: throw RfbException.NotConnected()
        if (!s.send(data.toByteString())) throw RfbException.NotConnected()
    }

    companion object {
        private val okHttp: OkHttpClient by lazy {
            OkHttpClient.Builder().connectTimeout(15, TimeUnit.SECONDS).readTimeout(0, TimeUnit.MILLISECONDS)
                .pingInterval(25, TimeUnit.SECONDS).build()
        }

        /**
         * The path-only `ws_url` (`/ws/bots/...?sandbox_id=…&user_id=…`) against the API origin,
         * http→ws / https→wss, with the query kept verbatim (iOS RFBClient.webSocketURL).
         */
        fun webSocketUrl(wsPath: String, config: ApiConfig = ApiConfig()): String? {
            if (wsPath.isBlank()) return null
            if (wsPath.startsWith("ws://") || wsPath.startsWith("wss://")) return wsPath
            if (wsPath.startsWith("http://")) return "ws://" + wsPath.removePrefix("http://")
            if (wsPath.startsWith("https://")) return "wss://" + wsPath.removePrefix("https://")
            val base = config.apiBaseUrl
            val scheme = if (base.scheme == "https") "wss" else "ws"
            val defaultPort = if (base.scheme == "https") 443 else 80
            val port = if (base.port != defaultPort) ":${base.port}" else ""
            return "$scheme://${base.host}$port/${wsPath.trimStart('/')}"
        }
    }
}

/**
 * Main-thread owner of one [RfbClient] (iOS BotDesktopSession): Compose state for the view,
 * reconnect with capped exponential backoff, the sign-in form, and the VIEW-ONLY input gate.
 */
class RfbSession(private val wsPath: String, allowsInput: Boolean = false) {
    sealed interface State {
        data object Connecting : State
        data object Live : State
        data class Reconnecting(val attempt: Int) : State
        data class Failed(val message: String) : State
        data class NeedsCredentials(val needsUsername: Boolean, val message: String?) : State
    }

    var state: State by mutableStateOf(State.Connecting)
        private set
    /** Latest framebuffer snapshot; null until the first FramebufferUpdate. */
    var frame: Bitmap? by mutableStateOf(null)
        private set
    var desktopName: String by mutableStateOf("")
        private set
    var framebufferWidth: Int by mutableStateOf(0)
        private set
    var framebufferHeight: Int by mutableStateOf(0)
        private set
    /** THE input gate: pointer and keys leave the phone only while this is true. */
    var allowsInput: Boolean by mutableStateOf(allowsInput)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var client: RfbClient? = null
    private var connectJob: Job? = null
    private var reconnectJob: Job? = null
    private var reconnectAttempt = 0
    private var intentionalClose = false
    private var credentials: RfbCredentials? = null

    /** Connects (idempotent while a client exists). */
    fun start() {
        if (client != null) return
        intentionalClose = false
        reconnectAttempt = 0
        connect()
    }

    /** Tears down without reconnecting (the screen closed). */
    fun stop() {
        intentionalClose = true
        reconnectJob?.cancel()
        reconnectJob = null
        connectJob?.cancel()
        connectJob = null
        client?.disconnect()
        client = null
    }

    /** Manual retry from the failed overlay. */
    fun retry() {
        stop()
        intentionalClose = false
        reconnectAttempt = 0
        state = State.Connecting
        connect()
    }

    fun submitCredentials(username: String, password: String) {
        credentials = RfbCredentials(username, password)
        retry()
    }

    fun dispose() {
        stop()
        scope.cancel()
    }

    fun sendPointer(x: Int, y: Int, buttonMask: Int) {
        val c = client ?: return
        if (!allowsInput || state != State.Live) return
        val cx = x.coerceIn(0, 0xFFFF)
        val cy = y.coerceIn(0, 0xFFFF)
        runCatching { c.sendPointer(cx, cy, buttonMask) }.onFailure { handleFailure(it) }
    }

    fun sendKey(keysym: Long, down: Boolean) {
        val c = client ?: return
        if (!allowsInput || state != State.Live) return
        runCatching { c.sendKey(keysym, down) }.onFailure { handleFailure(it) }
    }

    /** A full press (down + up) of one button. */
    fun tap(x: Int, y: Int, buttonMask: Int) {
        sendPointer(x, y, buttonMask)
        sendPointer(x, y, 0)
    }

    private fun connect() {
        state = if (reconnectAttempt > 0) State.Reconnecting(reconnectAttempt) else State.Connecting
        val c = RfbClient(
            onFrame = { bitmap -> scope.launch { frame = bitmap } },
            onServerInit = { name, w, h ->
                scope.launch {
                    desktopName = name
                    framebufferWidth = w
                    framebufferHeight = h
                }
            },
        )
        client = c
        val creds = credentials
        connectJob = scope.launch {
            try {
                withContext(Dispatchers.IO) { c.connect(wsPath, creds) }
                if (intentionalClose || client !== c) return@launch
                reconnectAttempt = 0
                state = State.Live
                withContext(Dispatchers.Default) { c.run() }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Throwable) {
                if (!c.intentionallyClosed && client === c) handleFailure(e)
            }
        }
    }

    /** Reconnect with backoff (1 s → 30 s, [MAX_RECONNECT_ATTEMPTS] tries), or ask for a sign-in. */
    private fun handleFailure(error: Throwable) {
        if (intentionalClose) return
        connectJob?.cancel()
        connectJob = null
        client?.disconnect()
        client = null
        when (error) {
            is RfbException.CredentialsRequired -> {
                state = State.NeedsCredentials(error.needsUsername, null)
                return
            }
            is RfbException.AuthenticationFailed -> {
                val needsUsername = !credentials?.username.isNullOrEmpty()
                credentials = null
                state = State.NeedsCredentials(needsUsername, error.message)
                return
            }
            else -> Unit
        }
        if (reconnectAttempt >= MAX_RECONNECT_ATTEMPTS) {
            state = State.Failed(error.message ?: "The desktop stream ended.")
            return
        }
        reconnectAttempt += 1
        state = State.Reconnecting(reconnectAttempt)
        val delayMs = minOf(30_000L, 1000L shl (reconnectAttempt - 1))
        reconnectJob = scope.launch {
            delay(delayMs)
            if (!intentionalClose) connect()
        }
    }

    companion object {
        const val MAX_RECONNECT_ATTEMPTS = 8
    }
}
