package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.net.HttpURLConnection
import java.net.InetAddress
import java.net.Socket
import java.net.URL
import java.util.Base64

/**
 * End to end over a real socket: the client below does exactly what
 * ictrl_rpc.py / Desktop's ictrl-client.ts do (X25519 pair, unwrap, seal,
 * open the sealed reply).
 */
class PhoneServerTest {
    private lateinit var http: HttpServer
    private lateinit var server: PhoneServer
    private lateinit var auth: AuthManager
    private var armed = true
    private var base = ""

    @Before fun setUp() {
        auth = AuthManager(MemorySessionStore(), consumeArmedGrant = { local -> if (local && armed) { armed = false; true } else false })
        auth.approver = ScriptedApprover(false)
        val hooks = FakeHooks()
        val dispatcher = RpcDispatcher({ FakeDeviceOps() }, { FakeDeviceOps.INFO }, auth, hooks, "com.allternit.phonelink")
        server = PhoneServer(auth, dispatcher, { hooks.config() })
        http = HttpServer(server::handle)
        http.start(InetAddress.getByName("127.0.0.1"), 0)
        base = "http://127.0.0.1:${http.localPort}"
    }

    @After fun tearDown() { http.stop() }

    private class Resp(val status: Int, val body: String, val headers: Map<String, String>)

    private fun request(method: String, path: String, body: String? = null): Resp {
        val c = URL(base + path).openConnection() as HttpURLConnection
        c.requestMethod = method
        c.connectTimeout = 5000
        c.readTimeout = 10000
        if (body != null) {
            c.doOutput = true
            c.setRequestProperty("Content-Type", "application/json")
            c.outputStream.use { it.write(body.toByteArray()) }
        }
        val status = c.responseCode
        val stream = if (status >= 400) c.errorStream else c.inputStream
        val text = stream?.use { String(it.readBytes()) } ?: ""
        val headers = c.headerFields.filterKeys { it != null }.mapValues { it.value.joinToString(",") }
        return Resp(status, text, headers)
    }

    /** ictrl_rpc.py pair(): returns K. */
    private fun pair(name: String = "test-desktop"): ByteArray {
        val priv = X25519.generatePrivateKey()
        val pub = X25519.publicKey(priv)
        val r = request("POST", "/pair", JSONObject().put("client_name", name).put("pub", Base64.getEncoder().encodeToString(pub)).toString())
        assertEquals(r.body, 200, r.status)
        val o = JSONObject(r.body)
        val devicePub = Base64.getDecoder().decode(o.getString("pub"))
        val pad = SecureEnvelope.pairingPad(X25519.sharedSecret(priv, devicePub), pub, devicePub)
        val key = SecureEnvelope.xor(Hex.decode(o.getString("token_ct"))!!, pad)
        assertEquals("unwrapped key matches the device's key id", o.getString("tok"), SecureEnvelope.tokenId(key))
        return key
    }

    private fun rpc(key: ByteArray, method: String, params: JSONObject = JSONObject(), ts: Long = System.currentTimeMillis() / 1000, nonce: ByteArray = SecureEnvelope.randomBytes(16)): Pair<Resp, JSONObject?> {
        val body = JSONObject().put("jsonrpc", "2.0").put("method", method).put("params", params).put("id", 1).toString()
        val r = request("POST", "/rpc", SecureEnvelope.sealRequest(key, body, ts, nonce).toString())
        if (r.status != 200) return r to null
        return r to JSONObject(SecureEnvelope.openResponse(key, nonce, JSONObject(r.body)))
    }

    @Test fun whoamiNamesTheDeviceWithoutCors() {
        val r = request("GET", "/whoami")
        assertEquals(200, r.status)
        val o = JSONObject(r.body)
        assertEquals("dev-1234", o.getString("device_id"))
        assertEquals(1, o.getInt("protocol_version"))
        assertEquals("stopped", o.getString("mesh_state"))
        assertTrue(o.isNull("mesh_host"))
        assertEquals("dev-1234", r.headers[PhoneServer.DEVICE_HEADER])
        assertNull(r.headers["Access-Control-Allow-Origin"])
        assertEquals("*", request("GET", "/schema").headers["Access-Control-Allow-Origin"])
    }

