package com.allternit.phonelink.core

import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject
import java.util.Base64

data class ServerConfig(
    val deviceId: String,
    val deviceName: String,
    val allowRemote: Boolean,
    val port: Int,
    /** Mesh node lifecycle state (MeshController.State in Go: stopped/starting/running/…). */
    val meshState: String = "stopped",
    /** `<meshIP>:8200` while joined, else null. */
    val meshHost: String? = null,
    /**
     * The shell server that serves the live screen (ShellServerLauncher.State,
     * lower-case): not_set_up / starting / running / waiting_for_wifi / error.
     */
    val screenState: String = "not_set_up",
)

/** Server-level operations `config` can trigger. Changes apply after the reply is sent. */
interface ServerHooks {
    fun config(): ServerConfig
    fun applyConfig(allowRemote: Boolean?, port: Int?, deviceName: String?)
    fun scheduleStop()
}

/**
 * JSON-RPC 2.0 dispatch with iCTRL's method names, params and result shapes
 * where they apply. Pure Kotlin: the device is behind [DeviceOps].
 */
class RpcDispatcher(
    private val ops: () -> DeviceOps?,
    private val info: () -> StaticDeviceInfo,
    private val auth: AuthManager,
    private val hooks: ServerHooks,
    ownPackage: String,
    private val sleep: (Long) -> Unit = { Thread.sleep(it) },
    private val clock: () -> Long = System::currentTimeMillis,
) {
    /** Never act here: this app's own screens, Settings, permission and install prompts. */
    val protectedPackages: Set<String> = setOf(
        ownPackage,
        "com.android.settings",
        "com.android.permissioncontroller",
        "com.google.android.permissioncontroller",
        "com.android.packageinstaller",
        "com.google.android.packageinstaller",
    )

    private val uiLock = Any()
    private var refGen = 0L
    private val refs = object : LinkedHashMap<String, UiNode>(256, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, UiNode>?) = size > 1024
    }

    companion object {
        /** Methods that read or change the screen: locked/off checks + serialized. */
        val UI_METHODS = setOf(
            "page_source", "screen_text", "screenshot", "tap", "double_tap", "long_press", "swipe", "drag",
            "scroll", "type", "clear_text", "find_element", "tap_element", "find_and_tap", "home", "back",
            "launch_app", "activate_app", "open_url",
        )
        /** Methods that touch or type into whatever is in front: refused on protected screens. */
        val ACTING_METHODS = setOf(
            "tap", "double_tap", "long_press", "swipe", "drag", "scroll", "type", "clear_text",
            "tap_element", "find_and_tap",
        )
        val PACKAGE_RE = Regex("^[A-Za-z][A-Za-z0-9_]*(\\.[A-Za-z0-9_]+)+$")
    }

    // ── JSON-RPC envelope ───────────────────────────────────────────────────

    /** Takes the decrypted request body, returns the JSON-RPC response text. */
    fun handleBody(plaintext: String, peerSource: String): String {
        val req = try { JSONObject(plaintext) } catch (e: JSONException) {
            return errorResponse(null, -32700, "Parse error", null)
        }
        val id = if (req.has("id")) req.opt("id") else null
        if (req.optString("jsonrpc") != "2.0" || req.opt("method") !is String) {
            return errorResponse(id, -32600, "Invalid Request", null)
        }
        val method = req.getString("method")
        val params = when (val p = req.opt("params")) {
            null, JSONObject.NULL -> JSONObject()
            is JSONObject -> p
            else -> return errorResponse(id, -32602, "Invalid params", null)
        }
        return try {
            val result = dispatch(method, params, peerSource)
            JSONObject().put("jsonrpc", "2.0").put("result", result).put("id", id ?: JSONObject.NULL).toString()
        } catch (e: RpcException) {
            val data = (e.extra?.let { JSONObject(it.toString()) } ?: JSONObject())
            if (e.type != null) data.put("type", e.type)
            errorResponse(id, e.code, e.message ?: "error", data.takeIf { it.length() > 0 })
        } catch (e: Exception) {
            errorResponse(id, RpcException.SERVER_ERROR, "${method} failed: ${e.javaClass.simpleName}: ${e.message}", JSONObject().put("type", "internal"))
        }
    }

    private fun errorResponse(id: Any?, code: Int, message: String, data: JSONObject?): String {
        val err = JSONObject().put("code", code).put("message", message)
        if (data != null) err.put("data", data)
        return JSONObject().put("jsonrpc", "2.0").put("error", err).put("id", id ?: JSONObject.NULL).toString()
    }

    // ── Dispatch ────────────────────────────────────────────────────────────

    fun dispatch(method: String, params: JSONObject, peerSource: String = "local"): JSONObject {
        if (method in UI_METHODS) {
            synchronized(uiLock) {
                val o = requireOps()
                requireUsableScreen(o)
                if (method in ACTING_METHODS) requireNotProtected(o)
                return dispatchUi(method, params, o)
            }
        }
        return when (method) {
            "device_info" -> deviceInfo()
            "config" -> config(params, peerSource)
            "is_locked" -> requireOps().let { JSONObject().put("locked", it.isScreenLocked()).put("screen_on", it.isScreenOn()) }
            "list_apps" -> listApps(params)
            "terminate_app" -> terminateApp(params)
            "wait" -> {
                val d = (params.optDoubleOrNull("duration") ?: 1.0).coerceIn(0.0, 30.0)
                sleep((d * 1000).toLong())
                JSONObject().put("message", "Waited ${d}s")
            }
            "stream_ticket" -> throw RpcException(RpcException.METHOD_NOT_FOUND, "Method not found: the Android server has no /stream", "unsupported")
            else -> throw RpcException(RpcException.METHOD_NOT_FOUND, "Method not found", "unknown_method")
        }
    }

    private fun dispatchUi(method: String, p: JSONObject, o: DeviceOps): JSONObject = when (method) {
        "page_source" -> pageSource(p, o)
        "screen_text" -> screenText(p, o)
        "screenshot" -> screenshot(p, o)
        "tap" -> withSettle(p, o) { val (x, y) = point(p, o, "x", "y", "ref"); ensure(o.tap(x, y), "tap"); JSONObject().put("message", "Tapped at (${x.toInt()}, ${y.toInt()})").put("x", x.toInt()).put("y", y.toInt()) }
        "double_tap" -> withSettle(p, o) { val (x, y) = point(p, o, "x", "y", "ref"); ensure(o.doubleTap(x, y), "double_tap"); JSONObject().put("message", "Double-tapped at (${x.toInt()}, ${y.toInt()})") }
        "long_press" -> withSettle(p, o) {
            val (x, y) = point(p, o, "x", "y", "ref")
            val d = (p.optDoubleOrNull("duration") ?: 1.0).coerceIn(0.1, 10.0)
            ensure(o.press(x, y, (d * 1000).toLong()), "long_press")
            JSONObject().put("message", "Long-pressed at (${x.toInt()}, ${y.toInt()}) for ${d}s")
        }
        "swipe" -> withSettle(p, o) { swipe(p, o) }
        "drag" -> withSettle(p, o) { drag(p, o) }
        "scroll" -> withSettle(p, o) { scroll(p, o) }
        "type" -> typeText(p, o)
        "clear_text" -> clearText(o)
        "find_element" -> findElement(p, o)
        "tap_element", "find_and_tap" -> withSettle(p, o) { tapElement(p, o) }
        "home" -> global(o, GlobalAction.HOME, "Home")
        "back" -> global(o, GlobalAction.BACK, "Back")
        "launch_app", "activate_app" -> launch(p, o, method)
        "open_url" -> openUrl(p, o)
        else -> throw RpcException(RpcException.METHOD_NOT_FOUND, "Method not found", "unknown_method")
    }

    // ── Guards ──────────────────────────────────────────────────────────────

    private fun requireOps(): DeviceOps = ops() ?: throw RpcException.typed(
        "accessibility_off",
        "Phone Link's accessibility service is not running. Re-run setup from Allternit Desktop, " +
            "or turn it on in Settings › Accessibility › Allternit Phone Link.",
    )

    private fun requireUsableScreen(o: DeviceOps) {
        if (!o.isScreenOn()) throw RpcException.typed("screen_off", "The phone's screen is off. Wake and unlock it, then retry.")
        if (o.isScreenLocked()) throw RpcException.typed("screen_locked", "The phone is locked. Phone Link never acts on the lock screen; unlock it by hand, then retry.")
    }

    private fun requireNotProtected(o: DeviceOps) {
        val fg = o.foregroundPackage()
        if (fg != null && fg in protectedPackages) {
            throw RpcException.typed(
                "protected_screen",
                "Refusing to act on $fg (Settings, permission prompts and Phone Link itself are off limits). Go home or launch another app first.",
                JSONObject().put("foreground_app", fg),
            )
        }
    }

    private fun ensure(ok: Boolean, what: String) {
        if (!ok) throw RpcException.typed("gesture_failed", "$what was not performed (the system cancelled the gesture).")
    }

    private fun screen(): Bounds { val i = info(); return Bounds(0, 0, i.widthPx, i.heightPx) }

    private fun mintRef(n: UiNode): String {
        val ref = "e${refGen}.${refs.size}.${n.bounds.centerX}x${n.bounds.centerY}"
        refs[ref] = n
        return ref
    }

    private fun resolveRef(ref: String): UiNode = refs[ref]
        ?: throw RpcException.typed("stale_ref", "Unknown or expired ref '$ref'. Call find_element again.")

    /** (x, y) from a ref or explicit pixel coordinates, validated against the screen. */
    private fun point(p: JSONObject, o: DeviceOps, xKey: String, yKey: String, refKey: String): Pair<Float, Float> {
        p.optStringOrNull(refKey)?.let { ref ->
            val n = resolveRef(ref)
            return n.bounds.centerX.toFloat() to n.bounds.centerY.toFloat()
        }
        val x = p.optDoubleOrNull(xKey)
        val y = p.optDoubleOrNull(yKey)
        if (x == null || y == null) throw RpcException.invalid("Provide $xKey and $yKey in pixels (see device_info) or $refKey")
        val s = screen()
        if (x < 0 || y < 0 || x >= s.width || y >= s.height) {
            throw RpcException.invalid("($x, $y) is outside the screen (${s.width}×${s.height} pixels)")
        }
        return x.toFloat() to y.toFloat()
    }

    private fun withSettle(p: JSONObject, o: DeviceOps, action: () -> JSONObject): JSONObject {
        val result = action()
        if (p.optBoolOrNull("settle") != false) {
            val timeout = ((p.optDoubleOrNull("settle_timeout") ?: 2.0).coerceIn(0.0, 3.0) * 1000).toLong()
            val t0 = clock()
            val settled = o.waitForIdle(300, timeout)
            result.put("settle", JSONObject().put("settled", settled).put("waited_ms", clock() - t0))
        }
        return result
    }

    // ── Methods ─────────────────────────────────────────────────────────────

    private fun deviceInfo(): JSONObject {
        val i = info()
        val cfg = hooks.config()
        val o = ops()
        val system = JSONObject().put("accessibility_enabled", o != null)
        if (o != null) {
            val m = o.systemMetrics()
            for (k in m.keys()) system.put(k, m.get(k))
            system.put("screen_locked", o.isScreenLocked()).put("screen_on", o.isScreenOn())
                .put("foreground_app", o.foregroundPackage() ?: JSONObject.NULL)
        }
        return JSONObject()
            .put("device_name", cfg.deviceName)
            .put("model", "${i.manufacturer} ${i.model}".trim())
            .put("manufacturer", i.manufacturer)
            .put("system_version", i.androidRelease)
            .put("sdk_int", i.sdkInt)
            .put("platform", Schema.PLATFORM)
            .put("server_version", Schema.SERVER_VERSION)
            .put("device_id", cfg.deviceId)
            .put("protocol_version", Schema.PROTOCOL_VERSION)
            .put("capabilities", JSONArray(Schema.CAPABILITY_TAGS))
            .put("point_width", i.widthPx.toDouble())
            .put("point_height", i.heightPx.toDouble())
            .put("scale", 1.0)
            .put("pixel_width", i.widthPx.toDouble())
            .put("pixel_height", i.heightPx.toDouble())
            .put("screen_width", i.widthPx.toDouble())
            .put("screen_height", i.heightPx.toDouble())
            .put("screen_scale", 1.0)
            .put("density", i.density.toDouble())
            .put("density_dpi", i.densityDpi)
            .put("coordinate_space", "pixels")
            .put("hint", "All tap/swipe/drag/long_press coordinates are in PIXELS of the full screen, " +
                "x∈[0,${i.widthPx}) y∈[0,${i.heightPx}) (status and navigation bars included). " +
                "page_source frames use the same space. A screenshot at scale s is (width·s × height·s); " +
                "tap at pixel/s. Rotation changes width/height: re-read device_info after it.")
            .put("mesh_state", cfg.meshState)
            .put("screen_state", cfg.screenState)
            .put("mesh_host", cfg.meshHost ?: JSONObject.NULL)
            .put("system", system)
    }

    private fun pageSource(p: JSONObject, o: DeviceOps): JSONObject {
        val all = o.snapshot()
        if (all.isEmpty()) throw RpcException.typed("no_window", "No window content is readable right now (the app may still be launching). Retry in a moment.")
        val maxElements = (p.optDoubleOrNull("max_elements")?.toInt() ?: 2000).coerceIn(1, 4000)
        val maxDepth = p.optDoubleOrNull("max_depth")?.toInt()?.coerceAtLeast(0) ?: Int.MAX_VALUE
        val filter = UiTree.Filter.from(p.optJSONObject("filter"))
        val visibleOnly = p.optBoolOrNull("visible_only") == true
        val sel = UiTree.select(all, screen(), filter, maxDepth, visibleOnly, maxElements)
        var hint = "Accessibility tree (class, label = content-desc or text, value, id = resource-id, " +
            "frame {{x, y}, {w, h}} in PIXELS, flags). Tap an element at its frame centre, or use find_element for a ref."
        if (sel.truncated) hint = "Tree truncated at ${sel.nodes.size} nodes: pass visible_only:true or a filter. $hint"
        val out = JSONObject()
            .put("format", "text")
            .put("source", UiTree.render(sel.nodes))
            .put("truncated", sel.truncated)
            .put("total_returned", sel.nodes.size)
            .put("coordinate_space", "pixels")
            .put("foreground_app", o.foregroundPackage() ?: JSONObject.NULL)
            .put("source_channel", "ax")
            .put("hint", hint)
        if (p.optBoolOrNull("structured") == true || p.optStringOrNull("format") == "json") {
            refGen++
            val arr = JSONArray()
            for (n in sel.nodes) arr.put(UiTree.nodeJson(n, mintRef(n)))
            out.put("nodes", arr)
        }
        return out
    }

    private fun textItems(o: DeviceOps): List<UiNode> {
        val s = screen()
        val seen = HashSet<String>()
        return o.snapshot().filter { n ->
            n.windowType == null && n.visibleToUser && n.bounds.intersects(s) && n.label.isNotBlank() &&
                seen.add("${n.label}|${n.bounds}")
        }
    }

    private fun screenText(p: JSONObject, o: DeviceOps): JSONObject {
        val needle = p.optStringOrNull("element_id")?.lowercase()
        val items = JSONArray()
        for (n in textItems(o)) {
            val t = n.value?.let { if (it != n.label) "${n.label} — $it" else it } ?: n.label
            if (needle != null && !t.lowercase().contains(needle)) continue
            val b = n.bounds
            items.put(JSONObject().put("text", t).put("x", b.left).put("y", b.top).put("width", b.width).put("height", b.height)
                .put("center_x", b.centerX).put("center_y", b.centerY).put("confidence", 1.0))
        }
        val out = JSONObject().put("count", items.length()).put("items", items).put("source", "tree")
            .put("coordinate_space", "pixels")
            .put("hint", "Read from the accessibility tree, not OCR: text drawn inside images or canvases is not included. " +
                "Pass include_image:true to also get a screenshot (API 30+).")
        if (p.optBoolOrNull("include_image") == true) {
            try {
                val shot = o.screenshot("jpeg", 0.6f, 1f)
                out.put("image", JSONObject().put("format", shot.format).put("encoding", "base64")
                    .put("data", Base64.getEncoder().encodeToString(shot.bytes)))
            } catch (e: RpcException) {
                out.put("image_error", e.message)
            }
        }
        return out
    }

    private fun screenshot(p: JSONObject, o: DeviceOps): JSONObject {
        val format = if (p.optStringOrNull("format")?.lowercase() == "png") "png" else "jpeg"
        val quality = (p.optDoubleOrNull("quality") ?: 0.6).coerceIn(0.1, 1.0).toFloat()
        val requested = p.optDoubleOrNull("scale") ?: 1.0
        val scale = requested.coerceIn(0.1, 1.0).toFloat()
        val shot = o.screenshot(format, quality, scale)
        val i = info()
        val effective = if (i.widthPx > 0) shot.pixelWidth.toDouble() / i.widthPx else scale.toDouble()
        return JSONObject()
            .put("format", shot.format)
            .put("encoding", "base64")
            .put("data", Base64.getEncoder().encodeToString(shot.bytes))
            .put("bytes", shot.bytes.size.toDouble())
            .put("pixel_width", shot.pixelWidth.toDouble())
            .put("pixel_height", shot.pixelHeight.toDouble())
            .put("point_width", i.widthPx.toDouble())
            .put("point_height", i.heightPx.toDouble())
            .put("scale", Math.round(effective * 1000) / 1000.0)
            .put("requested_scale", requested)
            .put("device_scale", 1.0)
            .put("coordinate_space", "pixels")
    }

    private fun velocityMs(v: String?): Long = when (v?.lowercase()) {
        "fast", "flick" -> 120
        "slow", "precise" -> 900
        else -> 350
    }

    private fun swipe(p: JSONObject, o: DeviceOps): JSONObject {
        val s = screen()
        val x = (p.optDoubleOrNull("x") ?: (s.width / 2.0)).toFloat()
        val y = (p.optDoubleOrNull("y") ?: (s.height / 2.0)).toFloat()
        val dir = (p.optStringOrNull("direction") ?: "up").lowercase()
        val dist = (p.optDoubleOrNull("distance") ?: (s.height / 3.0)).toFloat()
        val (x2, y2) = when (dir) {
            "up" -> x to y - dist
            "down" -> x to y + dist
            "left" -> x - dist to y
            "right" -> x + dist to y
            else -> throw RpcException.invalid("direction must be up/down/left/right")
        }
        val cx2 = x2.coerceIn(0f, (s.width - 1).toFloat())
        val cy2 = y2.coerceIn(0f, (s.height - 1).toFloat())
        val slow = p.optStringOrNull("velocity")?.lowercase() in setOf("slow", "precise")
        // A slow swipe ends with a short hold so the list stops where the finger does (no fling).
        ensure(o.swipe(x, y, cx2, cy2, velocityMs(p.optStringOrNull("velocity")), 0), "swipe")
        if (slow) sleep(150)
        return JSONObject().put("message", "Swiped $dir from (${x.toInt()}, ${y.toInt()}) to (${cx2.toInt()}, ${cy2.toInt()})")
    }

    private fun drag(p: JSONObject, o: DeviceOps): JSONObject {
        val (x1, y1) = point(p, o, "from_x", "from_y", "from_ref")
        val (x2, y2) = point(p, o, "to_x", "to_y", "to_ref")
        val d = (p.optDoubleOrNull("duration") ?: 0.5).coerceIn(0.05, 10.0)
        val hold = (p.optDoubleOrNull("hold") ?: 0.0).coerceIn(0.0, 5.0)
        ensure(o.swipe(x1, y1, x2, y2, (d * 1000).toLong(), (hold * 1000).toLong()), "drag")
        return JSONObject().put("message", "Dragged from (${x1.toInt()}, ${y1.toInt()}) to (${x2.toInt()}, ${y2.toInt()})")
            .put("screen", JSONObject().put("width", screen().width).put("height", screen().height))
    }

    private fun scroll(p: JSONObject, o: DeviceOps): JSONObject {
        val dir = (p.optStringOrNull("direction") ?: "down").lowercase()
        if (dir !in setOf("up", "down", "left", "right")) throw RpcException.invalid("direction must be up/down/left/right")
        val forward = dir == "down" || dir == "right"
        val target = UiTree.mainScrollable(o.snapshot(), screen())
        if (target != null && o.scrollNode(target, forward)) {
            return JSONObject().put("message", "Scrolled $dir").put("method", "accessibility_action")
        }
        val s = screen()
        val cx = s.width / 2f
        val cy = s.height / 2f
        val dy = s.height / 3f
        val dx = s.width / 3f
        val ok = when (dir) {
            "down" -> o.swipe(cx, cy + dy / 2, cx, cy - dy / 2, 350)
            "up" -> o.swipe(cx, cy - dy / 2, cx, cy + dy / 2, 350)
            "right" -> o.swipe(cx + dx / 2, cy, cx - dx / 2, cy, 350)
            else -> o.swipe(cx - dx / 2, cy, cx + dx / 2, cy, 350)
        }
        ensure(ok, "scroll")
        return JSONObject().put("message", "Scrolled $dir").put("method", "swipe")
    }

    private fun focused(o: DeviceOps): UiNode = o.focusedInput() ?: throw RpcException.typed(
        "no_focus", "No text field has focus. Tap a text field first (or pass x,y to type).",
    )

    private fun typeText(p: JSONObject, o: DeviceOps): JSONObject {
        val text = p.optStringOrNull("text") ?: ""
        val clear = p.optBoolOrNull("clear") == true
        if (text.isEmpty() && !clear) throw RpcException.invalid("text is required (or clear:true)")
        val fx = p.optDoubleOrNull("x")
        val fy = p.optDoubleOrNull("y")
        if (fx != null && fy != null) {
            val (x, y) = point(p, o, "x", "y", "_none")
            ensure(o.tap(x, y), "focus tap")
            o.waitForIdle(250, 1500)
        }
        requireNotProtected(o)
        val field = focused(o)
        val current = if (field.editable && field.hint != null && field.text == field.hint) "" else field.text.orEmpty()
        val wanted = if (clear) text else current + text
        var method = "set_text"
        var warning: String? = null
        if (!o.setText(field, wanted)) {
            method = "paste"
            if (clear && !o.setText(field, "")) warning = "Could not clear the field before pasting."
            if (text.isNotEmpty() && !o.paste(field, text)) {
                throw RpcException.typed("type_failed", "The focused field accepted neither ACTION_SET_TEXT nor a paste.")
            }
        }
        val out = JSONObject().put("message", "Typed ${text.length} characters").put("method", method)
        if (p.optBoolOrNull("submit") == true) {
            val fresh = o.focusedInput() ?: field
            if (!o.imeEnter(fresh)) {
                warning = listOfNotNull(warning, "submit needs Android 11+ (ACTION_IME_ENTER) or the field has no IME action; tap the Send/Search button instead.").joinToString(" ")
            } else out.put("submitted", true)
        }
        if (warning != null) out.put("warning", warning)
        return out
    }

    private fun clearText(o: DeviceOps): JSONObject {
        val field = focused(o)
        if (!o.setText(field, "")) throw RpcException.typed("type_failed", "The focused field refused ACTION_SET_TEXT.")
        return JSONObject().put("message", "Cleared the focused field")
    }

    private fun matchMode(p: JSONObject): Boolean = when (val m = p.optStringOrNull("match")?.lowercase()) {
        null, "exact" -> false
        "contains" -> true
        else -> throw RpcException.invalid("match must be 'exact' or 'contains', got '$m'")
    }

    private fun findElement(p: JSONObject, o: DeviceOps): JSONObject {
        val id = p.optStringOrNull("element_id")?.takeIf { it.isNotEmpty() } ?: throw RpcException.invalid("element_id is required")
        val s = screen()
        val matches = UiTree.find(o.snapshot(), id, matchMode(p), p.optStringOrNull("element_type"), s)
        if (matches.isEmpty()) {
            return JSONObject().put("exists", false).put("element_id", id)
                .put("message", "No element matches '$id'")
                .put("hint", "Check page_source for the exact label, or pass match:'contains'. If it is below the fold, swipe and retry.")
        }
        refGen++
        val best = matches.first().node
        val b = best.bounds
        val vis = UiTree.visibility(best, s)
        val out = JSONObject()
            .put("exists", true)
            .put("hittable", vis != "offscreen" && best.enabled)
            .put("ref", mintRef(best))
            .put("element_type", UiTree.elementType(best))
            .put("class", best.className)
            .put("is_text_input", best.editable)
            .put("x", b.left).put("y", b.top).put("width", b.width).put("height", b.height)
            .put("center_x", b.centerX).put("center_y", b.centerY)
            .put("label", best.label)
            .put("value", best.value ?: JSONObject.NULL)
            .put("resource_id", best.resourceId ?: JSONObject.NULL)
            .put("visibility", vis)
            .put("matched_by", matches.first().how)
            .put("match_count", matches.size)
        if (matches.size > 1) {
            val alts = JSONArray()
            for (m in matches.drop(1).take(5)) {
                alts.put(JSONObject().put("ref", mintRef(m.node)).put("label", m.node.label)
                    .put("center_x", m.node.bounds.centerX).put("center_y", m.node.bounds.centerY)
                    .put("visibility", UiTree.visibility(m.node, s)))
            }
            out.put("alternatives", alts)
        }
        return out
    }

    private fun tapElement(p: JSONObject, o: DeviceOps): JSONObject {
        val id = p.optStringOrNull("element_id")?.takeIf { it.isNotEmpty() } ?: throw RpcException.invalid("element_id is required")
        val s = screen()
        val m = UiTree.find(o.snapshot(), id, matchMode(p), p.optStringOrNull("element_type"), s).firstOrNull()
            ?: throw RpcException.typed("element_not_found", "No element matches '$id'", JSONObject().put("element_id", id))
        val n = m.node
        val b = n.bounds
        val inView = UiTree.visibility(n, s) != "offscreen"
        val method = when {
            inView && o.tap(b.centerX.toFloat(), b.centerY.toFloat()) -> "tap"
            o.click(n) -> "accessibility_click"
            else -> throw RpcException.typed("gesture_failed", "Could not tap '$id'")
        }
        return JSONObject().put("message", "Tapped '${n.label.ifEmpty { id }}'").put("matched", n.label.ifEmpty { n.resourceId ?: id })
            .put("method", method)
            .put("frame", JSONObject().put("x", b.left).put("y", b.top).put("w", b.width).put("h", b.height))
    }

    private fun global(o: DeviceOps, action: GlobalAction, name: String): JSONObject {
        val before = o.foregroundPackage()
        if (!o.globalAction(action)) throw RpcException.typed("gesture_failed", "$name was not performed")
        o.waitForIdle(300, 2000)
        val after = o.foregroundPackage()
        return JSONObject().put("message", "Pressed $name").put("foreground_app", after ?: JSONObject.NULL)
            .put("previous_app", before ?: JSONObject.NULL)
    }

    private fun packageParam(p: JSONObject): String {
        val pkg = p.optStringOrNull("bundle_id") ?: p.optStringOrNull("package")
            ?: throw RpcException.invalid("bundle_id (the Android package name) is required")
        if (!PACKAGE_RE.matches(pkg)) throw RpcException.invalid("'$pkg' is not an Android package name (e.g. com.openai.chatgpt)")
        return pkg
    }

    private fun launch(p: JSONObject, o: DeviceOps, method: String): JSONObject {
        val pkg = packageParam(p)
        if (pkg in protectedPackages) throw RpcException.typed("protected_app", "Phone Link does not open $pkg.")
        if (!o.isInstalled(pkg)) throw RpcException.typed("app_not_installed", "$pkg is not installed (or has no launcher activity).", JSONObject().put("bundle_id", pkg))
        if (!o.launchApp(pkg)) throw RpcException.typed("launch_failed", "Android refused to start $pkg.")
        return JSONObject().put("message", if (method == "activate_app") "Activated $pkg" else "Launched $pkg")
            .put("hint", "The app may still be drawing its first screen: read page_source now and again after ~0.5 s if you see a splash.")
    }

    private fun terminateApp(p: JSONObject): JSONObject {
        val pkg = packageParam(p)
        val o = requireOps()
        if (pkg in protectedPackages) throw RpcException.typed("protected_app", "Phone Link does not stop $pkg.")
        val what = synchronized(uiLock) { o.terminateApp(pkg) }
        return JSONObject().put("message", what).put("best_effort", true)
    }

    private fun listApps(p: JSONObject): JSONObject {
        val o = requireOps()
        val needle = p.optStringOrNull("text")?.lowercase()
        val all = p.optStringOrNull("element_type") == "all"
        val arr = JSONArray()
        for (a in o.listApps().sortedBy { it.label.lowercase() }) {
            if (!all && a.system) continue
            if (needle != null && !a.label.lowercase().contains(needle) && !a.packageName.lowercase().contains(needle)) continue
            arr.put(JSONObject().put("bundle_id", a.packageName).put("package", a.packageName).put("name", a.label).put("system", a.system))
        }
        return JSONObject().put("count", arr.length()).put("source", "system").put("apps", arr)
            .put("hint", "Launchable apps from the package manager. bundle_id is the Android package name; pass it to launch_app.")
    }

    private fun openUrl(p: JSONObject, o: DeviceOps): JSONObject {
        val url = p.optStringOrNull("url")?.trim()?.takeIf { it.contains(":") } ?: throw RpcException.invalid("url (scheme:…) is required")
        val scheme = url.substringBefore(':').lowercase()
        if (scheme in setOf("intent", "file", "content", "package", "android-app")) {
            throw RpcException.typed("protected_url", "open_url does not accept $scheme: URLs.")
        }
        val handler = o.resolveUrl(url) ?: throw RpcException.typed("no_handler", "No installed app opens $url")
        if (handler in protectedPackages) throw RpcException.typed("protected_app", "That URL opens $handler, which Phone Link does not drive.")
        if (!o.openUrl(url)) throw RpcException.typed("launch_failed", "Android refused to open $url")
        return JSONObject().put("message", "Opened $url").put("handler", handler)
    }

    private fun config(p: JSONObject, peerSource: String): JSONObject {
        if (p.optBoolOrNull("nearby_discovery") == true) {
            throw RpcException.typed("unsupported", "nearby_discovery is iPhone-only (AWDL); the Android server is reached over the LAN or adb forward.")
        }
        val msgs = ArrayList<String>()
        p.optStringOrNull("revoke_session_id")?.let { id ->
            msgs.add(if (auth.revokeSession(id)) "Revoked $id" else "No session $id")
        }
        if (p.optBoolOrNull("clear_sessions") == true) { auth.revokeAll(); msgs.add("Revoked all sessions") }

        val allow = p.optBoolOrNull("allow_remote_access")
        val port = p.optDoubleOrNull("listen_port")?.toInt()
        if (port != null && port !in 1024..65535) throw RpcException.invalid("listen_port must be 1024..65535 (Android apps cannot bind lower ports)")
        val name = p.optStringOrNull("device_name")
        if (name != null && name.toByteArray(Charsets.UTF_8).size > 63) throw RpcException.invalid("device_name is at most 63 bytes")

        val cur = hooks.config()
        if (allow != null || port != null || name != null) {
            hooks.applyConfig(allow, port, name)
            if (allow != null && allow != cur.allowRemote) msgs.add(if (allow) "Listening on all interfaces shortly" else "Listening on 127.0.0.1 only shortly")
            if (port != null && port != cur.port) msgs.add("Moving to port $port shortly")
            if (name != null) msgs.add("Device name set")
        }
        val stopping = p.optBoolOrNull("stop_server") == true
        if (stopping) { hooks.scheduleStop(); msgs.add("Stopping the server in ~1 s") }

        val now = hooks.config()
        val sessions = JSONArray()
        auth.activeSessions.forEach { sessions.put(it.publicJson(AuthManager.INACTIVITY_MS)) }
        return JSONObject()
            .put("message", msgs.ifEmpty { listOf("Current configuration") }.joinToString("; "))
            .put("allow_remote_access", allow ?: now.allowRemote)
            .put("listen_port", port ?: now.port)
            .put("device_name", now.deviceName)
            .put("device_id", now.deviceId)
            .put("request_source", peerSource)
            .put("stopping", stopping)
            .put("sessions", sessions)
    }
}
