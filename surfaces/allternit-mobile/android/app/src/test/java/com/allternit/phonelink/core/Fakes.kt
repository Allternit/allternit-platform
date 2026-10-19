package com.allternit.phonelink.core

import org.json.JSONObject

/** Scriptable stand-in for the accessibility service. */
class FakeDeviceOps : DeviceOps {
    var screenOn = true
    var locked = false
    var foreground: String? = "com.openai.chatgpt"
    var nodes: List<UiNode> = defaultTree()
    var focused: UiNode? = null
    var setTextWorks = true
    var installed = mutableSetOf("com.openai.chatgpt", "com.anthropic.claude", "org.telegram.messenger")
    var urlHandler: String? = "com.android.chrome"
    val actions = ArrayList<String>()
    var fieldText = ""

    override fun isScreenOn() = screenOn
    override fun isScreenLocked() = locked
    override fun foregroundPackage() = foreground
    override fun snapshot() = nodes
    override fun tap(x: Float, y: Float): Boolean { actions.add("tap ${x.toInt()},${y.toInt()}"); return true }
    override fun doubleTap(x: Float, y: Float): Boolean { actions.add("double_tap ${x.toInt()},${y.toInt()}"); return true }
    override fun press(x: Float, y: Float, durationMs: Long): Boolean { actions.add("press ${x.toInt()},${y.toInt()} ${durationMs}ms"); return true }
    override fun swipe(x1: Float, y1: Float, x2: Float, y2: Float, durationMs: Long, holdStartMs: Long): Boolean {
        actions.add("swipe ${x1.toInt()},${y1.toInt()}->${x2.toInt()},${y2.toInt()} ${durationMs}ms hold=$holdStartMs"); return true
    }
    override fun click(node: UiNode): Boolean { actions.add("click ${node.label}"); return true }
    override fun scrollNode(node: UiNode, forward: Boolean): Boolean { actions.add("scroll_node forward=$forward"); return true }
    override fun focusedInput(): UiNode? = focused
    override fun setText(node: UiNode, text: String): Boolean {
        if (!setTextWorks) return false
        fieldText = text; actions.add("set_text '$text'"); return true
    }
    override fun paste(node: UiNode, text: String): Boolean { fieldText += text; actions.add("paste '$text'"); return true }
    override fun imeEnter(node: UiNode): Boolean { actions.add("ime_enter"); return true }
    override fun globalAction(action: GlobalAction): Boolean { actions.add("global $action"); if (action == GlobalAction.HOME) foreground = "com.motorola.launcher3"; return true }
    override fun isInstalled(packageName: String) = packageName in installed
    override fun launchApp(packageName: String): Boolean { actions.add("launch $packageName"); foreground = packageName; return true }
    override fun terminateApp(packageName: String): String { actions.add("terminate $packageName"); return "killed $packageName" }
    override fun listApps() = listOf(
        AppInfo("com.openai.chatgpt", "ChatGPT", false),
        AppInfo("com.anthropic.claude", "Claude", false),
        AppInfo("com.android.settings", "Settings", true),
    )
    override fun resolveUrl(url: String) = urlHandler
    override fun openUrl(url: String): Boolean { actions.add("open $url"); return true }
    override fun screenshot(format: String, quality: Float, scale: Float): Screenshot {
        val w = (720 * scale).toInt(); val h = (1600 * scale).toInt()
        return Screenshot(byteArrayOf(1, 2, 3), format, w, h)
    }
    override fun waitForIdle(quietMs: Long, timeoutMs: Long) = true
    override fun systemMetrics(): JSONObject = JSONObject().put("battery_level", 80)

    companion object {
        val INFO = StaticDeviceInfo("motorola", "moto g play - 2024", "14", 34, 720, 1600, 280, 1.75f)

        fun defaultTree(): List<UiNode> = listOf(
            UiNode(0, "Window", null, null, null, "com.openai.chatgpt", Bounds(0, 0, 720, 1600), windowType = "application"),
            UiNode(1, "android.widget.FrameLayout", null, null, null, "com.openai.chatgpt", Bounds(0, 0, 720, 1600)),
            UiNode(2, "android.widget.TextView", "ChatGPT", null, "com.openai.chatgpt:id/title", "com.openai.chatgpt", Bounds(40, 80, 400, 140)),
            UiNode(2, "android.widget.EditText", "", null, "com.openai.chatgpt:id/composer", "com.openai.chatgpt", Bounds(20, 1400, 600, 1480),
                clickable = true, editable = true, hint = "Message"),
            UiNode(2, "android.widget.ImageButton", null, "Send prompt", "com.openai.chatgpt:id/send", "com.openai.chatgpt", Bounds(610, 1400, 700, 1480), clickable = true),
            UiNode(2, "androidx.recyclerview.widget.RecyclerView", null, null, "com.openai.chatgpt:id/list", "com.openai.chatgpt", Bounds(0, 160, 720, 1380), scrollable = true),
            UiNode(3, "android.widget.TextView", "Hello there", null, null, "com.openai.chatgpt", Bounds(20, 200, 700, 260)),
            UiNode(3, "android.widget.TextView", "Offscreen reply", null, null, "com.openai.chatgpt", Bounds(20, 1700, 700, 1760), visibleToUser = false),
        )
    }
}

class FakeHooks(var cfg: ServerConfig = ServerConfig("dev-1234", "Test phone", true, 8200)) : ServerHooks {
    var stopScheduled = false
    override fun config() = cfg
    override fun applyConfig(allowRemote: Boolean?, port: Int?, deviceName: String?) {
        cfg = cfg.copy(allowRemote = allowRemote ?: cfg.allowRemote, port = port ?: cfg.port, deviceName = deviceName ?: cfg.deviceName)
    }
    override fun scheduleStop() { stopScheduled = true }
    var screen = ScreenServiceStatus(false, "waiting_for_computer", "Waiting", ShellServerProtocol.LaunchLines("k", "s"))
    override fun screenService() = screen
}

/** Software AES-GCM standing in for the Keystore-backed sealer in unit tests. */
class AesGcmTestSealer : BlobSealer {
    private val key = javax.crypto.KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
    override fun seal(plain: ByteArray): ByteArray {
        val c = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding"); c.init(javax.crypto.Cipher.ENCRYPT_MODE, key)
        return c.iv + c.doFinal(plain)
    }
    override fun open(sealed: ByteArray): ByteArray {
        val c = javax.crypto.Cipher.getInstance("AES/GCM/NoPadding")
        c.init(javax.crypto.Cipher.DECRYPT_MODE, key, javax.crypto.spec.GCMParameterSpec(128, sealed.copyOfRange(0, 12)))
        return c.doFinal(sealed, 12, sealed.size - 12)
    }
}

/** Approver that answers immediately (or never, for timeout tests). */
class ScriptedApprover(var answer: Boolean?) : PairApprover {
    val requests = ArrayList<String>()
    val expired = ArrayList<String>()
    override fun requestApproval(requestId: String, clientName: String, fingerprint: String?, local: Boolean, answer: (Boolean) -> Unit) {
        requests.add("$clientName fp=$fingerprint local=$local")
        this.answer?.let(answer)
    }
    override fun expire(requestId: String) { expired.add(requestId) }
}
