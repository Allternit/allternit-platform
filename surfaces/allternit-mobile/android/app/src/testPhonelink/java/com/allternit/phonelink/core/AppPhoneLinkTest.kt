package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.Base64

class AppPhoneLinkTest {
    private val base = "https://api.test"
    private val phoneId = "11111111-2222-3333-4444-555555555555"
    private val http = FakeHttp()
    private val sockets = FakeConnector()
    private val store = FakeCredentialStore()
    private val runtime = FakeLinkRuntime()
    private val host = FakeScreenHost()
    private val ops = FakeDeviceOps()
    private val input = InputEvents(ops, { InputGeometry(720, 1600, 720, 1600) }, { runtime.now })
    private val appPriv = ByteArray(32) { (it + 1).toByte() }
    private val nodePriv = ByteArray(32) { (it + 100).toByte() }

    private val codes = ArrayList<String>()
    private val failures = ArrayList<PairFailure>()
    private var paired: AppPhoneCredentials? = null
    private var unpaired = 0
    private val controlStates = ArrayList<Boolean>()

    private val listener = object : LinkListener {
        override fun onPairCode(code: String) { codes.add(code) }
        override fun onPaired(credentials: AppPhoneCredentials) { paired = credentials }
        override fun onPairFailed(reason: PairFailure, message: String) { failures.add(reason) }
        override fun onControlConnected(connected: Boolean) { controlStates.add(connected) }
        override fun onUnpaired() { unpaired++ }
    }

    private fun link() = AppPhoneLink(base, http, sockets, store, runtime, host, input, "moto g play", listener) { appPriv }

    private fun redeemOk() {
        http.replies["/api/v1/app-phones/claim/redeem"] = HttpReply(200, JSONObject()
            .put("phoneId", phoneId.uppercase()).put("serial", "app:$phoneId").put("appPhoneKey", "key-1")
            .put("runtimeDeviceId", "rt-1").put("socketTicketPath", "/x").toString())
        http.replies["/socket-ticket"] = HttpReply(200, """{"ticket":"t 1"}""")
    }

    // ── Redeem ──────────────────────────────────────────────────────────────

    @Test fun redeemNormalisesCodeAndSendsNoAuth() {
        redeemOk()
        val reg = link().redeem("abcd-12 34", "Moto")
        assertEquals(Registration(phoneId, "rt-1", "key-1", "Moto"), reg)
        val call = http.calls.single()
        assertEquals("$base/api/v1/app-phones/claim/redeem", call.url)
        assertEquals("ABCD1234", JSONObject(call.body).getString("code"))
        assertEquals("Moto", JSONObject(call.body).getString("name"))
        assertFalse(call.headers.keys.any { it.equals("authorization", true) })
    }

    @Test fun redeemFailuresAreTyped() {
        http.replies["/claim/redeem"] = HttpReply(401, "{}")
        try { link().redeem("X", null); fail() } catch (e: RelayFailure) { assertEquals(401, e.status) }
        http.replies["/claim/redeem"] = HttpReply(429, "{}")
        try { link().redeem("X", null); fail() } catch (e: RelayFailure) { assertEquals(429, e.status) }
    }

    @Test fun ticketUsesKeyHeaderAndBuildsRuntimeSocketUrl() {
        redeemOk()
        val url = link().socketUrl("ingest", phoneId, "rt 1", "key-1")
        assertEquals("wss://api.test/api/v1/runtime-devices/rt%201/socket?ticket=t%201", url)
        val call = http.calls.single()
        assertEquals("$base/api/v1/app-phones/$phoneId/socket-ticket", call.url)
        assertEquals("key-1", call.headers["x-allternit-app-phone-key"])
        assertEquals("/api/v1/phones/app/ingest?phone=$phoneId", JSONObject(call.body).getString("path"))
    }

    // ── Pairing ─────────────────────────────────────────────────────────────

    private fun pairUpToPending(l: AppPhoneLink): FakeSocket {
        redeemOk()
        l.pair("ABCD1234", "Moto")
        val s = sockets.last("/runtime-devices/")
        s.listener.onOpen()
        val nodePub = X25519.publicKey(nodePriv)
        s.listener.onText(JSONObject().put("type", "pending").put("pub", Base64.getEncoder().encodeToString(nodePub)).toString())
        return s
    }

