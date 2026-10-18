package com.allternit.phonelink

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.app.ActivityManager
import android.app.KeyguardManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Path
import android.graphics.Rect
import android.net.Uri
import android.os.BatteryManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.view.Display
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import com.allternit.phonelink.core.AppInfo
import com.allternit.phonelink.core.Bounds
import com.allternit.phonelink.core.DeviceOps
import com.allternit.phonelink.core.GlobalAction
import com.allternit.phonelink.core.RpcException
import com.allternit.phonelink.core.Screenshot
import com.allternit.phonelink.core.UiNode
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

/**
 * The phone's hands and eyes. Enabled once over adb (see README); the system
 * rebinds it after every reboot by itself, which also restarts the server.
 */
class PhoneLinkAccessibilityService : AccessibilityService(), DeviceOps {
    @Volatile private var lastEventAt = SystemClock.uptimeMillis()
    @Volatile private var lastWindowPackage: String? = null
    private val main = Handler(Looper.getMainLooper())
    private val shotExecutor = Executors.newSingleThreadExecutor()

    companion object {
        private const val MAX_NODES = 6000
        private const val MAX_DEPTH = 120
    }

    // ── Lifecycle ───────────────────────────────────────────────────────────

    override fun onServiceConnected() {
        super.onServiceConnected()
        ServerController.onAccessibilityConnected(this, this)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        lastEventAt = SystemClock.uptimeMillis()
        if (event?.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) {
            event.packageName?.toString()?.let { lastWindowPackage = it }
        }
    }

    override fun onInterrupt() {}

    override fun onUnbind(intent: Intent?): Boolean {
        ServerController.onAccessibilityDisconnected(this)
        return super.onUnbind(intent)
    }

    override fun onDestroy() {
        ServerController.onAccessibilityDisconnected(this)
        shotExecutor.shutdownNow()
        super.onDestroy()
    }

    // ── State ───────────────────────────────────────────────────────────────

    override fun isScreenOn(): Boolean = getSystemService(PowerManager::class.java).isInteractive

    override fun isScreenLocked(): Boolean = getSystemService(KeyguardManager::class.java).isKeyguardLocked

    override fun foregroundPackage(): String? {
        val wins = try { windows } catch (e: Exception) { emptyList<AccessibilityWindowInfo>() }
        val app = wins.firstOrNull { it.type == AccessibilityWindowInfo.TYPE_APPLICATION && it.isFocused }
            ?: wins.firstOrNull { it.type == AccessibilityWindowInfo.TYPE_APPLICATION && it.isActive }
        return app?.root?.packageName?.toString() ?: rootInActiveWindow?.packageName?.toString() ?: lastWindowPackage
    }

    // ── Tree ────────────────────────────────────────────────────────────────

    private fun windowTypeName(t: Int) = when (t) {
        AccessibilityWindowInfo.TYPE_APPLICATION -> "application"
        AccessibilityWindowInfo.TYPE_INPUT_METHOD -> "input_method"
        AccessibilityWindowInfo.TYPE_SYSTEM -> "system"
        AccessibilityWindowInfo.TYPE_ACCESSIBILITY_OVERLAY -> "accessibility_overlay"
        AccessibilityWindowInfo.TYPE_SPLIT_SCREEN_DIVIDER -> "divider"
        else -> "other"
    }

    override fun snapshot(): List<UiNode> {
        val out = ArrayList<UiNode>()
        val screenH = ServerController.staticInfo().heightPx
        val wins = try { windows } catch (e: Exception) { emptyList<AccessibilityWindowInfo>() }
        val roots = ArrayList<Pair<AccessibilityNodeInfo, Pair<String, Rect>>>()
        if (wins.isEmpty()) {
            rootInActiveWindow?.let { r -> val b = Rect(); r.getBoundsInScreen(b); roots.add(r to ("application" to b)) }
        } else {
            // Bottom layer first, so the app comes before the keyboard and dialogs on top of it.
            for (w in wins.sortedBy { it.layer }) {
                val type = windowTypeName(w.type)
                if (type == "accessibility_overlay" || type == "divider") continue
                val b = Rect(); w.getBoundsInScreen(b)
                // Skip the status and navigation bars: thin system windows full of icons.
                if (type == "system" && b.height() < screenH / 5) continue
                val root = w.root ?: continue
                roots.add(root to (type to b))
            }
        }
        for ((root, meta) in roots) {
            if (out.size >= MAX_NODES) break
            val (type, rect) = meta
            out.add(UiNode(0, "Window", null, null, null, root.packageName?.toString(),
                Bounds(rect.left, rect.top, rect.right, rect.bottom), windowType = type, handle = root))
            walk(root, 1, out)
        }
        return out
    }

