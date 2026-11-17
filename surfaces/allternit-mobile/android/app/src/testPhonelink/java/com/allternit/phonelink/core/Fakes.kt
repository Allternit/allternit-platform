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
    val holdNotices = ArrayList<List<Any?>>()
    override fun holdNotice(waiting: Boolean, effect: String?, app: String?, timeoutMs: Long) { holdNotices.add(listOf(waiting, effect, app, timeoutMs)) }
    var screen = ScreenServiceStatus(false, "waiting_for_computer", "Waiting", ShellServerProtocol.LaunchLines("k", "s"))
    override fun screenService() = screen
    /** mesh_restart: whether there is a mesh, and what the restart does to cfg. */
    var meshAvailable = true
    val meshRestarts = ArrayList<Boolean>()
    var onMeshRestart: () -> Unit = {}
    override fun requestMeshRestart(deferred: Boolean): Boolean {
        if (!meshAvailable) return false
        meshRestarts.add(deferred)
        onMeshRestart()
        return true
    }
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

// ── App-phone lane fakes (AppPhoneLinkTest) ─────────────────────────────────

class FakeCredentialStore(var value: AppPhoneCredentials? = null) : CredentialStore {
    override fun load() = value
    override fun save(credentials: AppPhoneCredentials) { value = credentials }
    override fun clear() { value = null }
}

/** Runs background/serial work immediately; scheduled tasks wait for [runDue]. */
class FakeLinkRuntime(var now: Long = 1_700_000_000_000L) : LinkRuntime {
    class Task(val at: Long, val run: () -> Unit) : Cancellable {
        var cancelled = false
        override fun cancel() { cancelled = true }
    }
    val tasks = ArrayList<Task>()
    override fun nowMs() = now
    override fun background(task: () -> Unit) = task()
    override fun serial(task: () -> Unit) = task()
    override fun schedule(delayMs: Long, task: () -> Unit): Cancellable = Task(now + delayMs, task).also { tasks.add(it) }
    /** Advances the clock by [ms] and runs what became due (not cancelled). */
    fun advance(ms: Long) {
        now += ms
        val due = tasks.filter { !it.cancelled && it.at <= now }
        tasks.removeAll(due)
        due.forEach { it.run() }
    }
}

class FakeHttp : JsonHttp {
    class Call(val url: String, val headers: Map<String, String>, val body: String)
    val calls = ArrayList<Call>()
    /** Reply for the first matching URL suffix. */
    val replies = LinkedHashMap<String, HttpReply>()
    override fun postJson(url: String, headers: Map<String, String>, body: String): HttpReply {
        calls.add(Call(url, headers, body))
        return replies.entries.firstOrNull { url.endsWith(it.key) }?.value ?: HttpReply(404, "{}")
    }
}

class FakeSocket(val url: String, val listener: SocketListener) : SocketLink {
    val texts = ArrayList<String>()
    val binaries = ArrayList<ByteArray>()
    var closed: Pair<Int, String>? = null
    override fun sendText(text: String): Boolean { texts.add(text); return true }
    override fun sendBinary(bytes: ByteArray): Boolean { binaries.add(bytes); return true }
    override fun close(code: Int, reason: String) { closed = code to reason }
}

class FakeConnector : SocketConnector {
    val sockets = ArrayList<FakeSocket>()
    override fun connect(url: String, listener: SocketListener): FakeSocket = FakeSocket(url, listener).also { sockets.add(it) }
    fun last(endpoint: String) = sockets.last { it.url.contains(endpoint) }
}

class FakeScreenHost : ScreenHost {
    var requests = 0
    var tap = true
    var consent = Consent.NEEDED
    var fg = true
    var keyboard: JSONObject? = null
    override fun onScreenRequest() { requests++ }
    override fun canTap() = tap
    override fun consent() = consent
    override fun foreground() = fg
    override fun keyboard() = keyboard
}