    @Test fun pairHandshakeDerivesTheSameKeyAndCodeAsTheNode() {
        val l = link()
        val s = pairUpToPending(l)
        val hello = JSONObject(s.texts.single())
        assertEquals("hello", hello.getString("type"))
        assertEquals("android", hello.getString("platform"))
        assertEquals(phoneId, hello.getString("phoneId"))
        assertEquals("moto g play", hello.getString("model"))
        val appPub = X25519.publicKey(appPriv)
        assertArrayEquals(appPub, Base64.getDecoder().decode(hello.getString("pub")))
        assertEquals(listOf(AppPhoneLink.pairCode(appPub, X25519.publicKey(nodePriv))), codes)
        assertTrue(codes[0].matches(Regex("\\d{6}")))

        s.listener.onText("""{"type":"approved"}""")
        val creds = paired!!
        assertEquals(creds, store.value)
        assertEquals("app:$phoneId", creds.serial)
        assertEquals(AppPhoneLink.sessionKey(nodePriv, appPub, phoneId).toHex(), creds.sessionKeyHex)
        assertEquals(32, creds.sessionKey!!.size)
        assertTrue(l.isPaired)
        assertNotNull(s.closed)
        assertTrue(failures.isEmpty())
    }

    @Test fun deniedAndTimeoutAndEarlyCloseFailTheRightWay() {
        pairUpToPending(link()).listener.onText("""{"type":"denied"}""")
        assertEquals(listOf(PairFailure.DENIED), failures)

        failures.clear()
        pairUpToPending(link())
        runtime.advance(AppPhoneLink.APPROVAL_TIMEOUT_MS)
        assertEquals(listOf(PairFailure.TIMED_OUT), failures)

        failures.clear()
        pairUpToPending(link()).listener.onClosed()
        assertEquals(listOf(PairFailure.CLOSED), failures)
        assertNull(store.value)
    }

    @Test fun nodeKeyChangeAbortsPairing() {
        val s = pairUpToPending(link())
        s.listener.onText(JSONObject().put("type", "pending").put("pub", Base64.getEncoder().encodeToString(ByteArray(32) { 7 })).toString())
        assertEquals(listOf(PairFailure.KEY_MISMATCH), failures)
    }

    @Test fun badCodeSurfacesAsRefused() {
        http.replies["/claim/redeem"] = HttpReply(401, "{}")
        link().pair("NOPE", null)
        assertEquals(listOf(PairFailure.REFUSED), failures)
        assertTrue(sockets.sockets.isEmpty())
    }

    // ── Control ─────────────────────────────────────────────────────────────

    private val key = ByteArray(32) { (it * 3).toByte() }

    private fun paired() = AppPhoneCredentials(phoneId, "Moto", "rt-1", "key-1", key.toHex(), base).also { store.value = it }

    private fun nodeSeal(body: JSONObject, k: ByteArray = key, nonce: ByteArray = SecureEnvelope.randomBytes(16)) =
        SecureEnvelope.sealRequest(k, body.toString(), runtime.now / 1000, nonce).toString()

    private fun appOpen(text: String): JSONObject =
        JSONObject(SecureEnvelope.open(SecureEnvelope.parse(JSONObject(text)), key))

    private fun openControl(): Pair<AppPhoneLink, FakeSocket> {
        http.replies["/socket-ticket"] = HttpReply(200, """{"ticket":"c1"}""")
        val l = link()
        l.startControl()
        val s = sockets.last("/runtime-devices/")
        s.listener.onOpen()
        return l to s
    }

    @Test fun controlSendsStateWithTapAndConsent() {
        paired(); host.tap = true; host.consent = Consent.GRANTED
        val (l, s) = openControl()
        val state = appOpen(s.texts[0])
        assertEquals("state", state.getString("type"))
        assertTrue(state.getBoolean("canTap"))
        assertEquals("granted", state.getString("consent"))
        assertTrue(state.getBoolean("foreground"))
        assertEquals(listOf(true), controlStates)

        host.tap = false; host.consent = Consent.DENIED
        l.stateChanged()
        val next = appOpen(s.texts[1])
        assertFalse(next.getBoolean("canTap")); assertEquals("denied", next.getString("consent"))
    }