    @Test fun schemaAndSkillAreServed() {
        assertTrue(JSONObject(request("GET", "/schema").body).getJSONArray("methods").length() > 20)
        assertTrue(request("GET", "/skill").body.contains("protocol"))
        assertEquals(404, request("GET", "/nope").status)
    }

    @Test fun armedPairThenSealedCallsWork() {
        val key = pair()
        val (_, reply) = rpc(key, "device_info")
        val result = reply!!.getJSONObject("result")
        assertEquals("pixels", result.getString("coordinate_space"))
        val (_, err) = rpc(key, "nope")
        assertEquals(-32601, err!!.getJSONObject("error").getInt("code"))
    }

    @Test fun secondPairIsNotAutoApproved() {
        pair()
        val pub = X25519.publicKey(X25519.generatePrivateKey())
        val r = request("POST", "/pair", JSONObject().put("client_name", "again").put("pub", Base64.getEncoder().encodeToString(pub)).toString())
        assertEquals(403, r.status)
        assertEquals("pair_denied", JSONObject(r.body).getJSONObject("error").getString("code"))
    }

    /**
     * Security: the mesh bridge forwards tailnet connections to a dedicated
     * internal loopback listener whose requests are marked viaMesh. They must
     * not consume the one-shot armed pairing grant even though their remote
     * address is 127.0.0.1 — that grant is reserved for adb-over-cable.
     */
    @Test fun armedGrantIsNotConsumableThroughTheMesh() {
        val meshHttp = HttpServer(server::handle, viaMesh = true)
        meshHttp.start(InetAddress.getByName("127.0.0.1"), 0)
        try {
            val meshBase = "http://127.0.0.1:${meshHttp.localPort}"
            fun meshRequest(method: String, path: String, body: String? = null): Resp {
                val c = URL(meshBase + path).openConnection() as HttpURLConnection
                c.requestMethod = method
                c.connectTimeout = 5000
                c.readTimeout = 10000
                if (body != null) {
                    c.doOutput = true
                    c.setRequestProperty("Content-Type", "application/json")
                    c.outputStream.use { it.write(body.toByteArray()) }
                }
                val status = c.responseCode
                val text = (if (status >= 400) c.errorStream else c.inputStream)?.use { String(it.readBytes()) } ?: ""
                return Resp(status, text, emptyMap())
            }

            // 1. Plain pairing (key returned in the clear) is USB-only, mesh included.
            val plain = meshRequest("POST", "/pair", JSONObject().put("client_name", "mesh-peer").put("plain", true).toString())
            assertEquals(403, plain.status)
            assertEquals("plain_forbidden", JSONObject(plain.body).getJSONObject("error").getString("code"))

            // 2. The armed grant is NOT consumed by a mesh-forwarded pair: the
            //    scripted approver denies, and pairing fails.
            val pub = X25519.publicKey(X25519.generatePrivateKey())
            val denied = meshRequest("POST", "/pair", JSONObject().put("client_name", "mesh-peer").put("pub", Base64.getEncoder().encodeToString(pub)).toString())
            assertEquals(403, denied.status)
            assertEquals("pair_denied", JSONObject(denied.body).getJSONObject("error").getString("code"))

            // 3. The grant is still armed for the real loopback (adb forward): pairing succeeds.
            val key = pair()
            assertEquals("pixels", rpc(key, "device_info").second!!.getJSONObject("result").getString("coordinate_space"))
        } finally {
            meshHttp.stop()
        }
    }

