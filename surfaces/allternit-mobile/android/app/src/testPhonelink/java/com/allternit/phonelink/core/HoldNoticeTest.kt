package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class HoldNoticeTest {
    private val now = 1_800_000_000_000L // 2027-01-15

    @Test fun keepsTheNoticeUntilTheHoldExpiresPlusAMargin() {
        val at = java.time.Instant.ofEpochMilli(now + 120_000).toString()
        assertEquals(125_000L, HoldNotice.timeoutMs(at, now))
    }

    @Test fun neverKeepsItLongerThanTheCapEvenForALateExpiry() {
        val at = java.time.Instant.ofEpochMilli(now + 3_600_000).toString()
        assertEquals(HoldNotice.MAX_MS, HoldNotice.timeoutMs(at, now))
    }

    @Test fun anExpiredOrMissingOrUnreadableTimeStillEndsTheNotice() {
        assertEquals(5_000L, HoldNotice.timeoutMs(java.time.Instant.ofEpochMilli(now - 60_000).toString(), now))
        assertEquals(HoldNotice.MAX_MS, HoldNotice.timeoutMs(null, now))
        assertEquals(HoldNotice.MAX_MS, HoldNotice.timeoutMs("soon", now))
    }

    @Test fun bodyIsPlainWordsWithTheAppWhenKnown() {
        assertTrue(HoldNotice.body("send", "WhatsApp").startsWith("Your bot wants to send a message in WhatsApp."))
        assertTrue(HoldNotice.body("weird", null).startsWith("Your bot wants to do something that needs your OK."))
    }

    @Test fun dispatcherPassesTheNoticeToTheHooksAndRejectsABadState() {
        val hooks = FakeHooks()
        val d = RpcDispatcher({ FakeDeviceOps() }, { FakeDeviceOps.INFO }, AuthManager(MemorySessionStore()), hooks, "com.allternit.phonelink", clock = { 1_800_000_000_000L })
        val p = JSONObject().put("state", "waiting").put("effect", "pay").put("app", "com.bank")
            .put("expiresAt", java.time.Instant.ofEpochMilli(1_800_000_060_000L).toString())
        d.dispatch("hold_notice", p)
        assertEquals(listOf<Any?>(true, "pay", "com.bank", 65_000L), hooks.holdNotices[0])
        d.dispatch("hold_notice", JSONObject().put("state", "cleared"))
        assertFalse(hooks.holdNotices[1][0] as Boolean)
        try { d.dispatch("hold_notice", JSONObject().put("state", "x")); throw AssertionError("expected invalid") } catch (e: RpcException) { assertEquals(RpcException.INVALID_PARAMS, e.code) }
    }
}