    @Test fun stateCarriesTheBluetoothKeyboardForTheIPhoneListing() {
        paired()
        val (l, s) = openControl()
        assertFalse(appOpen(s.texts[0]).has("keyboard"))

        host.keyboard = JSONObject().put("on", true).put("paired", true).put("connected", true).put("host", "Sam's iPhone")
        l.stateChanged()
        val kb = appOpen(s.texts[1]).getJSONObject("keyboard")
        assertTrue(kb.getBoolean("on")); assertTrue(kb.getBoolean("connected"))
        assertEquals("Sam's iPhone", kb.getString("host"))
    }

    @Test fun inputEventsRunAndAreAnswered() {
        paired()
        val (_, s) = openControl()
        val req = JSONObject().put("id", "r1").put("method", "input.events")
            .put("params", JSONObject("""{"events":[{"type":"home"},{"type":"power"}]}"""))
        s.listener.onText(nodeSeal(req))
        assertEquals(listOf("global HOME"), ops.actions)
        val reply = appOpen(s.texts.last())
        assertEquals("r1", reply.getString("id"))
        assertEquals(1, reply.getJSONObject("result").getInt("handled"))
        assertEquals(1, reply.getJSONObject("result").getInt("ignored"))
    }

    @Test fun badInputAndUnknownMethodReturnErrors() {
        paired()
        val (_, s) = openControl()
        s.listener.onText(nodeSeal(JSONObject("""{"id":"a","method":"input.events","params":{"events":[{"type":"zap"}]}}""")))
        assertEquals("invalid_params", appOpen(s.texts.last()).getJSONObject("error").getString("code"))
        s.listener.onText(nodeSeal(JSONObject("""{"id":"b","method":"web.open"}""")))
        assertEquals("unknown_method", appOpen(s.texts.last()).getJSONObject("error").getString("code"))
    }

    @Test fun screenRequestCallsTheHost() {
        paired()
        val (_, s) = openControl()
        s.listener.onText(nodeSeal(JSONObject("""{"type":"screen_request","serial":"app:$phoneId"}""")))
        assertEquals(1, host.requests)
        assertTrue(ops.actions.isEmpty())
    }

    @Test fun replayedWrongKeyStaleAndPlainFramesAreDropped() {
        paired()
        val (_, s) = openControl()
        val sent = s.texts.size
        val msg = JSONObject("""{"type":"screen_request"}""")
        val once = nodeSeal(msg)
        s.listener.onText(once); s.listener.onText(once)
        assertEquals(1, host.requests)
        s.listener.onText(nodeSeal(msg, k = ByteArray(32) { 9 }))
        s.listener.onText(msg.toString())
        runtime.now += 10 * 60 * 1000
        s.listener.onText(SecureEnvelope.sealRequest(key, msg.toString(), runtime.now / 1000 - 600, SecureEnvelope.randomBytes(16)).toString())
        assertEquals(1, host.requests)
        assertEquals(sent, s.texts.size)
    }

    @Test fun controlReconnectsWithBackoffAfterClose() {
        paired()
        val (_, s) = openControl()
        s.listener.onClosed()
        assertEquals(listOf(true, false), controlStates)
        assertEquals(1, sockets.sockets.size)
        runtime.advance(1_000)
        assertEquals(2, sockets.sockets.size)
        sockets.sockets[1].listener.onClosed() // never opened: no extra "disconnected"
        runtime.advance(1_000)
        assertEquals(2, sockets.sockets.size)
        runtime.advance(1_000)
        assertEquals(3, sockets.sockets.size)
    }

    @Test fun revokedTicketClearsThePairing() {
        paired()
        http.replies["/socket-ticket"] = HttpReply(401, "{}")
        val l = link()
        l.startControl()
        assertEquals(1, unpaired)
        assertNull(store.value)
        assertFalse(l.isPaired)
        assertTrue(sockets.sockets.isEmpty())
    }

