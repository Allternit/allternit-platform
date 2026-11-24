package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class InputEventsTest {
    private val ops = FakeDeviceOps()
    private var now = 1_000L
    // Video 360x800 -> screen 720x1600: everything doubles.
    private val geo = InputGeometry(360, 800, 720, 1600)
    private val input = InputEvents(ops, { geo }, { now })

    private fun run(json: String) = input.handle(JSONObject("""{"events":$json}"""))

    @Test fun tapMapsVideoToScreen() {
        run("""[{"type":"touch","action":"down","pointerId":0,"x":100,"y":200}]""")
        now += 80
        val r = run("""[{"type":"touch","action":"up","pointerId":0,"x":101,"y":200}]""")
        assertEquals(listOf("tap 200,400"), ops.actions)
        assertEquals(1, r.handled)
    }

    @Test fun longStillPressIsPress() {
        run("""[{"type":"touch","action":"down","pointerId":0,"x":10,"y":10}]""")
        now += 900
        run("""[{"type":"touch","action":"up","pointerId":0,"x":10,"y":10}]""")
        assertEquals(listOf("press 20,20 900ms"), ops.actions)
    }

    @Test fun travelIsSwipeWithClampedDuration() {
        run("""[{"type":"touch","action":"down","pointerId":0,"x":100,"y":600},
                {"type":"touch","action":"move","pointerId":0,"x":100,"y":300},
                {"type":"touch","action":"up","pointerId":0,"x":100,"y":200}]""")
        assertEquals(listOf("swipe 200,1200->200,400 50ms hold=0"), ops.actions)
    }

    @Test fun secondFingerAndUpWithoutDownAreIgnored() {
        val r = run("""[{"type":"touch","action":"down","pointerId":1,"x":1,"y":1},
                       {"type":"touch","action":"up","pointerId":0,"x":1,"y":1}]""")
        assertEquals(InputResult(0, 2), r)
        assertTrue(ops.actions.isEmpty())
    }

    @Test fun navAndKeys() {
        val r = run("""[{"type":"back"},{"type":"home"},{"type":"app_switch"},{"type":"power"},
                       {"type":"key","action":"down","keyCode":4},{"type":"key","action":"up","keyCode":4},
                       {"type":"key","action":"up","keyCode":187}]""")
        assertEquals(listOf("global BACK", "global HOME", "global RECENTS", "global BACK", "global RECENTS"), ops.actions)
        assertEquals(InputResult(5, 2), r) // power and key down ignored
    }

    @Test fun textGoesToFocusedField() {
        ops.focused = ops.nodes.first { it.editable }
        assertEquals(1, run("""[{"type":"text","text":"hi"}]""").handled)
        assertEquals("hi", ops.fieldText)
        ops.focused = null
        assertEquals(1, run("""[{"type":"text","text":"x"}]""").ignored)
    }

    @Test fun enterAndBackspace() {
        ops.focused = UiNode(0, "android.widget.EditText", "abc", null, null, "p", Bounds(0, 0, 1, 1), editable = true)
        run("""[{"type":"key","action":"up","keyCode":66},{"type":"key","action":"up","keyCode":67}]""")
        assertEquals(listOf("ime_enter", "set_text 'ab'"), ops.actions)
    }

    @Test fun scrollWheelUpMovesFingerDown() {
        run("""[{"type":"scroll","x":180,"y":400,"vScroll":1}]""")
        assertEquals(listOf("swipe 360,800->360,1040 200ms hold=0"), ops.actions)
    }

    @Test fun rejectsBadBatches() {
        for (bad in listOf("[]", """[{"type":"zap"}]""", """[{"type":"touch","action":"down","pointerId":0,"x":"a","y":1}]""",
            """[{"type":"text","text":${"\"" + "a".repeat(2001) + "\""}}]""", """[{"type":"key","action":"up"}]""", "5")) {
            try { run(bad); fail("accepted $bad") } catch (e: RpcException) { assertEquals(RpcException.INVALID_PARAMS, e.code) }
        }
        val many = (1..65).joinToString(",", "[", "]") { """{"type":"home"}""" }
        try { run(many); fail("accepted 65") } catch (e: RpcException) { assertEquals(RpcException.INVALID_PARAMS, e.code) }
        try { input.handle(JSONObject("{}")); fail() } catch (e: RpcException) { }
    }
}