    @Test fun plainPairWorksOverLoopback() {
        val r = request("POST", "/pair", JSONObject().put("client_name", "usb").put("plain", true).toString())
        assertEquals(200, r.status)
        val key = Hex.decode(JSONObject(r.body).getString("token"))!!
        assertEquals("pixels", rpc(key, "device_info").second!!.getJSONObject("result").getString("coordinate_space"))
    }

    @Test fun pairRequestValidation() {
        assertEquals(400, request("POST", "/pair", "{}").status)
        assertEquals(400, request("POST", "/pair", JSONObject().put("client_name", "x").put("pub", "AAAA").toString()).status)
    }

    @Test fun unsealedAndBrokenRequestsAreRefusedInTheClear() {
        val plain = request("POST", "/rpc", """{"jsonrpc":"2.0","method":"device_info","id":1}""")
        assertEquals(401, plain.status)
        assertEquals("auth_required", JSONObject(plain.body).getJSONObject("error").getString("code"))
        assertEquals(400, request("POST", "/rpc", "garbage").status)
        val bad = request("POST", "/rpc", JSONObject().put("v", 1).put("tok", "zz").put("ct", "").toString())
        assertEquals(400, bad.status)
        assertEquals("bad_envelope", JSONObject(bad.body).getJSONObject("error").getString("code"))
    }

    @Test fun replayAndSkewAreRejectedWithServerTime() {
        val key = pair()
        val nonce = SecureEnvelope.randomBytes(16)
        val ts = System.currentTimeMillis() / 1000
        assertEquals(200, rpc(key, "device_info", ts = ts, nonce = nonce).first.status)
        val replay = rpc(key, "device_info", ts = ts, nonce = nonce).first
        assertEquals(401, replay.status)
        assertEquals("nonce_replayed", JSONObject(replay.body).getJSONObject("error").getString("code"))
        val skew = rpc(key, "device_info", ts = ts - 500).first
        val err = JSONObject(skew.body).getJSONObject("error")
        assertEquals("ts_skew", err.getString("code"))
        assertTrue(Math.abs(err.getLong("server_ts") - ts) < 5)
    }

    @Test fun forgottenKeyGetsAuthFailed() {
        val (r, _) = rpc(ByteArray(32) { 9 }, "device_info")
        assertEquals(401, r.status)
        assertEquals("auth_failed", JSONObject(r.body).getJSONObject("error").getString("code"))
    }

    @Test fun oversizedBodyIsRefused() {
        Socket("127.0.0.1", http.localPort).use { s ->
            s.getOutputStream().write("POST /rpc HTTP/1.1\r\nContent-Length: 99999999\r\n\r\n".toByteArray())
            val line = s.getInputStream().bufferedReader().readLine()
            assertTrue(line, line.startsWith("HTTP/1.1 413"))
        }
    }

    @Test fun stopClosesTheListenerImmediately() {
        http.stop()
        assertFalse(http.isRunning)
        try {
            Socket("127.0.0.1", http.localPort.takeIf { it > 0 } ?: 1).close()
        } catch (_: Exception) { /* refused, as expected */ }
    }

    // ── POST /stream ────────────────────────────────────────────────────────

    /** A server whose StreamOpener connects to an echo socket, so the 101 pipe can be exercised end to end. */
    private fun streamServer(opened: MutableList<String>): Pair<HttpServer, java.net.ServerSocket> {
        val echo = java.net.ServerSocket(0, 4, InetAddress.getByName("127.0.0.1"))
        kotlin.concurrent.thread(isDaemon = true) {
            while (!echo.isClosed) {
                val s = try { echo.accept() } catch (e: java.io.IOException) { break }
                kotlin.concurrent.thread(isDaemon = true) { s.use { it.getInputStream().copyTo(it.getOutputStream()) } }
            }
        }
        val hooks = FakeHooks()
        val dispatcher = RpcDispatcher({ FakeDeviceOps() }, { FakeDeviceOps.INFO }, auth, hooks, "com.allternit.phonelink")
        val streaming = PhoneServer(auth, dispatcher, { hooks.config() }, streams = StreamOpener { cmd ->
            opened.add(cmd)
            Socket("127.0.0.1", echo.localPort)
        })
        val h = HttpServer(streaming::handle, viaMesh = true)
        h.start(InetAddress.getByName("127.0.0.1"), 0)
        return h to echo
    }