    private fun walk(root: AccessibilityNodeInfo, startDepth: Int, out: MutableList<UiNode>) {
        val stack = ArrayDeque<Pair<AccessibilityNodeInfo, Int>>()
        stack.addLast(root to startDepth)
        while (stack.isNotEmpty() && out.size < MAX_NODES) {
            val (n, depth) = stack.removeLast()
            out.add(toUiNode(n, depth))
            if (depth >= MAX_DEPTH) continue
            // Push children in reverse so they pop in order (pre-order output).
            for (i in n.childCount - 1 downTo 0) {
                val c = try { n.getChild(i) } catch (e: Exception) { null } ?: continue
                stack.addLast(c to depth + 1)
            }
        }
    }

    private fun toUiNode(n: AccessibilityNodeInfo, depth: Int): UiNode {
        val r = Rect(); n.getBoundsInScreen(r)
        val hint = n.hintText?.toString()
        return UiNode(
            depth = depth,
            className = n.className?.toString() ?: "android.view.View",
            text = n.text?.toString(),
            contentDesc = n.contentDescription?.toString(),
            resourceId = n.viewIdResourceName,
            packageName = n.packageName?.toString(),
            bounds = Bounds(r.left, r.top, r.right, r.bottom),
            clickable = n.isClickable,
            longClickable = n.isLongClickable,
            editable = n.isEditable,
            focused = n.isFocused,
            scrollable = n.isScrollable,
            enabled = n.isEnabled,
            checkable = n.isCheckable,
            checked = n.isChecked,
            selected = n.isSelected,
            visibleToUser = n.isVisibleToUser,
            password = n.isPassword,
            hint = hint,
            handle = n,
        )
    }

    private fun nodeOf(u: UiNode): AccessibilityNodeInfo? = (u.handle as? AccessibilityNodeInfo)?.also { it.refresh() }

    // ── Gestures ────────────────────────────────────────────────────────────

    private fun clampX(x: Float) = x.coerceAtLeast(0f)
    private fun clampY(y: Float) = y.coerceAtLeast(0f)

    private fun perform(g: GestureDescription, expectedMs: Long): Boolean {
        val latch = CountDownLatch(1)
        val ok = AtomicBoolean(false)
        val cb = object : AccessibilityService.GestureResultCallback() {
            override fun onCompleted(gestureDescription: GestureDescription?) { ok.set(true); latch.countDown() }
            override fun onCancelled(gestureDescription: GestureDescription?) { latch.countDown() }
        }
        val dispatched = AtomicBoolean(false)
        val posted = CountDownLatch(1)
        main.post { dispatched.set(dispatchGesture(g, cb, null)); posted.countDown() }
        posted.await(2, TimeUnit.SECONDS)
        if (!dispatched.get()) return false
        latch.await(expectedMs + 3000, TimeUnit.MILLISECONDS)
        return ok.get()
    }

    private fun point(x: Float, y: Float) = Path().apply { moveTo(clampX(x), clampY(y)) }

