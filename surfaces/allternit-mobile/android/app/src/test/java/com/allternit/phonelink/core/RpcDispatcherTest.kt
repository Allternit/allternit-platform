package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class RpcDispatcherTest {
    private lateinit var ops: FakeDeviceOps
    private var opsAvailable = true
    private lateinit var hooks: FakeHooks
    private lateinit var auth: AuthManager
    private lateinit var d: RpcDispatcher
    private val slept = ArrayList<Long>()

    @Before fun setUp() {
        ops = FakeDeviceOps()
        hooks = FakeHooks()
        auth = AuthManager(MemorySessionStore())
        d = RpcDispatcher({ if (opsAvailable) ops else null }, { FakeDeviceOps.INFO }, auth, hooks, "com.allternit.phonelink", sleep = { slept.add(it) })
    }

    private fun call(method: String, params: JSONObject = JSONObject()) = d.dispatch(method, params)

    private fun expectType(type: String, block: () -> Unit) {
        try { block(); fail("expected $type") } catch (e: RpcException) { assertEquals(type, e.type) }
    }

    // ── JSON-RPC framing ────────────────────────────────────────────────────

    @Test fun handleBodyWrapsResultsAndErrors() {
        val ok = JSONObject(d.handleBody("""{"jsonrpc":"2.0","method":"is_locked","id":7}""", "lan"))
        assertEquals(7, ok.getInt("id"))
        assertFalse(ok.getJSONObject("result").getBoolean("locked"))

        val unknown = JSONObject(d.handleBody("""{"jsonrpc":"2.0","method":"nope","id":"a"}""", "lan"))
        assertEquals(-32601, unknown.getJSONObject("error").getInt("code"))
        assertEquals("a", unknown.getString("id"))

        assertEquals(-32700, JSONObject(d.handleBody("not json", "lan")).getJSONObject("error").getInt("code"))
        assertEquals(-32600, JSONObject(d.handleBody("""{"jsonrpc":"1.0","method":"x"}""", "lan")).getJSONObject("error").getInt("code"))
        assertEquals(-32602, JSONObject(d.handleBody("""{"jsonrpc":"2.0","method":"x","params":[1]}""", "lan")).getJSONObject("error").getInt("code"))
    }

    @Test fun typedErrorsCarryDataType() {
        ops.locked = true
        val r = JSONObject(d.handleBody("""{"jsonrpc":"2.0","method":"tap","params":{"x":1,"y":1},"id":1}""", "lan"))
        val err = r.getJSONObject("error")
        assertEquals(-32000, err.getInt("code"))
        assertEquals("screen_locked", err.getJSONObject("data").getString("type"))
    }

    // ── Safety ──────────────────────────────────────────────────────────────

    @Test fun refusesEverythingOnTheScreenWhileLockedOrOff() {
        ops.locked = true
        for (m in listOf("tap", "page_source", "screenshot", "launch_app", "home", "type")) {
            expectType("screen_locked") { call(m, JSONObject().put("x", 1).put("y", 1).put("bundle_id", "com.openai.chatgpt").put("text", "a")) }
        }
        assertTrue(ops.actions.isEmpty())
        ops.locked = false; ops.screenOn = false
        expectType("screen_off") { call("page_source") }
        // Non-UI calls still answer.
        assertEquals("pixels", call("device_info").getString("coordinate_space"))
    }

    @Test fun refusesToActOnProtectedScreens() {
        for (pkg in listOf("com.allternit.phonelink", "com.android.settings", "com.google.android.permissioncontroller")) {
            ops.foreground = pkg
            expectType("protected_screen") { call("tap", JSONObject().put("x", 10).put("y", 10)) }
            expectType("protected_screen") { call("type", JSONObject().put("text", "x")) }
        }
        assertTrue(ops.actions.isEmpty())
        // Reading and leaving are allowed.
        call("page_source")
        call("home")
        assertEquals(listOf("global HOME"), ops.actions)
    }

    @Test fun refusesToOpenProtectedApps() {
        ops.installed.add("com.android.settings")
        expectType("protected_app") { call("launch_app", JSONObject().put("bundle_id", "com.android.settings")) }
        ops.urlHandler = "com.android.settings"
        expectType("protected_app") { call("open_url", JSONObject().put("url", "https://example.com")) }
        expectType("protected_url") { call("open_url", JSONObject().put("url", "intent://x#Intent;end")) }
    }

    @Test fun accessibilityOffIsTyped() {
        opsAvailable = false
        expectType("accessibility_off") { call("page_source") }
        assertFalse(call("device_info").getJSONObject("system").getBoolean("accessibility_enabled"))
    }

    // ── Methods ─────────────────────────────────────────────────────────────

    @Test fun deviceInfoReportsPixelSpace() {
        val r = call("device_info")
        assertEquals(720.0, r.getDouble("point_width"), 0.0)
        assertEquals(1600.0, r.getDouble("pixel_height"), 0.0)
        assertEquals(1.0, r.getDouble("scale"), 0.0)
        assertEquals(1, r.getInt("protocol_version"))
        assertEquals("dev-1234", r.getString("device_id"))
        assertEquals("android", r.getString("platform"))
        assertEquals("com.openai.chatgpt", r.getJSONObject("system").getString("foreground_app"))
    }

    @Test fun pageSourceUsesTheIctrlLineShape() {
        val r = call("page_source")
        val src = r.getString("source")
        assertTrue(src, src.contains("  ImageButton, label: 'Send prompt', id: 'com.openai.chatgpt:id/send', {{610, 1400}, {90, 80}}, clickable"))
        assertTrue(src, src.contains("EditText, id: 'com.openai.chatgpt:id/composer', {{20, 1400}, {580, 80}}, hint: 'Message', clickable editable"))
        assertTrue(src.startsWith("Window(application)"))
        assertEquals("pixels", r.getString("coordinate_space"))
        assertFalse(r.has("nodes"))
    }

    @Test fun pageSourceFiltersAndVisibleOnly() {
        val filtered = call("page_source", JSONObject().put("filter", JSONObject().put("label", "hello"))).getString("source")
        assertTrue(filtered.contains("Hello there"))
        assertTrue("ancestors kept", filtered.contains("RecyclerView"))
        assertFalse(filtered.contains("Send prompt"))
        val visible = call("page_source", JSONObject().put("visible_only", true)).getString("source")
        assertFalse(visible.contains("Offscreen reply"))
        val structured = call("page_source", JSONObject().put("structured", true)).getJSONArray("nodes")
        assertEquals(FakeDeviceOps.defaultTree().size, structured.length())
        assertTrue(structured.getJSONObject(4).getString("ref").isNotEmpty())
        val capped = call("page_source", JSONObject().put("max_elements", 2))
        assertTrue(capped.getBoolean("truncated"))
    }

    @Test fun findElementThenTapByRef() {
        val f = call("find_element", JSONObject().put("element_id", "Send prompt"))
        assertTrue(f.getBoolean("exists"))
        assertEquals("button", f.getString("element_type"))
        assertEquals(655, f.getInt("center_x"))
        assertEquals("in_view", f.getString("visibility"))
        call("tap", JSONObject().put("ref", f.getString("ref")))
        assertEquals("tap 655,1440", ops.actions.last())
        expectType("stale_ref") { call("tap", JSONObject().put("ref", "e99.0.1x1")) }
    }

    @Test fun findElementMatchOrder() {
        assertEquals("id", call("find_element", JSONObject().put("element_id", "send")).getString("matched_by"))
        assertEquals("id", call("find_element", JSONObject().put("element_id", "com.openai.chatgpt:id/send")).getString("matched_by"))
        assertFalse(call("find_element", JSONObject().put("element_id", "Hello")).getBoolean("exists"))
        val c = call("find_element", JSONObject().put("element_id", "hello").put("match", "contains"))
        assertEquals("Hello there", c.getString("label"))
        val off = call("find_element", JSONObject().put("element_id", "Offscreen reply"))
        assertEquals("offscreen", off.getString("visibility"))
        assertFalse(off.getBoolean("hittable"))
    }

    @Test fun tapElementTapsInViewAndClicksOffscreen() {
        call("tap_element", JSONObject().put("element_id", "Send prompt"))
        assertEquals("tap 655,1440", ops.actions.last())
        val r = call("find_and_tap", JSONObject().put("element_id", "Offscreen reply"))
        assertEquals("accessibility_click", r.getString("method"))
        expectType("element_not_found") { call("tap_element", JSONObject().put("element_id", "Nope")) }
    }

    @Test fun tapValidatesCoordinates() {
        call("tap", JSONObject().put("x", 100).put("y", 200))
        assertEquals("tap 100,200", ops.actions.last())
        expectType("invalid_params") { call("tap", JSONObject().put("x", 800).put("y", 10)) }
        expectType("invalid_params") { call("tap", JSONObject().put("x", 10)) }
        assertTrue(call("tap", JSONObject().put("x", 1).put("y", 1)).getJSONObject("settle").getBoolean("settled"))
        assertFalse(call("tap", JSONObject().put("x", 1).put("y", 1).put("settle", false)).has("settle"))
    }

    @Test fun swipeDirectionsAndDrag() {
        call("swipe", JSONObject().put("direction", "up").put("distance", 400))
        assertEquals("swipe 360,800->360,400 350ms hold=0", ops.actions.last())
        call("swipe", JSONObject().put("direction", "left").put("velocity", "fast").put("x", 600).put("y", 500).put("distance", 300))
        assertEquals("swipe 600,500->300,500 120ms hold=0", ops.actions.last())
        call("drag", JSONObject().put("from_x", 10).put("from_y", 20).put("to_x", 30).put("to_y", 40).put("hold", 0.6))
        assertEquals("swipe 10,20->30,40 500ms hold=600", ops.actions.last())
        expectType("invalid_params") { call("swipe", JSONObject().put("direction", "diagonal")) }
    }

    @Test fun scrollPrefersTheListAction() {
        assertEquals("accessibility_action", call("scroll", JSONObject().put("direction", "down")).getString("method"))
        assertEquals("scroll_node forward=true", ops.actions.last())
    }

    @Test fun typeAppendsClearsAndFallsBackToPaste() {
        expectType("no_focus") { call("type", JSONObject().put("text", "hi")) }
        ops.focused = UiNode(0, "android.widget.EditText", "draft ", null, null, "com.openai.chatgpt", Bounds(0, 0, 10, 10), editable = true)
        assertEquals("set_text", call("type", JSONObject().put("text", "more")).getString("method"))
        assertEquals("draft more", ops.fieldText)
        call("type", JSONObject().put("text", "new").put("clear", true))
        assertEquals("new", ops.fieldText)
        // Hint text is not content.
        ops.focused = UiNode(0, "android.widget.EditText", "Message", null, null, "x.y", Bounds(0, 0, 10, 10), editable = true, hint = "Message")
        call("type", JSONObject().put("text", "Hi"))
        assertEquals("Hi", ops.fieldText)
        ops.setTextWorks = false
        ops.fieldText = ""
        val r = call("type", JSONObject().put("text", "pasted").put("submit", true))
        assertEquals("paste", r.getString("method"))
        assertEquals("pasted", ops.fieldText)
        assertTrue(r.getBoolean("submitted"))
    }

    @Test fun appsAndUrls() {
        val apps = call("list_apps").getJSONArray("apps")
        assertEquals(2, apps.length())
        assertEquals(3, call("list_apps", JSONObject().put("element_type", "all")).getInt("count"))
        assertEquals(1, call("list_apps", JSONObject().put("text", "claude")).getInt("count"))
        call("launch_app", JSONObject().put("bundle_id", "com.anthropic.claude"))
        assertEquals("launch com.anthropic.claude", ops.actions.last())
        expectType("app_not_installed") { call("launch_app", JSONObject().put("bundle_id", "com.x.notthere")) }
        expectType("invalid_params") { call("launch_app", JSONObject().put("bundle_id", "not a package")) }
        assertEquals("com.android.chrome", call("open_url", JSONObject().put("url", "https://chatgpt.com")).getString("handler"))
        ops.urlHandler = null
        expectType("no_handler") { call("open_url", JSONObject().put("url", "weird://x")) }
        assertTrue(call("terminate_app", JSONObject().put("bundle_id", "com.openai.chatgpt")).getBoolean("best_effort"))
    }

    @Test fun screenshotReportsScaleAgainstPixels() {
        val r = call("screenshot", JSONObject().put("scale", 2.0))
        assertEquals(1.0, r.getDouble("scale"), 0.0) // clamped: never above native
        assertEquals("jpeg", r.getString("format"))
        val half = call("screenshot", JSONObject().put("scale", 0.5).put("format", "png"))
        assertEquals(0.5, half.getDouble("scale"), 0.0)
        assertEquals(360.0, half.getDouble("pixel_width"), 0.0)
        assertEquals("png", half.getString("format"))
    }

    @Test fun screenTextComesFromTheTree() {
        val r = call("screen_text")
        assertEquals("tree", r.getString("source"))
        val texts = (0 until r.getJSONArray("items").length()).map { r.getJSONArray("items").getJSONObject(it).getString("text") }
        assertTrue(texts.contains("Hello there"))
        assertFalse(texts.contains("Offscreen reply"))
        assertEquals(1, call("screen_text", JSONObject().put("element_id", "chatgpt")).getInt("count"))
    }

    @Test fun waitIsBounded() {
        call("wait", JSONObject().put("duration", 999))
        assertEquals(30_000L, slept.last())
    }

    @Test fun configListsRevokesAndStops() {
        val s = (auth.apply { approver = ScriptedApprover(true) }.requestAuth("Desk", false, null) as PairOutcome.Approved).session
        var r = call("config")
        assertEquals(1, r.getJSONArray("sessions").length())
        assertFalse("never leaks the key", r.toString().contains(s.keyHex))
        r = call("config", JSONObject().put("allow_remote_access", false).put("listen_port", 8300).put("device_name", "Moto"))
        assertFalse(r.getBoolean("allow_remote_access"))
        assertEquals(8300, r.getInt("listen_port"))
        assertEquals("Moto", hooks.cfg.deviceName)
        r = call("config", JSONObject().put("revoke_session_id", s.id))
        assertEquals(0, r.getJSONArray("sessions").length())
        expectType("invalid_params") { call("config", JSONObject().put("listen_port", 80)) }
        expectType("unsupported") { call("config", JSONObject().put("nearby_discovery", true)) }
        assertTrue(call("config", JSONObject().put("stop_server", true)).getBoolean("stopping"))
        assertTrue(hooks.stopScheduled)
    }

    @Test fun schemaListsEveryDispatchedMethod() {
        val names = Schema.METHODS.map { it.name }.toSet()
        for (m in RpcDispatcher.UI_METHODS) assertTrue(m, m in names)
        for (m in listOf("device_info", "config", "is_locked", "list_apps", "terminate_app", "wait", "screen_service")) assertTrue(m, m in names)
    }

    @Test fun screenServiceReportsStateAndTheLaunchLines() {
        val down = call("screen_service")
        assertEquals(false, down.getBoolean("running"))
        assertEquals("waiting_for_computer", down.getString("state"))
        assertEquals("Waiting", down.getString("lastError"))
        assertEquals("k", down.getJSONObject("launch").getString("kill"))
        assertEquals("s", down.getJSONObject("launch").getString("start"))
        hooks.screen = ScreenServiceStatus(true, "running", null, null)
        val up = call("screen_service")
        assertEquals(true, up.getBoolean("running"))
        assertTrue(up.isNull("lastError"))
        assertTrue(up.isNull("launch"))
    }

    // ── mesh_restart ────────────────────────────────────────────────────────

    /** A dispatcher whose sleep advances its clock, so wait loops run instantly. */
    private fun meshDispatcher(onSleep: (Long) -> Unit): RpcDispatcher {
        var now = 0L
        return RpcDispatcher({ ops }, { FakeDeviceOps.INFO }, auth, hooks, "com.allternit.phonelink",
            sleep = { now += it; onSleep(now) }, clock = { now })
    }

    @Test fun meshRestartWaitsForTheRejoin() {
        hooks.cfg = hooks.cfg.copy(meshState = "running", meshHost = "100.64.0.16:8200")
        hooks.onMeshRestart = { hooks.cfg = hooks.cfg.copy(meshState = "starting", meshHost = null) }
        val md = meshDispatcher { now -> if (now >= 3_000) hooks.cfg = hooks.cfg.copy(meshState = "running", meshHost = "100.64.0.16:8200") }
        val r = md.dispatch("mesh_restart", JSONObject())
        assertEquals(listOf(false), hooks.meshRestarts)
        assertTrue(r.getBoolean("restarted"))
        assertFalse(r.getBoolean("deferred"))
        assertEquals("running", r.getString("mesh_state"))
        assertEquals("100.64.0.16:8200", r.getString("mesh_host"))
        assertEquals("100.64.0.16:8200", r.getString("previous_mesh_host"))
        assertEquals(3_000L, r.getLong("waited_ms"))
    }

    @Test fun meshRestartGivesUpAfterWaitAndReportsTheState() {
        hooks.onMeshRestart = { hooks.cfg = hooks.cfg.copy(meshState = "starting", meshHost = null) }
        val r = meshDispatcher {}.dispatch("mesh_restart", JSONObject().put("wait", 5))
        assertEquals("starting", r.getString("mesh_state"))
        assertTrue(r.isNull("mesh_host"))
        assertEquals(5_000L, r.getLong("waited_ms"))
    }

    @Test fun meshRestartWithWaitZeroIsDeferredForMeshCallers() {
        val r = meshDispatcher { fail("must not wait") }.dispatch("mesh_restart", JSONObject().put("wait", 0))
        assertEquals(listOf(true), hooks.meshRestarts)
        assertTrue(r.getBoolean("deferred"))
    }

    @Test fun meshRestartWithoutAMeshIsATypedError() {
        hooks.meshAvailable = false
        expectType("mesh_unavailable") { call("mesh_restart") }
    }

    @Test fun meshRestartIsInTheSchema() {
        assertTrue(Schema.METHODS.any { it.name == "mesh_restart" })
        assertTrue("mesh_restart" in Schema.CAPABILITY_TAGS)
    }
}