    private fun sealedStream(key: ByteArray, plaintext: JSONObject): ByteArray =
        SecureEnvelope.sealRequest(key, plaintext.toString(), System.currentTimeMillis() / 1000, SecureEnvelope.randomBytes(16)).toString().toByteArray()

    private fun readHead(input: java.io.InputStream): String {
        val sb = StringBuilder()
        while (!sb.endsWith("\r\n\r\n")) {
            val b = input.read()
            if (b < 0) break
            sb.append(b.toChar())
        }
        return sb.toString()
    }

    @Test fun streamUpgradesASealedMeshRequestToARawPipe() {
        val key = pair()
        val opened = ArrayList<String>()
        val (h, echo) = streamServer(opened)
        try {
            Socket("127.0.0.1", h.localPort).use { s ->
                val body = sealedStream(key, JSONObject().put("kind", "video").put("scid", "0000abcd")
                    .put("args", org.json.JSONArray().put("audio=false").put("max_size=1080")))
                s.getOutputStream().write("POST /stream HTTP/1.1\r\nHost: x\r\nContent-Length: ${body.size}\r\n\r\n".toByteArray() + body)
                val head = readHead(s.getInputStream())
                assertTrue(head, head.startsWith("HTTP/1.1 101 "))
                s.getOutputStream().write("frames".toByteArray())
                val back = ByteArray(6)
                java.io.DataInputStream(s.getInputStream()).readFully(back)
                assertEquals("frames", String(back))
            }
            assertEquals(listOf("SCRCPY 0000abcd audio=false max_size=1080"), opened)
        } finally {
            h.stop(); echo.close()
        }
    }

    @Test fun streamRefusesPlainLanUnsealedAndUnsafeRequests() {
        val key = pair()
        val opened = ArrayList<String>()
        val (h, echo) = streamServer(opened)
        try {
            val hooks = FakeHooks()
            val dispatcher = RpcDispatcher({ FakeDeviceOps() }, { FakeDeviceOps.INFO }, auth, hooks, "com.allternit.phonelink")
            val direct = PhoneServer(auth, dispatcher, { hooks.config() }, streams = StreamOpener { error("must not open") })
            val ok = sealedStream(key, JSONObject().put("kind", "control").put("scid", "0000abcd"))
            // Plain LAN: never, even sealed.
            val lan = direct.handle(HttpRequest("POST", "/stream", "", emptyMap(), ok, InetAddress.getByName("10.0.0.5")))
            assertEquals(403, lan.status)
            // Mesh but unsealed.
            val unsealed = direct.handle(HttpRequest("POST", "/stream", "", emptyMap(), """{"kind":"control"}""".toByteArray(), InetAddress.getByName("127.0.0.1"), viaMesh = true))
            assertEquals(401, unsealed.status)
            // Sealed but shell syntax in an option: refused before the shell server.
            val evil = sealedStream(key, JSONObject().put("kind", "video").put("scid", "0000abcd").put("args", org.json.JSONArray().put("a=b;reboot")))
            val bad = direct.handle(HttpRequest("POST", "/stream", "", emptyMap(), evil, InetAddress.getByName("127.0.0.1"), viaMesh = true))
            assertEquals(400, bad.status)
            assertTrue(opened.isEmpty())
        } finally {
            h.stop(); echo.close()
        }
    }

    @Test fun whoamiReportsTheScreenState() {
        assertEquals("not_set_up", JSONObject(request("GET", "/whoami").body).getString("screen_state"))
    }
}
