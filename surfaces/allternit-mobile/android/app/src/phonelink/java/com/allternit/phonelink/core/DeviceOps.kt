package com.allternit.phonelink.core

import org.json.JSONObject

/** Screen rectangle in PIXELS (Android's coordinate space). */
data class Bounds(val left: Int, val top: Int, val right: Int, val bottom: Int) {
    val width: Int get() = right - left
    val height: Int get() = bottom - top
    val centerX: Int get() = left + width / 2
    val centerY: Int get() = top + height / 2
    val isEmpty: Boolean get() = width <= 0 || height <= 0
    fun intersects(o: Bounds) = left < o.right && o.left < right && top < o.bottom && o.top < bottom
    fun contains(o: Bounds) = left <= o.left && top <= o.top && right >= o.right && bottom >= o.bottom
}

/**
 * One accessibility node, flattened in pre-order with its depth. `handle` is
 * the platform object (AccessibilityNodeInfo on device, anything in tests) and
 * never leaves the process.
 */
class UiNode(
    val depth: Int,
    val className: String,
    val text: String?,
    val contentDesc: String?,
    val resourceId: String?,
    val packageName: String?,
    val bounds: Bounds,
    val clickable: Boolean = false,
    val longClickable: Boolean = false,
    val editable: Boolean = false,
    val focused: Boolean = false,
    val scrollable: Boolean = false,
    val enabled: Boolean = true,
    val checkable: Boolean = false,
    val checked: Boolean = false,
    val selected: Boolean = false,
    val visibleToUser: Boolean = true,
    val password: Boolean = false,
    val hint: String? = null,
    /** "application", "input_method", "system", ... for window roots; null on children. */
    val windowType: String? = null,
    val handle: Any? = null,
) {
    val shortClass: String get() = className.substringAfterLast('.').ifEmpty { "View" }

    /** iCTRL "label": the accessibility label = content-desc, else the text. */
    val label: String get() = contentDesc?.takeIf { it.isNotEmpty() } ?: text.orEmpty()

    /** iCTRL "value": an editable's text, or the text when content-desc already is the label. */
    val value: String?
        get() {
            val t = text?.takeIf { it.isNotEmpty() } ?: return null
            if (editable) return t
            return if (!contentDesc.isNullOrEmpty() && t != contentDesc) t else null
        }

    /** Resource id without the "pkg:id/" prefix. */
    val shortId: String? get() = resourceId?.substringAfter(":id/")
}

data class StaticDeviceInfo(
    val manufacturer: String,
    val model: String,
    val androidRelease: String,
    val sdkInt: Int,
    val widthPx: Int,
    val heightPx: Int,
    val densityDpi: Int,
    val density: Float,
)

data class AppInfo(val packageName: String, val label: String, val system: Boolean)

class Screenshot(val bytes: ByteArray, val format: String, val pixelWidth: Int, val pixelHeight: Int)

enum class GlobalAction { HOME, BACK, RECENTS, NOTIFICATIONS }

/** Typed RPC failure; becomes a JSON-RPC error with data.type. */
class RpcException(val code: Int, message: String, val type: String? = null, val extra: JSONObject? = null) : Exception(message) {
    companion object {
        const val SERVER_ERROR = -32000
        const val INVALID_PARAMS = -32602
        const val METHOD_NOT_FOUND = -32601
        fun typed(type: String, message: String, extra: JSONObject? = null) = RpcException(SERVER_ERROR, message, type, extra)
        fun invalid(message: String) = RpcException(INVALID_PARAMS, message, "invalid_params")
    }
}

/**
 * Everything the dispatcher needs from the device. On Android this is the
 * AccessibilityService; tests use a fake. All coordinates are pixels.
 */
interface DeviceOps {
    fun isScreenOn(): Boolean
    fun isScreenLocked(): Boolean
    fun foregroundPackage(): String?

    /** Pre-order nodes of the on-screen windows (app, IME, system dialogs). */
    fun snapshot(): List<UiNode>

    fun tap(x: Float, y: Float): Boolean
    fun doubleTap(x: Float, y: Float): Boolean
    fun press(x: Float, y: Float, durationMs: Long): Boolean
    /** Finger from (x1,y1) to (x2,y2) over [durationMs], optionally holding at the start first. */
    fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long, holdStartMs: Long = 0): Boolean

    /** ACTION_CLICK on the node or its nearest clickable ancestor. */
    fun click(node: UiNode): Boolean
    /** ACTION_SCROLL_FORWARD / BACKWARD on the node. */
    fun scrollNode(node: UiNode, forward: Boolean): Boolean

    /** The node holding input focus (editable), freshly read. */
    fun focusedInput(): UiNode?
    fun setText(node: UiNode, text: String): Boolean
    /** Clipboard paste at the cursor. */
    fun paste(node: UiNode, text: String): Boolean
    /** IME action (Enter/Search/Send). API 30+. */
    fun imeEnter(node: UiNode): Boolean

    fun globalAction(action: GlobalAction): Boolean

    fun isInstalled(packageName: String): Boolean
    fun launchApp(packageName: String): Boolean
    /** Best effort; returns a short description of what was done. */
    fun terminateApp(packageName: String): String
    fun listApps(): List<AppInfo>
    /** Package that would open [url], or null if nothing can. */
    fun resolveUrl(url: String): String?
    fun openUrl(url: String): Boolean

    /** @param scale output pixels per screen pixel (0.1..1). Throws RpcException. */
    fun screenshot(format: String, quality: Float, scale: Float): Screenshot

    /** Waits until no UI change has been seen for [quietMs], up to [timeoutMs]. True if settled. */
    fun waitForIdle(quietMs: Long, timeoutMs: Long): Boolean

    fun systemMetrics(): JSONObject
}
