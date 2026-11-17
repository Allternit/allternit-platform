package com.allternit.phonelink.core

import org.json.JSONObject
import java.net.URLEncoder
import java.security.MessageDigest
import java.util.Base64
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

// The Android end of the app-phone lane (the iPhone app's AppPhoneService /
// AppPhoneRelay / IngestSocket in one JVM-testable class): redeem the owner's
// claim code, pair with the runtime (pair socket: X25519 + 6-digit code the
// owner approves), keep the control socket (state frames out, input.events,
// hid.* and screen_request in) and open the ingest socket the screen packets go out on.
// No Android classes: everything the platform provides sits behind the small
// interfaces below. Every socket message is a SecureEnvelope (same
// construction both ways), exactly like iOS.

/** What pairing leaves behind. [sessionKeyHex] is the 32-byte HKDF output. */
data class AppPhoneCredentials(
    val phoneId: String,
    val name: String,
    val runtimeDeviceId: String,
    val appPhoneKey: String,
    val sessionKeyHex: String,
    val cloudBaseUrl: String,
) {
    val serial: String get() = "app:$phoneId"
    val sessionKey: ByteArray? get() = Hex.decode(sessionKeyHex)?.takeIf { it.size == 32 }
}

/** Keystore-backed storage in the app (step 4); in-memory in tests. */
interface CredentialStore {
    fun load(): AppPhoneCredentials?
    fun save(credentials: AppPhoneCredentials)
    fun clear()
}

data class HttpReply(val status: Int, val body: String)

/** Blocking JSON POST (called off the main thread). Throws IOException when the network fails. */
interface JsonHttp {
    fun postJson(url: String, headers: Map<String, String>, body: String): HttpReply
}

interface SocketLink {
    fun sendText(text: String): Boolean
    fun sendBinary(bytes: ByteArray): Boolean
    fun close(code: Int, reason: String)
}

interface SocketListener {
    fun onOpen()
    fun onText(text: String)
    fun onBinary(bytes: ByteArray)
    /** Closed or failed; the socket is dead either way. */
    fun onClosed()
}

/** Dials a ws(s) URL. The real one must also send a ping every 20 s (the relay drops a silent socket). */
interface SocketConnector {
    fun connect(url: String, listener: SocketListener): SocketLink
}

interface Cancellable { fun cancel() }

interface LinkRuntime {
    fun nowMs(): Long
    /** Any worker thread: ticket and redeem calls block here. */
    fun background(task: () -> Unit)
    /** ONE ordered thread: control requests run here so input events keep their order. */
    fun serial(task: () -> Unit)
    fun schedule(delayMs: Long, task: () -> Unit): Cancellable
}

/** Screen-share consent as the app reports it in its state frame. */
enum class Consent(val wire: String) { GRANTED("granted"), NEEDED("needed"), DENIED("denied"), UNKNOWN("unknown") }

/** What the app (step 4) provides to the link. */
interface ScreenHost {
    /**
     * A viewer is waiting and no ingest is open (`screen_request`, at most
     * one per 10 s) or a bot asked (`screen.prompt`): ask the person to
     * start sharing (notification or the share card). Never start capture here:
     * only the person's tap on the system consent dialog can.
     */
    fun onScreenRequest()
    /** Accessibility service connected (taps possible). */
    fun canTap(): Boolean
    fun consent(): Consent
    fun foreground(): Boolean
}

enum class PairFailure { DENIED, TIMED_OUT, CLOSED, KEY_MISMATCH, NETWORK, REFUSED }

interface LinkListener {
    /** 6-digit code to show beside the owner's approval prompt. */
    fun onPairCode(code: String) {}
    fun onPaired(credentials: AppPhoneCredentials) {}
    fun onPairFailed(reason: PairFailure, message: String) {}
    fun onControlConnected(connected: Boolean) {}
    /** Revoked (401/404 on a ticket): credentials were cleared; pair again. */
    fun onUnpaired() {}
}

class RelayFailure(val status: Int, message: String) : Exception(message) {
    /** 401/404: key revoked or phone removed; retrying cannot help. (403 is a runtime refusal.) */
    val isFatal: Boolean get() = status == 401 || status == 404
}

/** What redeeming a claim code returns, before the pair handshake. */
data class Registration(val phoneId: String, val runtimeDeviceId: String, val appPhoneKey: String, val name: String)