    override fun tap(x: Float, y: Float): Boolean =
        perform(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(point(x, y), 0, 50)).build(), 50)

    override fun doubleTap(x: Float, y: Float): Boolean = perform(
        GestureDescription.Builder()
            .addStroke(GestureDescription.StrokeDescription(point(x, y), 0, 40))
            .addStroke(GestureDescription.StrokeDescription(point(x, y), 140, 40))
            .build(), 180,
    )

    override fun press(x: Float, y: Float, durationMs: Long): Boolean {
        val d = durationMs.coerceIn(1, GestureDescription.getMaxGestureDuration())
        return perform(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(point(x, y), 0, d)).build(), d)
    }

    override fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long, holdStartMs: Long): Boolean {
        val d = durationMs.coerceIn(1, GestureDescription.getMaxGestureDuration())
        val line = Path().apply { moveTo(clampX(x1), clampY(y1)); lineTo(clampX(x2), clampY(y2)) }
        if (holdStartMs <= 0) {
            return perform(GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(line, 0, d)).build(), d)
        }
        // Hold, then move as a continuation of the same finger (API 26+).
        val hold = GestureDescription.StrokeDescription(point(x1, y1), 0, holdStartMs, true)
        if (!perform(GestureDescription.Builder().addStroke(hold).build(), holdStartMs)) return false
        val move = hold.continueStroke(line, 0, d, false)
        return perform(GestureDescription.Builder().addStroke(move).build(), d)
    }

    override fun click(node: UiNode): Boolean {
        var n: AccessibilityNodeInfo? = nodeOf(node)
        var hops = 0
        while (n != null && !n.isClickable && hops < 8) { n = n.parent; hops++ }
        return n?.performAction(AccessibilityNodeInfo.ACTION_CLICK) == true
    }

    override fun scrollNode(node: UiNode, forward: Boolean): Boolean =
        nodeOf(node)?.performAction(if (forward) AccessibilityNodeInfo.ACTION_SCROLL_FORWARD else AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD) == true

    // ── Text ────────────────────────────────────────────────────────────────

    override fun focusedInput(): UiNode? {
        val f = findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return null
        return if (f.isEditable) toUiNode(f, 0) else null
    }

    override fun setText(node: UiNode, text: String): Boolean {
        val n = nodeOf(node) ?: return false
        val args = Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) }
        // The action's own result: re-reading the text to "verify" misfires on fields that
        // reformat input, and a false negative here would paste the text a second time.
        return n.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
    }

    override fun paste(node: UiNode, text: String): Boolean {
        val n = nodeOf(node) ?: return false
        val done = CountDownLatch(1)
        main.post {
            try {
                getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Allternit", text))
            } finally { done.countDown() }
        }
        done.await(2, TimeUnit.SECONDS)
        return n.performAction(AccessibilityNodeInfo.ACTION_PASTE)
    }

    override fun imeEnter(node: UiNode): Boolean {
        if (Build.VERSION.SDK_INT < 30) return false
        val n = nodeOf(node) ?: return false
        return n.performAction(AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.id)
    }

    override fun globalAction(action: GlobalAction): Boolean = performGlobalAction(
        when (action) {
            GlobalAction.HOME -> AccessibilityService.GLOBAL_ACTION_HOME
            GlobalAction.BACK -> AccessibilityService.GLOBAL_ACTION_BACK
            GlobalAction.RECENTS -> AccessibilityService.GLOBAL_ACTION_RECENTS
            GlobalAction.NOTIFICATIONS -> AccessibilityService.GLOBAL_ACTION_NOTIFICATIONS
        },
    )

    // ── Apps ────────────────────────────────────────────────────────────────

    override fun isInstalled(packageName: String): Boolean =
        packageManager.getLaunchIntentForPackage(packageName) != null && !isCarrierStub(packageName)

    /**
     * Carrier "stub" apps (e.g. a preloaded WhatsApp whose versionName reads
     * "stub(132.0.26)") have a launcher icon but only open an install page.
     * They are not installed apps: they never list and never launch.
     */
    private fun isCarrierStub(packageName: String): Boolean = try {
        @Suppress("DEPRECATION")
        packageManager.getPackageInfo(packageName, 0).versionName?.startsWith("stub", ignoreCase = true) == true
    } catch (e: PackageManager.NameNotFoundException) { false }

    override fun launchApp(packageName: String): Boolean {
        if (isCarrierStub(packageName)) return false
        val intent = packageManager.getLaunchIntentForPackage(packageName) ?: return false
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_RESET_TASK_IF_NEEDED)
        return try { startActivity(intent); true } catch (e: Exception) { false }
    }

    override fun terminateApp(packageName: String): String {
        val wasFront = foregroundPackage() == packageName
        if (wasFront) {
            performGlobalAction(AccessibilityService.GLOBAL_ACTION_HOME)
            SystemClock.sleep(500)
        }
        getSystemService(ActivityManager::class.java).killBackgroundProcesses(packageName)
        return if (wasFront) "Left $packageName (Home) and asked Android to kill its background process"
        else "Asked Android to kill $packageName's background process"
    }

    override fun listApps(): List<AppInfo> {
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        @Suppress("DEPRECATION")
        val list = packageManager.queryIntentActivities(intent, 0)
        return list.distinctBy { it.activityInfo.packageName }.filterNot { isCarrierStub(it.activityInfo.packageName) }.map { ri ->
            val ai = ri.activityInfo.applicationInfo
            val system = (ai.flags and ApplicationInfo.FLAG_SYSTEM) != 0 && (ai.flags and ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) == 0
            AppInfo(ri.activityInfo.packageName, ri.loadLabel(packageManager).toString(), system)
        }
    }

    override fun resolveUrl(url: String): String? {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
        @Suppress("DEPRECATION")
        val ri = packageManager.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY) ?: return null
        return ri.activityInfo?.packageName
    }

    override fun openUrl(url: String): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try { startActivity(intent); true } catch (e: Exception) { false }
    }

    // ── Screenshot ──────────────────────────────────────────────────────────

    override fun screenshot(format: String, quality: Float, scale: Float): Screenshot {
        if (Build.VERSION.SDK_INT < 30) throw RpcException.typed("unsupported", "screenshot needs Android 11 (API 30) or later")
        var bitmap: Bitmap? = null
        var lastError = 0
        for (attempt in 0 until 3) {
            val result = AtomicReference<Bitmap?>()
            val err = AtomicReference(0)
            val latch = CountDownLatch(1)
            takeScreenshot(Display.DEFAULT_DISPLAY, shotExecutor, object : AccessibilityService.TakeScreenshotCallback {
                override fun onSuccess(screenshot: AccessibilityService.ScreenshotResult) {
                    val hb = screenshot.hardwareBuffer
                    try {
                        val hw = Bitmap.wrapHardwareBuffer(hb, screenshot.colorSpace)
                        result.set(hw?.copy(Bitmap.Config.ARGB_8888, false))
                        hw?.recycle()
                    } finally {
                        hb.close()
                        latch.countDown()
                    }
                }
                override fun onFailure(errorCode: Int) { err.set(errorCode); latch.countDown() }
            })
            latch.await(5, TimeUnit.SECONDS)
            bitmap = result.get()
            if (bitmap != null) break
            lastError = err.get()
            // ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT: the system rate-limits captures.
            if (lastError == AccessibilityService.ERROR_TAKE_SCREENSHOT_INTERVAL_TIME_SHORT) SystemClock.sleep(400) else break
        }
        val full = bitmap ?: throw when (lastError) {
            AccessibilityService.ERROR_TAKE_SCREENSHOT_SECURE_WINDOW -> RpcException.typed("secure_window", "The app on screen blocks screenshots (FLAG_SECURE). Use page_source instead.")
            0 -> RpcException.typed("screenshot_failed", "Screenshot timed out")
            else -> RpcException.typed("screenshot_failed", "Screenshot failed (Android error $lastError)")
        }
        val w = maxOf(1, (full.width * scale).toInt())
        val h = maxOf(1, (full.height * scale).toInt())
        val scaled = if (w == full.width && h == full.height) full else Bitmap.createScaledBitmap(full, w, h, true)
        val png = format == "png"
        val bytes = ByteArrayOutputStream().use { os ->
            scaled.compress(if (png) Bitmap.CompressFormat.PNG else Bitmap.CompressFormat.JPEG, (quality * 100).toInt().coerceIn(10, 100), os)
            os.toByteArray()
        }
        if (scaled !== full) scaled.recycle()
        full.recycle()
        return Screenshot(bytes, if (png) "png" else "jpeg", w, h)
    }

    // ── Misc ────────────────────────────────────────────────────────────────

    override fun waitForIdle(quietMs: Long, timeoutMs: Long): Boolean {
        val start = SystemClock.uptimeMillis()
        // Give the app a moment to start reacting before judging quiet.
        SystemClock.sleep(minOf(120L, timeoutMs))
        while (true) {
            val now = SystemClock.uptimeMillis()
            if (now - lastEventAt >= quietMs) return true
            if (now - start >= timeoutMs) return false
            SystemClock.sleep(50)
        }
    }

    override fun systemMetrics(): JSONObject {
        val o = JSONObject()
        try {
            val bm = getSystemService(BatteryManager::class.java)
            o.put("battery_level", bm.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY))
            o.put("charging", bm.isCharging)
        } catch (_: Exception) {}
        try {
            val pm = getSystemService(PowerManager::class.java)
            if (Build.VERSION.SDK_INT >= 29) o.put("thermal_status", pm.currentThermalStatus)
            o.put("battery_optimization_exempt", pm.isIgnoringBatteryOptimizations(packageName))
        } catch (_: Exception) {}
        try {
            val mi = ActivityManager.MemoryInfo()
            getSystemService(ActivityManager::class.java).getMemoryInfo(mi)
            o.put("memory_available_mb", mi.availMem / (1024 * 1024))
            o.put("memory_total_mb", mi.totalMem / (1024 * 1024))
        } catch (_: Exception) {}
        return o
    }
}
