package com.allternit.phonelink.core

import org.json.JSONArray
import org.json.JSONObject

/** Method catalogue served at GET /schema, plus the GET /skill manual. */
object Schema {
    const val PROTOCOL_VERSION = 1
    const val SERVER_VERSION = "0.2.1-android"
    const val PLATFORM = "android"

    /** Feature tags (a subset of iCTRL's, plus android-only ones). */
    val CAPABILITY_TAGS = listOf(
        "envelope_v1", "page_source_filter", "visible_only", "max_depth", "find_element_ref",
        "clear_text", "open_url", "stop_server", "screenshot_jpeg", "screen_text",
        "system_metrics", "android", "coordinate_pixels", "back_button", "page_source_structured",
    )

    class P(val name: String, val type: String, val required: Boolean, val description: String)
    class M(val name: String, val description: String, val params: List<P>, val returns: String)

    private val settle = listOf(
        P("settle", "boolean", false, "Default true: wait until the UI stops changing (accessibility events quiet for 300 ms, up to settle_timeout) and report it in `settle`."),
        P("settle_timeout", "number", false, "Max seconds to wait for the UI to settle (default 2, max 3)."),
    )

    val METHODS: List<M> = listOf(
        M("device_info", "Device, screen and server info. Coordinates for every action are in PIXELS of the full screen (coordinate_space: 'pixels', scale 1).", emptyList(),
            "{device_name, model, system_version, platform, point_width, point_height, pixel_width, pixel_height, scale, coordinate_space, protocol_version, capabilities, system{...}}"),
        M("config", "Read or change server settings and paired sessions.", listOf(
            P("allow_remote_access", "boolean", false, "true = listen on all interfaces (LAN); false = 127.0.0.1 only"),
            P("listen_port", "number", false, "TCP port (1024..65535, default 8200)"),
            P("device_name", "string", false, "Name reported by /whoami; '' restores the default"),
            P("revoke_session_id", "string", false, "Revoke one paired client"),
            P("clear_sessions", "boolean", false, "Revoke every paired client"),
            P("stop_server", "boolean", false, "Stop the server ~1 s after replying. It stays off (also across reboots) until started from the app or re-provisioned."),
        ), "{message, allow_remote_access, listen_port, device_name, sessions:[{session_id, client_name, granted_at, last_active_at, expires, last_address, last_via}]}"),
        M("page_source", "Accessibility tree of the windows on screen (app, keyboard, system dialogs). Text form close to iCTRL's: `Class, label: '…', value: '…', id: 'pkg:id/x', {{x, y}, {w, h}}, flags` in PIXELS.", listOf(
            P("filter", "object", false, "{label:'…'} | {type:'button'} | {text:'…'}: matching nodes plus their ancestors"),
            P("max_depth", "number", false, "Max depth"),
            P("max_elements", "number", false, "Max nodes returned (default 2000, cap 4000)"),
            P("visible_only", "boolean", false, "Only nodes on screen (plus ancestors)"),
            P("structured", "boolean", false, "Also return `nodes`: one object per node with class, text, content_desc, resource_id, bounds, flags"),
        ), "{format:'text', source, truncated, total_returned, coordinate_space:'pixels', foreground_app, nodes?}"),
        M("screen_text", "Every text on screen with its frame, read from the accessibility tree (no OCR on Android).", listOf(
            P("element_id", "string", false, "Only items containing this substring (case-insensitive)"),
            P("include_image", "boolean", false, "Also attach a JPEG screenshot (API 30+)"),
        ), "{count, items:[{text, x, y, width, height, center_x, center_y}], source:'tree'}"),
        M("screenshot", "Capture the screen (Android 11+/API 30+).", listOf(
            P("format", "string", false, "'jpeg' (default) or 'png'"),
            P("quality", "number", false, "JPEG quality 0.1-1.0 (default 0.6)"),
            P("scale", "number", false, "Output pixels per screen pixel, 0.1-1 (default 1; larger values are clamped to 1)"),
        ), "{format, encoding:'base64', data, bytes, pixel_width, pixel_height, point_width, point_height, scale}"),
        M("tap", "Tap at x,y (pixels) or an element ref.", listOf(P("ref", "string", false, "Element ref from find_element"), P("x", "number", false, "X in pixels"), P("y", "number", false, "Y in pixels")) + settle, "{message, x, y, settle?}"),
        M("double_tap", "Double tap.", listOf(P("ref", "string", false, "Element ref"), P("x", "number", false, "X"), P("y", "number", false, "Y")) + settle, "{message}"),
        M("long_press", "Press and hold.", listOf(P("ref", "string", false, "Element ref"), P("x", "number", false, "X"), P("y", "number", false, "Y"), P("duration", "number", false, "Seconds (default 1.0)")) + settle, "{message}"),
        M("swipe", "Finger swipe. 'up' moves the finger up (reveals content below).", listOf(
            P("x", "number", false, "Start X (default centre)"), P("y", "number", false, "Start Y (default centre)"),
            P("direction", "string", false, "up/down/left/right (default up)"),
            P("distance", "number", false, "Finger travel in pixels (default 1/3 of the screen height)"),
            P("velocity", "string", false, "fast | normal (default) | slow"),
        ) + settle, "{message}"),
        M("drag", "Drag between two points or element refs.", listOf(
            P("from_ref", "string", false, "Start ref"), P("to_ref", "string", false, "End ref"),
            P("from_x", "number", false, "Start X"), P("from_y", "number", false, "Start Y"),
            P("to_x", "number", false, "End X"), P("to_y", "number", false, "End Y"),
            P("duration", "number", false, "Seconds (default 0.5)"),
            P("hold", "number", false, "Seconds to hold at the start first (default 0; ~0.6 picks up list items)"),
        ) + settle, "{message}"),
        M("scroll", "Coarse scroll of the main list (ACTION_SCROLL_*), falling back to a swipe. Prefer swipe.", listOf(P("direction", "string", false, "down (default, reveal below) / up / left / right")) + settle, "{message, method}"),
        M("type", "Type into the focused field (ACTION_SET_TEXT; falls back to clipboard paste). Appends unless clear:true.", listOf(
            P("text", "string", true, "Text"), P("clear", "boolean", false, "Replace the field's contents"),
            P("submit", "boolean", false, "Press the IME action (Enter/Send/Search) after typing (API 30+)"),
            P("x", "number", false, "Tap here first to focus"), P("y", "number", false, "Tap here first to focus"),
        ), "{message, method:'set_text'|'paste', warning?}"),
        M("clear_text", "Empty the focused field.", emptyList(), "{message}"),
        M("find_element", "Find an element by label (content-desc or text), resource-id (full or after ':id/'), or substring with match:'contains'. Returns a ref.", listOf(
            P("element_id", "string", true, "Label, text, content-desc or resource-id"),
            P("match", "string", false, "'exact' (default) or 'contains'"),
            P("element_type", "string", false, "button/textfield/label/image/cell/switch/any"),
        ), "{exists, hittable, ref, element_type, is_text_input, x, y, width, height, center_x, center_y, label, value, resource_id, visibility, match_count, alternatives?}"),
        M("tap_element", "find_element + tap in one call.", listOf(P("element_id", "string", true, "Label or id"), P("match", "string", false, "'exact' or 'contains'"), P("element_type", "string", false, "Type filter")) + settle, "{message, matched, frame}"),
        M("find_and_tap", "Same as tap_element.", listOf(P("element_id", "string", true, "Label or id"), P("match", "string", false, "'exact' or 'contains'"), P("element_type", "string", false, "Type filter")) + settle, "{message, matched, frame:{x,y,w,h}}"),
        M("home", "Press Home.", emptyList(), "{message, foreground_app, previous_app}"),
        M("back", "Press Back (Android only).", emptyList(), "{message, foreground_app}"),
        M("is_locked", "Whether the keyguard is showing.", emptyList(), "{locked, screen_on}"),
        M("list_apps", "Installed launchable apps.", listOf(P("text", "string", false, "Filter by name or package substring"), P("element_type", "string", false, "'all' to include system apps")), "{count, source:'system', apps:[{bundle_id, package, name, system}]}"),
        M("launch_app", "Open an app by package name (bundle_id is accepted as an alias).", listOf(P("bundle_id", "string", true, "Package name, e.g. com.openai.chatgpt")), "{message, hint}"),
        M("activate_app", "Bring an installed app to the front.", listOf(P("bundle_id", "string", true, "Package name")), "{message}"),
        M("terminate_app", "Best effort: leaves the app (Home) and kills its background process. Android does not let a normal app force-stop another.", listOf(P("bundle_id", "string", true, "Package name")), "{message, best_effort:true}"),
        M("open_url", "Open a URL or deep link.", listOf(P("url", "string", true, "URL")), "{message, handler}"),
        M("wait", "Sleep.", listOf(P("duration", "number", false, "Seconds (default 1, max 30)")), "{message}"),
    )

