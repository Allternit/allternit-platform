package com.allternit.phonelink.core

import org.json.JSONException
import org.json.JSONObject
import java.util.Base64

/**
 * HTTP routes of the iCTRL protocol: GET /whoami, /schema, /skill, POST /pair,
 * POST /rpc. Port of the relevant parts of iCTRLKit/JSONRPCServer.swift.
 */
class PhoneServer(
    private val auth: AuthManager,
    private val dispatcher: RpcDispatcher,
    private val config: () -> ServerConfig,
    private val clock: () -> Long = System::currentTimeMillis,
    private val log: (String) -> Unit = {},
    /** The shell server's raw streams for POST /stream; null where the phone has none. */
    private val streams: StreamOpener? = null,
) {
    companion object {
        const val DEVICE_HEADER = "X-ICTRL-Device"
    }

    fun handle(req: HttpRequest): HttpResponse {
        val r = route(req)
        // Every response names the device (clients keep one key per device id).
        return HttpResponse(r.status, r.contentType, r.body, r.headers + baseHeaders(cors = r.headers["X-No-CORS"] == null) - "X-No-CORS", r.upgrade)
    }

    private fun baseHeaders(cors: Boolean): Map<String, String> {
        val h = linkedMapOf(DEVICE_HEADER to config().deviceId)
        if (cors) {
            h["Access-Control-Allow-Origin"] = "*"
            h["Access-Control-Allow-Methods"] = "GET, POST, OPTIONS"
            h["Access-Control-Allow-Headers"] = "Content-Type, Authorization"
        }
        return h
    }

    private fun route(req: HttpRequest): HttpResponse = when {
        req.method == "GET" && req.path == "/" -> {
            val accept = req.headers["accept"].orEmpty()
            if (accept.contains("application/json")) HttpResponse.json(200, Schema.document().toString(2))
            else HttpResponse.text(200, Schema.SKILL_MD, "text/markdown; charset=utf-8")
        }
        req.method == "GET" && req.path == "/schema" -> HttpResponse.json(200, Schema.document().toString(2))
        req.method == "GET" && req.path == "/whoami" -> {
            val c = config()
            val body = JSONObject()
                .put("device_id", c.deviceId)
                .put("name", c.deviceName)
                .put("platform", Schema.PLATFORM)
                .put("protocol_version", Schema.PROTOCOL_VERSION)
                .put("server_version", Schema.SERVER_VERSION)
                .put("mesh_state", c.meshState)
                .put("mesh_host", c.meshHost ?: JSONObject.NULL)
                .put("screen_state", c.screenState)
            // Same-origin only, like iCTRL: a web page must not read the device id.
            HttpResponse.json(200, body.toString(), mapOf("X-No-CORS" to "1"))
        }
        req.method == "GET" && (req.path == "/skill" || req.path == "/skill.md" || req.path == "/SKILL.md") ->
            HttpResponse.text(200, Schema.SKILL_MD, "text/markdown; charset=utf-8")
        req.method == "POST" && req.path == "/pair" -> handlePair(req)
        req.method == "POST" && req.path == "/rpc" -> handleRpc(req)
        req.method == "POST" && req.path == "/stream" -> handleStream(req)
        req.method == "OPTIONS" -> HttpResponse.text(200, "")
        else -> HttpResponse.text(404, "Not Found")
    }

    private fun errorBody(code: String, message: String, retryAfter: Int? = null): String {
        val e = JSONObject().put("code", code).put("message", message).put("server_ts", clock() / 1000)
        if (retryAfter != null) e.put("retry_after", retryAfter)
        return JSONObject().put("error", e).toString()
    }

    private fun pairError(status: Int, code: String, message: String, retryAfter: Int? = null): HttpResponse =
        HttpResponse.json(status, errorBody(code, message, retryAfter),
            if (retryAfter != null) mapOf("Retry-After" to retryAfter.toString()) else emptyMap())

    private fun peerOf(req: HttpRequest): Peer? = req.remote?.let { Peer(it.hostAddress ?: "?", if (req.isLoopback) "local" else "lan") }

    private fun handlePair(req: HttpRequest): HttpResponse {
        val obj = try { req.body?.let { JSONObject(String(it, Charsets.UTF_8)) } } catch (e: JSONException) { null }
        val clientName = obj?.opt("client_name") as? String
        if (obj == null || clientName.isNullOrEmpty()) return pairError(400, "bad_request", "client_name is required")
        val plain = obj.optBoolOrNull("plain") ?: false
        var clientPub: ByteArray? = null
        if (!plain) {
            val b64 = obj.opt("pub") as? String
            val pub = try { b64?.let { Base64.getDecoder().decode(it) } } catch (e: IllegalArgumentException) { null }
            if (pub == null || pub.size != 32) {
                return pairError(400, "bad_request", "Provide plain:true (loopback only) or pub (base64 raw 32B X25519)")
            }
            clientPub = pub
        }
        // Mesh-forwarded connections arrive from 127.0.0.1 but are remote peers:
        // no plaintext pairing, and (in requestAuth below) no armed grant.
        if (plain && (!req.isLoopback || req.viaMesh)) {
            return pairError(403, "plain_forbidden", "Plain grant is localhost/USB only; use X25519 mode")
        }
        val fingerprint = clientPub?.let { SecureEnvelope.sha256(it).copyOfRange(0, 4).toHex() }
        log("pair request from ${req.remote?.hostAddress} name=$clientName fp=$fingerprint")

        val session = when (val outcome = auth.requestAuth(clientName, req.isLoopback && !req.viaMesh, fingerprint)) {
            is PairOutcome.Approved -> outcome.session.also { auth.recordPeer(it.id, peerOf(req)) }
            PairOutcome.Denied -> return pairError(403, "pair_denied", "User rejected or approval timed out")
            is PairOutcome.Busy -> return pairError(429, "pair_busy", "Another pairing request is waiting for approval on the device", outcome.retryAfter)
            is PairOutcome.CoolingDown -> return pairError(429, "pair_cooldown", "A pairing request was just refused; try again in ${outcome.retryAfter}s", outcome.retryAfter)
        }
        val key = session.key ?: return pairError(500, "internal", "Session has no key")
        val response = JSONObject()
            .put("tok", session.id)
            .put("client_name", session.clientName)
            .put("expires", (session.lastActiveAtMs + AuthManager.INACTIVITY_MS) / 1000)
        if (clientPub != null) {
            try {
                val devicePriv = X25519.generatePrivateKey()
                val devicePub = X25519.publicKey(devicePriv)
                val shared = X25519.sharedSecret(devicePriv, clientPub)
                val pad = SecureEnvelope.pairingPad(shared, clientPub, devicePub)
                response.put("pub", Base64.getEncoder().encodeToString(devicePub))
                response.put("token_ct", SecureEnvelope.xor(key, pad).toHex())
            } catch (e: IllegalArgumentException) {
                auth.revokeSession(session.id)
                return pairError(400, "bad_request", "X25519 key agreement failed")
            }
        } else {
            response.put("token", key.toHex())
        }
        return HttpResponse.json(200, response.toString(2))
    }

    private fun authError(status: Int, code: String, message: String) = HttpResponse.json(status, errorBody(code, message))

    /**
     * POST /stream: a sealed request (same v1 envelope as /rpc) for one raw
     * scrcpy socket, answered with 101 and then the scrcpy bytes both ways.
     * Plaintext is {"kind":"video","scid":"<8 hex>","args":["key=value",…]}
     * or {"kind":"control","scid":"<8 hex>"}. Mesh (WireGuard-encrypted) and
     * loopback only: the screen and input never cross a plain LAN socket.
     */
    private fun handleStream(req: HttpRequest): HttpResponse {
        if (!req.viaMesh && !req.isLoopback) {
            return authError(403, "stream_forbidden", "Live screen streams are available over the Allternit mesh only")
        }
        val opener = streams ?: return authError(404, "stream_unavailable", "This phone has no screen service")
        val obj = try { req.body?.let { JSONObject(String(it, Charsets.UTF_8)) } } catch (e: JSONException) { null }
        if (obj == null || !SecureEnvelope.looksLikeEnvelope(obj)) return authError(401, "auth_required", "Encrypted envelope required")
        val opened = try {
            auth.openEnvelope(obj, peerOf(req))
        } catch (f: SecureEnvelope.Failure) {
            return authError(if (f.code == "bad_envelope") 400 else 401, f.code, f.message ?: "")
        } catch (e: Exception) {
            return authError(401, "auth_failed", AuthManager.AUTH_FAILED_MESSAGE)
        }
        val ask = try { JSONObject(opened.plaintext) } catch (e: JSONException) { null }
            ?: return authError(400, "bad_request", "Stream request must be JSON")
        val scid = ask.optString("scid")
        val command = when (ask.optString("kind")) {
            "video" -> {
                val args = ask.optJSONArray("args")
                val list = (0 until (args?.length() ?: 0)).map { args!!.optString(it) }
                "SCRCPY $scid ${list.joinToString(" ")}".trimEnd()
            }
            "control" -> "ATTACH $scid"
            else -> return authError(400, "bad_request", "kind must be video or control")
        }
        // Validate before touching the shell server: same rules it applies.
        ShellServerProtocol.parseCommand(command).onFailure { e -> return authError(400, "bad_request", e.message ?: "bad stream request") }
        val raw = try {
            opener.open(command)
        } catch (e: Exception) {
            log("stream ${ask.optString("kind")} refused: ${e.message}")
            return authError(503, "stream_unavailable", e.message ?: "The screen service isn't running")
        }
        log("stream ${ask.optString("kind")} scid=$scid opened (${if (req.viaMesh) "mesh" else "local"})")
        return HttpResponse(101, "application/octet-stream", ByteArray(0),
            mapOf("Upgrade" to "allternit-stream", "Connection" to "Upgrade", "X-No-CORS" to "1")) { client ->
            Pipe.join(client.getInputStream(), client.getOutputStream(), raw.getInputStream(), raw.getOutputStream()) {
                try { raw.close() } catch (_: Exception) {}
                try { client.close() } catch (_: Exception) {}
            }
        }
    }

    private fun handleRpc(req: HttpRequest): HttpResponse {
        val obj = try { req.body?.let { JSONObject(String(it, Charsets.UTF_8)) } } catch (e: JSONException) { null }
            ?: return authError(400, "bad_envelope", "Body must be a v1 encrypted envelope")
        if (!SecureEnvelope.looksLikeEnvelope(obj)) {
            return authError(401, "auth_required", "Encrypted envelope required. POST /pair to obtain a key, then wrap each JSON-RPC body per the v1 protocol (see /skill).")
        }
        val opened = try {
            auth.openEnvelope(obj, peerOf(req))
        } catch (f: SecureEnvelope.Failure) {
            return authError(if (f.code == "bad_envelope") 400 else 401, f.code, f.message ?: "")
        } catch (e: Exception) {
            return authError(401, "auth_failed", AuthManager.AUTH_FAILED_MESSAGE)
        }
        val reply = dispatcher.handleBody(opened.plaintext, if (req.isLoopback && !req.viaMesh) "local" else "lan")
        // Always 200: the real status travels inside the sealed body.
        return HttpResponse.json(200, opened.seal(reply))
    }
}