    @Test fun networkErrorOnTicketRetriesNotUnpairs() {
        paired()
        http.replies["/socket-ticket"] = HttpReply(503, "{}")
        link().startControl()
        assertEquals(0, unpaired)
        http.replies["/socket-ticket"] = HttpReply(200, """{"ticket":"c1"}""")
        runtime.advance(1_000)
        assertEquals(1, sockets.sockets.size)
    }

    @Test fun stopControlClosesSocket() {
        paired()
        val (l, s) = openControl()
        l.stopControl()
        assertEquals(1001, s.closed!!.first)
        s.listener.onClosed()
        runtime.advance(60_000)
        assertEquals(1, sockets.sockets.size)
    }

    // ── Ingest ──────────────────────────────────────────────────────────────

    private fun decryptFrame(text: ByteArray): ByteArray {
        val p = SecureEnvelope.parse(JSONObject(String(text, Charsets.UTF_8)))
        assertEquals(SecureEnvelope.tokenId(key), p.tokId)
        val kMac = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_MAC.toByteArray(), p.nonce)
        val expected = SecureEnvelope.hmac(kMac, "v1".toByteArray(), p.tok4, SecureEnvelope.be64(p.ts), p.nonce, p.ct)
        assertTrue(SecureEnvelope.constantTimeEquals(expected, p.tag))
        val kEnc = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_ENC_REQ.toByteArray(), p.nonce)
        return SecureEnvelope.keystreamXor(kEnc, p.nonce, p.ct)
    }

    private class IngestEvents : AppPhoneLink.IngestListener {
        var ready = 0; var closed = 0; val controls = ArrayList<String>(); val fatal = ArrayList<String>()
        override fun onReady() { ready++ }
        override fun onClosed() { closed++ }
        override fun onControl(type: String) { controls.add(type) }
        override fun onFatal(message: String) { fatal.add(message) }
    }

    @Test fun ingestSealsBinaryPacketsAndPassesNodeControls() {
        paired()
        http.replies["/socket-ticket"] = HttpReply(200, """{"ticket":"i1"}""")
        val ev = IngestEvents()
        val ch = link().openIngest(ev)
        val s = sockets.last("/runtime-devices/")
        assertFalse(ch.send(BroadcastPacket.status(true))) // not open yet
        s.listener.onOpen()
        assertEquals(1, ev.ready)
        val frame = BroadcastPacket.frame(byteArrayOf(0, 0, 0, 1, 0x65, 0xff.toByte(), 0x80.toByte()))
        assertTrue(ch.send(BroadcastPacket.config(720, 1600, 2.0, 0)))
        assertTrue(ch.send(frame))
        assertArrayEquals(BroadcastPacket.config(720, 1600, 2.0, 0), decryptFrame(s.binaries[0]))
        assertArrayEquals(frame, decryptFrame(s.binaries[1]))

        s.listener.onText("""{"type":"keyframe"}""")
        s.listener.onText(nodeSeal(JSONObject("""{"type":"resume"}""")))
        assertEquals(listOf("keyframe", "resume"), ev.controls)

        ch.finish(BroadcastPacket.ended("stopped"))
        assertArrayEquals(BroadcastPacket.ended("stopped"), decryptFrame(s.binaries[2]))
        assertEquals(1000, s.closed!!.first)
        runtime.advance(60_000)
        assertEquals(1, sockets.sockets.size) // finished: no retry
    }

    @Test fun ingestRetriesAfterDropAndStopsOnRevoke() {
        paired()
        http.replies["/socket-ticket"] = HttpReply(200, """{"ticket":"i1"}""")
        val ev = IngestEvents()
        val ch = link().openIngest(ev)
        sockets.sockets[0].listener.onOpen()
        sockets.sockets[0].listener.onClosed()
        assertEquals(1, ev.closed)
        assertFalse(ch.isReady)
        runtime.advance(1_000)
        assertEquals(2, sockets.sockets.size)

        sockets.sockets[1].listener.onClosed()
        http.replies["/socket-ticket"] = HttpReply(404, "{}")
        runtime.advance(2_000)
        assertEquals(1, ev.fatal.size)
        assertNull(store.value)
        assertEquals(1, unpaired)
    }
}