    fun document(): JSONObject {
        val methods = JSONArray()
        for (m in METHODS) {
            val params = JSONArray()
            for (p in m.params) params.put(JSONObject().put("name", p.name).put("type", p.type).put("required", p.required).put("description", p.description))
            methods.put(JSONObject().put("name", m.name).put("description", m.description).put("params", params).put("returns", m.returns))
        }
        return JSONObject()
            .put("protocol", "JSON-RPC 2.0")
            .put("server_version", SERVER_VERSION)
            .put("platform", PLATFORM)
            .put("protocol_version", PROTOCOL_VERSION)
            .put("capability_tags", JSONArray(CAPABILITY_TAGS))
            .put("endpoints", JSONObject()
                .put("pair", "POST /pair {client_name, plain:true (loopback only) | pub:<b64 raw 32B X25519>} → wrapped key. Blocks until the phone user taps Allow (60 s) unless a one-shot grant was armed over adb.")
                .put("rpc", "POST /rpc v1 encrypted envelope {v:1, tok, ts, nonce, ct, tag} (same as iCTRL).")
                .put("whoami", "GET /whoami {device_id, name, protocol_version, server_version, platform}")
                .put("schema", "GET /schema this document")
                .put("skill", "GET /skill operating manual (Markdown)"))
            .put("methods", methods)
    }

    val SKILL_MD: String = """
---
name: allternit-phone-link-android
description: Drive the apps on an Android phone through Allternit Phone Link (iCTRL protocol v1).
skill_version: $SERVER_VERSION
---

# Allternit Phone Link (Android)

Same wire protocol as the iCTRL iPhone runner (protocol_version 1): pair once with
`POST /pair`, then send every JSON-RPC call to `POST /rpc` as a sealed envelope.
`ictrl_rpc.py` and Allternit Desktop work unchanged.

Differences from iCTRL:
- Coordinates are **pixels** (device_info.coordinate_space = "pixels", scale 1).
- Apps are addressed by **package name** (`bundle_id` is accepted as the parameter name).
- `back` exists. `screen_text` reads text from the accessibility tree (no OCR).
- No /stream, /remote, /logs, nearby discovery, resolve_app, page/collect, scroll_to_element.
- UI methods refuse with error data.type = "screen_locked" / "screen_off" while the phone is
  locked or dark, and "protected_screen" on Settings, the permission controller or this app.

Fast path: `launch_app` → `page_source {visible_only:true}` → `find_element` → `tap {ref}` →
`type {text, submit:true}` → `page_source` again.
""".trimIndent()
}