class AppPhoneLink(
    private val cloudBaseUrl: String,
    private val http: JsonHttp,
    private val sockets: SocketConnector,
    private val store: CredentialStore,
    private val runtime: LinkRuntime,
    private val host: ScreenHost,
    private val input: InputEvents,
    private val deviceModel: String,
    private val listener: LinkListener = object : LinkListener {},
    /** Phone Use `bt-android`: the `hid.*` methods (this phone as the iPhone's Bluetooth keyboard and mouse); null = not offered. */
    private val hid: HidCommands? = null,
    private val newPrivateKey: () -> ByteArray = { X25519.generatePrivateKey() },
) {
    private val lock = Any()
    @Volatile private var credentials: AppPhoneCredentials? = store.load()

    val isPaired: Boolean get() = credentials != null
    fun credentials(): AppPhoneCredentials? = credentials

    // ── Redeem ──────────────────────────────────────────────────────────────

    /** Blocking. Trades the owner's one-time code for the app phone key. */
    fun redeem(code: String, name: String?): Registration {
        val clean = code.filterNot { it == ' ' || it == '-' }.uppercase()
        val body = JSONObject().put("code", clean)
        if (!name.isNullOrBlank()) body.put("name", name)
        val reply = post("/api/v1/app-phones/claim/redeem", emptyMap(), body.toString())
        if (reply.status !in 200..299) {
            throw RelayFailure(reply.status, when (reply.status) {
                401 -> "That code is wrong or has expired. Ask for a new one."
                429 -> "Too many tries. Wait a few minutes."
                else -> message(reply.body) ?: "Could not redeem the code (${reply.status})"
            })
        }
        val o = JSONObject(reply.body)
        return Registration(
            phoneId = o.getString("phoneId").lowercase(),
            runtimeDeviceId = o.getString("runtimeDeviceId"),
            appPhoneKey = o.getString("appPhoneKey"),
            name = name?.takeIf { it.isNotBlank() } ?: deviceModel,
        )
    }

    // ── Ticket / socket URL ─────────────────────────────────────────────────

    /** The ws(s) URL for one socket: mint a ticket with the app phone key, then the runtime tunnel. */
    internal fun socketUrl(endpoint: String, phoneId: String, runtimeDeviceId: String, appPhoneKey: String): String {
        val id = enc(phoneId)
        val path = "/api/v1/phones/app/$endpoint?phone=$id"
        val reply = post(
            "/api/v1/app-phones/$id/socket-ticket",
            mapOf("x-allternit-app-phone-key" to appPhoneKey),
            JSONObject().put("path", path).toString(),
        )
        if (reply.status !in 200..299) {
            throw RelayFailure(reply.status, message(reply.body) ?: "Ticket request failed (${reply.status})")
        }
        val o = JSONObject(reply.body)
        o.optString("url").takeIf { it.isNotEmpty() }?.let { return it }
        val ticket = o.optString("ticket").takeIf { it.isNotEmpty() } ?: throw RelayFailure(0, "No ticket in the reply")
        val base = cloudBaseUrl.trimEnd('/')
        val ws = when {
            base.startsWith("https://") -> "wss://" + base.removePrefix("https://")
            base.startsWith("http://") -> "ws://" + base.removePrefix("http://")
            else -> throw RelayFailure(0, "Bad cloud URL")
        }
        return "$ws/api/v1/runtime-devices/${enc(runtimeDeviceId)}/socket?ticket=${enc(ticket)}"
    }

    private fun post(path: String, headers: Map<String, String>, body: String): HttpReply =
        http.postJson(cloudBaseUrl.trimEnd('/') + path, headers + mapOf("Content-Type" to "application/json", "Accept" to "application/json"), body)

    private fun message(body: String): String? = try {
        val o = JSONObject(body)
        o.optString("message").ifEmpty { o.optString("error") }.takeIf { it.isNotEmpty() }
    } catch (e: Exception) { null }

    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    // ── Pairing ─────────────────────────────────────────────────────────────

    private var pairConn: PairConn? = null
    private var pairTimeout: Cancellable? = null

    /** Redeem [code], then run the pair handshake. Progress and result go to the listener. */
    fun pair(code: String, name: String?) {
        cancelPairing()
        runtime.background {
            val reg = try {
                redeem(code, name)
            } catch (e: RelayFailure) {
                listener.onPairFailed(PairFailure.REFUSED, e.message ?: "Refused"); return@background
            } catch (e: Exception) {
                listener.onPairFailed(PairFailure.NETWORK, "Could not reach Allternit. Check the connection."); return@background
            }
            runPairing(reg)
        }
    }

    fun cancelPairing() {
        val old: PairConn?
        synchronized(lock) {
            old = pairConn; pairConn = null
            pairTimeout?.cancel(); pairTimeout = null
        }
        old?.let { it.dead = true; it.link?.close(1001, "cancelled") }
    }

    private fun runPairing(reg: Registration) {
        val url = try {
            socketUrl("pair", reg.phoneId, reg.runtimeDeviceId, reg.appPhoneKey)
        } catch (e: RelayFailure) {
            listener.onPairFailed(PairFailure.REFUSED, e.message ?: "Refused"); return
        } catch (e: Exception) {
            listener.onPairFailed(PairFailure.NETWORK, "Could not reach Allternit. Check the connection."); return
        }
        val priv = newPrivateKey()
        val conn = PairConn(reg, priv, X25519.publicKey(priv))
        synchronized(lock) {
            pairConn = conn
            pairTimeout = runtime.schedule(APPROVAL_TIMEOUT_MS) { failPair(conn, PairFailure.TIMED_OUT, "Nobody approved in time. Try again.") }
        }
        conn.link = sockets.connect(url, conn)
    }

    private fun failPair(conn: PairConn, reason: PairFailure, message: String) {
        synchronized(lock) {
            if (pairConn !== conn || conn.dead) return
            conn.dead = true; pairConn = null
            pairTimeout?.cancel(); pairTimeout = null
        }
        conn.link?.close(1000, "done")
        listener.onPairFailed(reason, message)
    }

    private inner class PairConn(val reg: Registration, val priv: ByteArray, val pub: ByteArray) : SocketListener {
        @Volatile var link: SocketLink? = null
        @Volatile var dead = false
        private var nodePub: ByteArray? = null

        override fun onOpen() {
            val hello = JSONObject()
                .put("type", "hello").put("phoneId", reg.phoneId).put("name", reg.name)
                .put("model", deviceModel).put("pub", Base64.getEncoder().encodeToString(pub))
                .put("platform", "android")
            link?.sendText(hello.toString())
        }

        override fun onBinary(bytes: ByteArray) = onText(String(bytes, Charsets.UTF_8))

        override fun onText(text: String) {
            if (dead) return
            val o = try { JSONObject(text) } catch (e: Exception) { return }
            (o.opt("pub") as? String)?.let { b64 ->
                val bytes = try { Base64.getDecoder().decode(b64) } catch (e: IllegalArgumentException) { null }
                if (bytes != null && bytes.size == 32) {
                    val earlier = nodePub
                    if (earlier != null && !earlier.contentEquals(bytes)) {
                        failPair(this, PairFailure.KEY_MISMATCH, "The key changed during pairing. Try again."); return
                    }
                    if (earlier == null) { nodePub = bytes; listener.onPairCode(pairCode(pub, bytes)) }
                }
            }
            when (o.opt("type")) {
                "denied" -> failPair(this, PairFailure.DENIED, "The owner declined this phone.")
                "approved" -> {
                    val np = nodePub ?: return failPair(this, PairFailure.CLOSED, "Pairing ended early. Try again.")
                    val key = try { sessionKey(priv, np, reg.phoneId) } catch (e: IllegalArgumentException) {
                        return failPair(this, PairFailure.KEY_MISMATCH, "The key exchange failed. Try again.")
                    }
                    val creds = AppPhoneCredentials(reg.phoneId, reg.name, reg.runtimeDeviceId, reg.appPhoneKey, key.toHex(), cloudBaseUrl.trimEnd('/'))
                    synchronized(lock) {
                        if (pairConn !== this || dead) return
                        dead = true; pairConn = null
                        pairTimeout?.cancel(); pairTimeout = null
                    }
                    store.save(creds)
                    credentials = creds
                    link?.close(1000, "paired")
                    listener.onPaired(creds)
                }
            }
        }

        override fun onClosed() = failPair(this, PairFailure.CLOSED, "The connection closed before the owner approved.")
    }

    // ── Unpair ──────────────────────────────────────────────────────────────

    /** Forget the pairing on this phone (the owner revokes the key in Allternit). */
    fun unpair() {
        stopControl()
        cancelPairing()
        store.clear()
        credentials = null
    }

    // ── Control socket ──────────────────────────────────────────────────────

    private var controlWanted = false
    private var control: ControlConn? = null
    private var retry: Cancellable? = null
    private var retryDelayMs = 1_000L
    private val seenNonces = HashMap<String, Long>()

    /** Keep the control socket open (retrying with backoff) while the link service runs. */
    fun startControl() {
        synchronized(lock) {
            if (controlWanted) return
            controlWanted = true
            retryDelayMs = 1_000L
        }
        connectControl()
    }

    fun stopControl() {
        val old: ControlConn?
        synchronized(lock) {
            controlWanted = false
            retry?.cancel(); retry = null
            old = control; control = null
        }
        old?.let { it.dead = true; it.link?.close(1001, "stop"); if (it.open) listener.onControlConnected(false) }
    }

    /** Accessibility, consent or foreground changed: tell the node. */
    fun stateChanged() {
        val c = synchronized(lock) { control }
        if (c != null && c.open) sendSealed(c, stateFrame())
    }

    private fun stateFrame() = JSONObject()
        .put("type", "state").put("foreground", host.foreground()).put("browserOpen", false)
        .put("canTap", host.canTap()).put("consent", host.consent().wire)

    private fun connectControl() {
        runtime.background {
            val creds = credentials
            if (creds == null || !synchronized(lock) { controlWanted && control == null }) return@background
            val url = try {
                socketUrl("control", creds.phoneId, creds.runtimeDeviceId, creds.appPhoneKey)
            } catch (e: RelayFailure) {
                if (e.isFatal) revoked() else scheduleRetry(); return@background
            } catch (e: Exception) {
                scheduleRetry(); return@background
            }
            val key = creds.sessionKey ?: run { revoked(); return@background }
            val conn = ControlConn(key)
            synchronized(lock) {
                if (!controlWanted || control != null) return@background
                control = conn
            }
            conn.link = sockets.connect(url, conn)
        }
    }

    private fun scheduleRetry() {
        synchronized(lock) {
            if (!controlWanted) return
            val d = retryDelayMs
            retryDelayMs = minOf(d * 2, 30_000L)
            retry?.cancel()
            retry = runtime.schedule(d) { connectControl() }
        }
    }

    private fun revoked() {
        stopControl()
        store.clear()
        credentials = null
        listener.onUnpaired()
    }

    private inner class ControlConn(val key: ByteArray) : SocketListener {
        @Volatile var link: SocketLink? = null
        @Volatile var dead = false
        @Volatile var open = false

        override fun onOpen() {
            if (dead) return
            open = true
            synchronized(lock) { retryDelayMs = 1_000L }
            sendSealed(this, stateFrame())
            listener.onControlConnected(true)
        }

        override fun onBinary(bytes: ByteArray) = onText(String(bytes, Charsets.UTF_8))

        override fun onText(text: String) {
            if (dead) return
            // Control messages are sealed only: a plain frame could come from anyone on the relay path.
            val msg = openSealed(text, key) ?: return
            if (msg.opt("type") == "screen_request") { host.onScreenRequest(); return }
            val id = msg.opt("id") ?: return
            val method = msg.opt("method") as? String ?: return
            val params = msg.optJSONObject("params") ?: JSONObject()
            runtime.serial { reply(this, id, method, params) }
        }

        override fun onClosed() {
            val was = open
            open = false
            // Nobody can send the matching key-up or button-up now (contract §9.2).
            hid?.releaseAll()
            val mine = synchronized(lock) { if (control === this) { control = null; true } else false }
            if (!mine || dead) return
            dead = true
            if (was) listener.onControlConnected(false)
            scheduleRetry()
        }
    }

    private fun reply(conn: ControlConn, id: Any, method: String, params: JSONObject) {
        val out = JSONObject().put("id", id)
        try {
            when (method) {
                "input.events" -> {
                    val r = input.handle(params)
                    out.put("result", JSONObject().put("handled", r.handled).put("ignored", r.ignored))
                }
                "screen.prompt" -> {
                    host.onScreenRequest()
                    out.put("result", JSONObject().put("shown", true).put("live", host.consent() == Consent.GRANTED))
                }
                in HidCommands.METHODS -> {
                    val h = hid ?: throw RpcException(RpcException.METHOD_NOT_FOUND, "Unknown method $method", "unknown_method")
                    out.put("result", h.handle(method, params))
                }
                else -> out.put("error", JSONObject().put("code", "unknown_method").put("message", "Unknown method $method"))
            }
        } catch (e: RpcException) {
            out.put("error", JSONObject().put("code", e.type ?: "internal").put("message", e.message ?: ""))
        } catch (e: Exception) {
            out.put("error", JSONObject().put("code", "internal").put("message", e.message ?: "error"))
        }
        if (!conn.dead) sendSealed(conn, out)
    }

    // ── Ingest socket ───────────────────────────────────────────────────────

    interface IngestListener {
        fun onReady() {}
        fun onClosed() {}
        /** `resume`, `pause` or `keyframe` from the node. */
        fun onControl(type: String) {}
        /** Revoked or removed: stop the broadcast, retrying cannot help. */
        fun onFatal(message: String) {}
    }

    /** One broadcast's ingest socket: sealed packets out, retrying with backoff until [IngestChannel.finish]. */
    fun openIngest(listener: IngestListener): IngestChannel = IngestChannel(listener).also { it.connect() }

    inner class IngestChannel internal constructor(private val l: IngestListener) {
        @Volatile var isReady = false; private set
        private var finished = false
        private var conn: IngestConn? = null
        private var delayMs = 1_000L
        private var retryTask: Cancellable? = null

        internal fun connect() {
            runtime.background {
                val creds = credentials
                val key = creds?.sessionKey
                if (creds == null || key == null) { l.onFatal("This phone is no longer paired."); return@background }
                if (synchronized(lock) { finished || conn != null }) return@background
                val url = try {
                    socketUrl("ingest", creds.phoneId, creds.runtimeDeviceId, creds.appPhoneKey)
                } catch (e: RelayFailure) {
                    if (e.isFatal) { revoked(); l.onFatal("This phone is no longer paired. Pair it again."); return@background }
                    later(); return@background
                } catch (e: Exception) {
                    later(); return@background
                }
                val c = IngestConn(key)
                synchronized(lock) {
                    if (finished || conn != null) return@background
                    conn = c
                }
                c.link = sockets.connect(url, c)
            }
        }

        private fun later() {
            synchronized(lock) {
                if (finished) return
                val d = delayMs
                delayMs = minOf(d * 2, 15_000L)
                retryTask = runtime.schedule(d) { connect() }
            }
        }

        /** One packet (BroadcastPacket bytes), sealed into one binary frame. False when not ready. */
        fun send(packet: ByteArray): Boolean {
            val c = synchronized(lock) { conn }
            if (c == null || !isReady) return false
            val sealed = seal(packet, c.key) ?: return false
            return c.link?.sendBinary(sealed) ?: false
        }

        /** Last packet (ENDED), then close and stop retrying. */
        fun finish(last: ByteArray?) {
            val c: IngestConn?
            synchronized(lock) {
                finished = true
                retryTask?.cancel(); retryTask = null
                c = conn; conn = null
            }
            if (c == null) return
            if (isReady && last != null) seal(last, c.key)?.let { c.link?.sendBinary(it) }
            isReady = false
            c.dead = true
            c.link?.close(1000, "done")
        }

        private inner class IngestConn(val key: ByteArray) : SocketListener {
            @Volatile var link: SocketLink? = null
            @Volatile var dead = false

            override fun onOpen() {
                if (dead) return
                isReady = true
                synchronized(lock) { delayMs = 1_000L }
                l.onReady()
            }

            override fun onBinary(bytes: ByteArray) = onText(String(bytes, Charsets.UTF_8))

            override fun onText(text: String) {
                if (dead) return
                val o = try { JSONObject(text) } catch (e: Exception) { return }
                // {"type":…}, plain or sealed (the node may send either).
                val msg = if (SecureEnvelope.looksLikeEnvelope(o)) openSealed(text, key) ?: return else o
                (msg.opt("type") as? String)?.let { l.onControl(it) }
            }

            override fun onClosed() {
                val mine = synchronized(lock) { if (conn === this) { conn = null; true } else false }
                if (!mine || dead) return
                dead = true
                val was = isReady
                isReady = false
                if (was) l.onClosed()
                later()
            }
        }
    }

    // ── Sealing ─────────────────────────────────────────────────────────────

    /** Seals raw bytes (a video packet is not UTF-8) into the v1 envelope; same construction as [SecureEnvelope.sealRequest]. */
    private fun seal(plain: ByteArray, key: ByteArray): ByteArray? = try {
        val nonce = SecureEnvelope.randomBytes(16)
        val ts = runtime.nowMs() / 1000
        val tok4 = SecureEnvelope.sha256(key).copyOfRange(0, 4)
        val kEnc = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_ENC_REQ.toByteArray(Charsets.UTF_8), nonce)
        val kMac = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_MAC.toByteArray(Charsets.UTF_8), nonce)
        val ct = SecureEnvelope.keystreamXor(kEnc, nonce, plain)
        val tag = SecureEnvelope.hmac(kMac, "v1".toByteArray(Charsets.UTF_8), tok4, SecureEnvelope.be64(ts), nonce, ct)
        ("{\"v\":1,\"tok\":\"${tok4.toHex()}\",\"ts\":$ts,\"nonce\":\"${nonce.toHex()}\"," +
            "\"ct\":\"${Base64.getEncoder().encodeToString(ct)}\",\"tag\":\"${tag.toHex()}\"}").toByteArray(Charsets.UTF_8)
    } catch (e: Exception) { null }

    private fun sendSealed(conn: ControlConn, obj: JSONObject) {
        val sealed = SecureEnvelope.sealRequest(conn.key, obj.toString(), runtime.nowMs() / 1000, SecureEnvelope.randomBytes(16))
        conn.link?.sendText(sealed.toString())
    }

    /** Verifies key id, ±120 s window, replay and MAC; returns the opened JSON object. */
    private fun openSealed(text: String, key: ByteArray): JSONObject? {
        val env = try { JSONObject(text) } catch (e: Exception) { return null }
        if (!SecureEnvelope.looksLikeEnvelope(env)) return null
        val p = try { SecureEnvelope.parse(env) } catch (e: SecureEnvelope.Failure) { return null }
        if (p.tokId != SecureEnvelope.tokenId(key)) return null
        val nowS = runtime.nowMs() / 1000
        if (kotlin.math.abs(nowS - p.ts) > SecureEnvelope.TIMESTAMP_WINDOW_S) return null
        synchronized(lock) {
            seenNonces.entries.removeAll { nowS - it.value > SecureEnvelope.TIMESTAMP_WINDOW_S * 2 }
            if (seenNonces.containsKey(p.nonceHex)) return null
        }
        val plain = try { SecureEnvelope.open(p, key) } catch (e: SecureEnvelope.Failure) { return null }
        synchronized(lock) { seenNonces[p.nonceHex] = nowS }
        return try { JSONObject(plain) } catch (e: Exception) { null }
    }

    companion object {
        const val APPROVAL_TIMEOUT_MS = 120_000L
        private const val PAIR_INFO = "allternit-app-phone v1"

        /** HKDF-SHA256(X25519(appPriv, nodePub), salt = phoneId, info = "allternit-app-phone v1"), 32 bytes. */
        fun sessionKey(privateKey: ByteArray, nodePublic: ByteArray, phoneId: String): ByteArray {
            val shared = X25519.sharedSecret(privateKey, nodePublic)
            val prk = hmac(phoneId.toByteArray(Charsets.UTF_8), shared)
            return hmac(prk, PAIR_INFO.toByteArray(Charsets.UTF_8) + byteArrayOf(1)) // T(1) is the whole 32 bytes
        }

        /** SHA-256(appPub || nodePub), first 4 bytes big-endian, mod 1 000 000, zero-padded to 6. */
        fun pairCode(appPublic: ByteArray, nodePublic: ByteArray): String {
            val d = MessageDigest.getInstance("SHA-256").digest(appPublic + nodePublic)
            val n = (d[0].toLong() and 0xff shl 24) or (d[1].toLong() and 0xff shl 16) or (d[2].toLong() and 0xff shl 8) or (d[3].toLong() and 0xff)
            return "%06d".format(n % 1_000_000)
        }

        private fun hmac(key: ByteArray, data: ByteArray): ByteArray =
            Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(key, "HmacSHA256")) }.doFinal(data)
    }
}
