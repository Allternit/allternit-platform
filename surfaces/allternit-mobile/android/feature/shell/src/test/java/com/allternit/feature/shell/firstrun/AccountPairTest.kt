package com.allternit.feature.shell.firstrun

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Connections rows, design pick B grouped (step 11). Mirrors iOS AccountPairAnswerRoomTests and the web accountPair test. */
class AccountPairTest {
    @Test fun groupsOnlyWhenBothRowsAreListed() {
        assertTrue(AccountPair.pairsUp(listOf("chatgpt", "codex", "claude")))
        assertFalse(AccountPair.pairsUp(listOf("chatgpt", "claude")))
        assertFalse(AccountPair.pairsUp(listOf("codex")))
    }

    @Test fun chatgptRowIsTheSubscriptionWithAModelCount() {
        val e = AccountPair.extras("chatgpt", ConnectionWire("chatgpt", ConnState.On, modelCount = 3), paired = true)!!
        assertEquals("ChatGPT subscription", e.title)
        assertEquals("Device sign-in", e.badge)
        assertTrue(e.powers.startsWith("Powers: chat models"))
        assertEquals("Signed in · 3 models", AccountPair.statusLine(e, ConnState.On, "x"))
        assertEquals("Not signed in", AccountPair.statusLine(e, ConnState.Off, "x"))
    }

    @Test fun noModelCountWhenNoneAreKnown() {
        val none = AccountPair.extras("chatgpt", ConnectionWire("chatgpt", ConnState.On), paired = false)!!
        assertEquals("Signed in", AccountPair.statusLine(none, ConnState.On, "x"))
        val one = AccountPair.extras("chatgpt", ConnectionWire("chatgpt", ConnState.On, modelCount = 1), paired = false)!!
        assertEquals("Signed in · 1 model", AccountPair.statusLine(one, ConnState.On, "x"))
    }

    @Test fun codexRowIsItsOwnSignInWithItsOwnButtons() {
        val row = ConnectionWire("codex", ConnState.On, found = "Codex CLI ready")
        val paired = AccountPair.extras("codex", row, paired = true)!!
        assertEquals("Codex CLI", paired.title)
        assertEquals("Own sign-in", paired.badge)
        assertEquals("Set up", paired.connectLabel)
        assertEquals("Sign out", paired.disconnectLabel)
        assertTrue(paired.powers.contains("Signing out of one does not sign out the other"))
        assertEquals("Signed in · Codex CLI ready", AccountPair.statusLine(paired, ConnState.On, "x"))
        assertEquals("Not set up", AccountPair.statusLine(paired, ConnState.Off, "x"))
        assertNull(paired.modelCount)
        assertEquals("Powers: coding runs.", AccountPair.extras("codex", row, paired = false)!!.powers)
    }

    @Test fun needsYouKeepsTheRealReason() {
        val e = AccountPair.extras("chatgpt", ConnectionWire("chatgpt", ConnState.Needs), paired = true)!!
        assertEquals("Needs you · Sign in again", AccountPair.statusLine(e, ConnState.Needs, "Needs you · Sign in again"))
    }

    @Test fun otherRowsGetNoRole() {
        assertNull(AccountPair.extras("claude", ConnectionWire("claude"), paired = true))
    }

    @Test fun theWireRowCountsItsModels() {
        val row = ConnectionWire.parse(kotlinx.serialization.json.Json.parseToJsonElement("""{"id":"chatgpt","state":"on","models":["a","b"]}"""))!!
        assertEquals(2, row.modelCount)
        assertEquals(0, ConnectionWire.parse(kotlinx.serialization.json.Json.parseToJsonElement("""{"id":"chatgpt"}"""))!!.modelCount)
    }
}
