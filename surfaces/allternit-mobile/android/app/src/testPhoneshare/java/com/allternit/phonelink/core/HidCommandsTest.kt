package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Report encoding and the hid.* methods (contract §9.2). Runs in both builds. */
class HidCommandsTest {
    private class FakeLink(var state: HidState = HidState(HidAvailability.READY, paired = true, connected = true, name = "Eve's iPhone")) : HidLink {
        val sent = ArrayList<Pair<Int, List<Int>>>()
        var accept = true
        override fun state() = state
        override fun send(reportId: Int, data: ByteArray): Boolean {
            if (!accept) return false
            sent.add(reportId to data.map { it.toInt() })
            return true
        }
        fun mouse() = sent.filter { it.first == HidReports.MOUSE_ID }.map { it.second }
        fun keyboard() = sent.filter { it.first == HidReports.KEYBOARD_ID }.map { it.second }
    }

    private fun setup(state: HidState? = null): Pair<FakeLink, HidCommands> {
        val link = if (state != null) FakeLink(state) else FakeLink()
        return link to HidCommands(link, pause = {})
    }

    private fun p(vararg kv: Pair<String, Any>) = JSONObject().apply { kv.forEach { put(it.first, it.second) } }

    private fun rpcError(run: () -> Unit): RpcException {
        try { run() } catch (e: RpcException) { return e }
        fail("expected an RpcException"); throw AssertionError()
    }

    // ── Encoding ────────────────────────────────────────────────────────────

    @Test fun keyboardReportIsModifiersReservedAndSixKeys() {
        assertArrayEquals(byteArrayOf(0x0A, 0, 0x2C, 0x04, 0, 0, 0, 0), HidReports.keyboard(HidReports.CMD or HidReports.SHIFT, listOf(0x2C, 0x04)))
        // A seventh key cannot fit a boot report: dropped, not wrapped.
        assertEquals(8, HidReports.keyboard(0, (4..10).toList()).size)
        assertEquals(9, HidReports.keyboard(0, (4..10).toList())[7].toInt())
    }

    @Test fun mouseReportClampsEveryAxisToASignedByte() {
        assertArrayEquals(byteArrayOf(1, 127, -127, 5), HidReports.mouse(HidReports.BUTTON_LEFT, 500, -900, 5))
        assertArrayEquals(byteArrayOf(2, -127, 0, 127), HidReports.mouse(HidReports.BUTTON_RIGHT, -128, 0, 128))
    }

    @Test fun stepsAddUpExactlyWithinASignedByte() {
        val s = HidReports.steps(-300)
        assertEquals(-300, s.sum())
        assertTrue(s.all { it in -127..127 })
        assertTrue(HidReports.steps(0).isEmpty())
    }

    @Test fun descriptorDeclaresBothReportIdsAndTheBootKeyRange() {
        val d = HidReports.DESCRIPTOR.map { it.toInt() and 0xFF }
        fun has(vararg seq: Int) = d.windowed(seq.size).any { it == seq.toList() }
        assertTrue("keyboard report id", has(0x85, 1))
        assertTrue("mouse report id", has(0x85, 2))
        assertTrue("key array max 101", has(0x29, 0x65, 0x81, 0x00))
        assertTrue("x, y, wheel relative", has(0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F))
        // Collections open and close in pairs.
        assertEquals(d.windowed(2).count { it == listOf(0xA1, 0x01) } + d.windowed(2).count { it == listOf(0xA1, 0x00) }, d.count { it == 0xC0 })
    }

    @Test fun usLayoutStrokesMatchTheDesktopTable() {
        assertEquals(0x04 to 0, HidReports.charStroke('a'))
        assertEquals(0x04 to HidReports.SHIFT, HidReports.charStroke('A'))
        assertEquals(0x27 to 0, HidReports.charStroke('0'))
        assertEquals(0x1F to HidReports.SHIFT, HidReports.charStroke('@'))
        assertEquals(0x2C to 0, HidReports.charStroke(' '))
        assertNull(HidReports.charStroke('é'))
        assertNull(HidReports.charStroke('\n'))
    }

    // ── Methods ─────────────────────────────────────────────────────────────

    @Test fun pointerMoveIsClampedAndCarriesHeldButtons() {
        val (link, hid) = setup()
        hid.handle("hid.pointer.click", p("button" to "left", "up" to false))
        hid.handle("hid.pointer.move", p("dx" to 300, "dy" to -20))
        assertEquals(listOf(1, 127, -20, 0), link.mouse().last())
        hid.handle("hid.pointer.click", p("button" to "left", "down" to false))
        assertEquals(listOf(0, 0, 0, 0), link.mouse().last())
    }

    @Test fun aFullClickIsPressThenRelease() {
        val (link, hid) = setup()
        hid.handle("hid.pointer.click", p("button" to "right"))
        assertEquals(listOf(listOf(2, 0, 0, 0), listOf(0, 0, 0, 0)), link.mouse())
    }

    @Test fun cornerResetSweepsUpAndLeftPastTheCoresReach() {
        val (link, hid) = setup()
        hid.handle("hid.pointer.reset", JSONObject())
        val moves = link.mouse()
        assertEquals(HidReports.CORNER_STEPS, moves.size)
        assertTrue(moves.sumOf { -it[1] } >= 20_000)
        assertTrue(moves.all { it[0] == 0 && it[1] == -127 && it[2] == -127 })
    }

