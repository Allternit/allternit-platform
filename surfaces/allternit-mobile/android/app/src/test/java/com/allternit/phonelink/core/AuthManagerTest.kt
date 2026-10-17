package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class AuthManagerTest {
    private var now = 1_760_600_000_000L
    private val store = MemorySessionStore()

    private fun manager(approver: PairApprover? = ScriptedApprover(true), timeoutMs: Long = 200, armed: (Boolean) -> Boolean = { false }) =
        AuthManager(store, clock = { now }, approvalTimeoutMs = timeoutMs, consumeArmedGrant = armed).also { it.approver = approver }

    private fun pairOk(m: AuthManager, name: String = "Desktop", local: Boolean = false): AuthSession {
        val o = m.requestAuth(name, local, "abcd1234")
        assertTrue("expected approval, got $o", o is PairOutcome.Approved)
        return (o as PairOutcome.Approved).session
    }

    private fun envelope(s: AuthSession, body: String = """{"jsonrpc":"2.0","method":"device_info","id":1}""", ts: Long = now / 1000, nonce: ByteArray = SecureEnvelope.randomBytes(16)) =
        SecureEnvelope.sealRequest(s.key!!, body, ts, nonce)

    private fun expect(code: String, block: () -> Unit) {
        try { block(); fail("expected $code") } catch (f: SecureEnvelope.Failure) { assertEquals(code, f.code) }
    }

    @Test fun approvedPairingMintsAKeyWhoseIdIsItsHash() {
        val m = manager()
        val s = pairOk(m)
        assertEquals(SecureEnvelope.tokenId(s.key!!), s.id)
        assertEquals(1, store.saved.size)
    }

    @Test fun deniedPairingStartsCooldown() {
        val m = manager(ScriptedApprover(false))
        assertEquals(PairOutcome.Denied, m.requestAuth("X", false, null))
        val again = m.requestAuth("X", false, null)
        assertTrue(again is PairOutcome.CoolingDown)
        now += AuthManager.PAIR_COOLDOWN_MS + 1
        assertEquals(PairOutcome.Denied, m.requestAuth("X", false, null))
    }

    @Test fun unansweredPairingTimesOutAndWithdrawsThePrompt() {
        val approver = ScriptedApprover(null)
        val m = manager(approver, timeoutMs = 100)
        assertEquals(PairOutcome.Denied, m.requestAuth("X", false, null))
        assertEquals(1, approver.expired.size)
    }

    @Test fun secondRequestWhileOnePendsIsBusyButLoopbackHasItsOwnSlot() {
        val approver = ScriptedApprover(null)
        val m = manager(approver, timeoutMs = 1500)
        val pool = Executors.newSingleThreadExecutor()
        val first = pool.submit<PairOutcome> { m.requestAuth("lan-1", false, null) }
        Thread.sleep(150)
        assertTrue(m.requestAuth("lan-2", false, null) is PairOutcome.Busy)
        // The phone's own slot is independent; it also waits, so arm it to answer.
        approver.answer = true
        assertTrue(m.requestAuth("usb", true, null) is PairOutcome.Approved)
        first.get(3, TimeUnit.SECONDS)
        pool.shutdown()
    }

    @Test fun armedGrantSkipsThePromptOnce() {
        var armed = true
        val approver = ScriptedApprover(false)
        val m = manager(approver, armed = { local -> if (local && armed) { armed = false; true } else false })
        assertTrue(m.requestAuth("Desktop", true, null) is PairOutcome.Approved)
        assertTrue(approver.requests.isEmpty())
        assertEquals(PairOutcome.Denied, m.requestAuth("Desktop", true, null))
    }

    @Test fun noApproverMeansDenied() {
        assertEquals(PairOutcome.Denied, manager(approver = null).requestAuth("X", false, null))
    }

    @Test fun validEnvelopeOpensAndSealsTheReply() {
        val m = manager()
        val s = pairOk(m)
        val nonce = SecureEnvelope.randomBytes(16)
        val opened = m.openEnvelope(envelope(s, nonce = nonce), Peer("192.168.1.5", "lan"))
        assertEquals(s.id, opened.tokId)
        assertTrue(opened.plaintext.contains("device_info"))
        val reply = JSONObject(opened.seal("""{"jsonrpc":"2.0","result":{},"id":1}"""))
        assertEquals("""{"jsonrpc":"2.0","result":{},"id":1}""", SecureEnvelope.openResponse(s.key!!, nonce, reply))
        assertEquals("192.168.1.5", m.activeSessions.first().lastAddress)
    }

    @Test fun replayedNonceIsRejected() {
        val m = manager()
        val s = pairOk(m)
        val env = envelope(s)
        m.openEnvelope(env)
        expect("nonce_replayed") { m.openEnvelope(JSONObject(env.toString())) }
    }

    @Test fun clockSkewBeyond120sIsRejectedWithItsOwnCode() {
        val m = manager()
        val s = pairOk(m)
        m.openEnvelope(envelope(s, ts = now / 1000 + 120))
        m.openEnvelope(envelope(s, ts = now / 1000 - 120))
        expect("ts_skew") { m.openEnvelope(envelope(s, ts = now / 1000 + 121)) }
        expect("ts_skew") { m.openEnvelope(envelope(s, ts = now / 1000 - 121)) }
    }

    @Test fun unknownTokenAndForgedTagAreRejected() {
        val m = manager()
        val s = pairOk(m)
        val other = SecureEnvelope.sealRequest(ByteArray(32) { 7 }, "{}", now / 1000, SecureEnvelope.randomBytes(16))
        expect("auth_failed") { m.openEnvelope(other) }
        val forged = envelope(s).put("tag", "00".repeat(32))
        expect("bad_tag") { m.openEnvelope(forged) }
    }

    @Test fun forgedRequestDoesNotBurnTheNonce() {
        val m = manager()
        val s = pairOk(m)
        val nonce = SecureEnvelope.randomBytes(16)
        val good = envelope(s, nonce = nonce)
        expect("bad_tag") { m.openEnvelope(JSONObject(good.toString()).put("tag", "11".repeat(32))) }
        assertNotNull(m.openEnvelope(good))
    }

    @Test fun idleSessionExpiresAfter30DaysButUseSlidesTheWindow() {
        val m = manager()
        val s = pairOk(m)
        now += AuthManager.INACTIVITY_MS - 1000
        m.openEnvelope(envelope(s)) // slides lastActive forward
        now += AuthManager.INACTIVITY_MS - 1000
        m.openEnvelope(envelope(s))
        now += AuthManager.INACTIVITY_MS + 1000
        expect("auth_failed") { m.openEnvelope(envelope(s)) }
        assertTrue(m.activeSessions.isEmpty())
    }

    @Test fun sessionsSurviveARestartThroughTheStore() {
        val s = pairOk(manager())
        val restarted = AuthManager(store, clock = { now })
        assertNotNull(restarted.openEnvelope(envelope(s)))
    }

    @Test fun revokeKillsTheKey() {
        val m = manager()
        val s = pairOk(m)
        assertTrue(m.revokeSession(s.id))
        expect("auth_failed") { m.openEnvelope(envelope(s)) }
    }

    @Test fun sessionJsonRoundTrip() {
        val s = AuthSession("630dcd29", "00".repeat(32), "Desk", 1, 2, null, "lan")
        val back = AuthSession.listFromJson(AuthSession.listToJson(listOf(s)))
        assertEquals(listOf(s), back)
    }
}