    @Test fun scrollDownIsANegativeWheelSplitIntoBytes() {
        val (link, hid) = setup()
        hid.handle("hid.scroll", p("lines" to 200))
        val wheel = link.mouse().map { it[3] }
        assertEquals(-200, wheel.sum())
        assertTrue(wheel.all { it in -127..-1 })
        hid.handle("hid.scroll", p("lines" to -3))
        assertEquals(3, link.mouse().last()[3])
    }

    @Test fun keyDownAndUpTrackModifiersAndKeys() {
        val (link, hid) = setup()
        hid.handle("hid.key", p("usage" to 0x2C, "modifiers" to HidReports.CMD, "down" to true))
        assertEquals(listOf(8, 0, 0x2C, 0, 0, 0, 0, 0), link.keyboard().last())
        hid.handle("hid.key", p("usage" to 0x2C, "modifiers" to HidReports.CMD, "down" to false))
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0), link.keyboard().last())
    }

    @Test fun keyZeroUpReleasesEverything() {
        val (link, hid) = setup()
        hid.handle("hid.key", p("usage" to 0x04, "modifiers" to 2, "down" to true))
        hid.handle("hid.key", p("usage" to 0x05, "modifiers" to 0, "down" to true))
        hid.handle("hid.key", p("usage" to 0, "modifiers" to 0, "down" to false))
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0), link.keyboard().last())
    }

    @Test fun typeSendsDownUpPerCharacterAndRefusesWhatTheLayoutCannotSend() {
        val (link, hid) = setup()
        val r = hid.handle("hid.type", p("text" to "Hi!"))
        assertEquals(3, r.getInt("sent"))
        val kb = link.keyboard()
        assertEquals(listOf(2, 0, 0x0B, 0, 0, 0, 0, 0), kb[0]) // H
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0), kb[1])
        assertEquals(listOf(0, 0, 0x0C, 0, 0, 0, 0, 0), kb[2]) // i
        assertEquals(listOf(2, 0, 0x1E, 0, 0, 0, 0, 0), kb[4]) // !
        val before = link.sent.size
        assertEquals("bad_params", rpcError { hid.handle("hid.type", p("text" to "café")) }.type)
        assertEquals("nothing is sent when one character is refused", before, link.sent.size)
    }

    @Test fun badArgumentsAreInvalidParams() {
        val (_, hid) = setup()
        assertEquals("invalid_params", rpcError { hid.handle("hid.key", p("usage" to 0x200, "modifiers" to 0, "down" to true)) }.type)
        assertEquals("invalid_params", rpcError { hid.handle("hid.pointer.click", p("button" to "middle")) }.type)
        assertEquals("invalid_params", rpcError { hid.handle("hid.pointer.move", p("dx" to "1", "dy" to 0)) }.type)
        assertEquals("invalid_params", rpcError { hid.handle("hid.type", p("text" to "x".repeat(501))) }.type)
    }

    @Test fun releaseAllLetsGoOfHeldKeysAndButtons() {
        val (link, hid) = setup()
        hid.handle("hid.key", p("usage" to 0x04, "modifiers" to 8, "down" to true))
        hid.handle("hid.pointer.click", p("button" to "left", "up" to false))
        hid.releaseAll()
        assertEquals(listOf(0, 0, 0, 0, 0, 0, 0, 0), link.keyboard().last())
        assertEquals(listOf(0, 0, 0, 0), link.mouse().last())
        val n = link.sent.size
        hid.releaseAll()
        assertEquals("nothing held: nothing sent", n, link.sent.size)
    }

    // ── State mapping (contract §5) ─────────────────────────────────────────

    @Test fun statusMapsToTheContractHealthStates() {
        val connected = HidState(HidAvailability.READY, paired = true, connected = true, name = "iPhone")
        assertNull(connected.needsYou())
        assertNull(connected.unavailableReason())
        assertEquals(true, connected.toJson().getBoolean("connected"))

        val waiting = HidState(HidAvailability.READY, paired = true, connected = false)
        assertEquals("BT_NOT_PAIRED", waiting.needsYou())
        assertEquals("BT_NOT_PAIRED", waiting.toJson().getString("needs_you"))

        val unsupported = HidState(HidAvailability.UNSUPPORTED, paired = false, connected = false)
        assertNull("not something the person can fix on the iPhone", unsupported.needsYou())
        assertEquals(HidState.NOT_AVAILABLE_MESSAGE, unsupported.unavailableReason())
        assertFalse(unsupported.toJson().getBoolean("available"))
        assertEquals("unsupported", unsupported.toJson().getString("availability"))
    }

    @Test fun pairStatusAnswersInEveryStateButSendsNeedAConnection() {
        val (link, hid) = setup(HidState(HidAvailability.READY, paired = false, connected = false))
        assertEquals(false, hid.handle("hid.pair.status", JSONObject()).getBoolean("connected"))
        assertEquals("bt_not_paired", rpcError { hid.handle("hid.pointer.move", p("dx" to 1, "dy" to 1)) }.type)
        link.state = HidState(HidAvailability.BLUETOOTH_OFF, paired = true, connected = false)
        assertEquals("hid_unavailable", rpcError { hid.handle("hid.key", p("usage" to 4, "modifiers" to 0, "down" to true)) }.type)
        assertTrue(link.sent.isEmpty())
    }

    @Test fun aRefusedReportIsHidFailedWhileConnected() {
        val (link, hid) = setup()
        link.accept = false
        assertEquals("hid_failed", rpcError { hid.handle("hid.pointer.move", p("dx" to 1, "dy" to 1)) }.type)
    }
}
