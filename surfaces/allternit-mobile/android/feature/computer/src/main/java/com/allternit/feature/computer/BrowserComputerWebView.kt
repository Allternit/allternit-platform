package com.allternit.feature.computer

import android.annotation.SuppressLint
import android.content.Context
import com.allternit.feature.chat.BoxExecParams
import com.allternit.feature.chat.BoxExecResult
import com.allternit.feature.chat.BrowserComputerExecutor
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.webkit.JavascriptInterface
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import java.util.UUID
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.delay
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/**
 * The browser computer on Android (iOS BrowserComputerWebView, plan step 74/76; CONTRACT-colibri-unkernel section 6).
 *
 * The VM runs in a WebView that loads https://box.allternit.com/ as the top-level page. The box page has an explicit
 * native host mode (ai PR "native host transport for box RPC"): when `window.AllternitBoxNative` exists it posts every
 * version 1 JSON message to it, and the app answers by calling `window.__allternitBoxReceive(message)`. The app is the
 * parent peer; there is no injected script and no faked origin. Same message shapes as the frame path.
 * The page must ship that mode before this works on a device.
 */
internal const val BOX_ORIGIN = "https://box.allternit.com"
private const val BOX_HANDLER = "AllternitBoxNative"
private const val WEBVIEW_PLAY_URL = "market://details?id=com.google.android.webview"
private const val WEBVIEW_PLAY_WEB = "https://play.google.com/store/apps/details?id=com.google.android.webview"

/** What the host is doing, one plain state for the UI. */
internal enum class BoxHostState { Probing, Unsupported, Loading, Ready, Crashed, Failed }

/** The frame's last `status` data (contract 6.5). */
internal data class BoxStatus(
    val state: String = "idle",
    val computerId: String? = null,
    val loaded: Long? = null,
    val total: Long? = null,
    val diskBytesUsed: Long? = null,
    val persisted: Boolean? = null,
    val error: String? = null,
)

internal class BoxRpcError(val code: String, override val message: String) : Exception(message)

internal const val NO_JSPI_MESSAGE = "The browser computer needs an updated Android System WebView."
internal const val CRASHED_MESSAGE = "The computer stopped because the phone ran low on memory. Start it again."

/** The JavascriptInterface the box page calls in native host mode. Calls arrive on a WebView thread. */
internal class BoxNativeBridge(private val onMessage: (String) -> Unit) {
    @JavascriptInterface
    fun postMessage(json: String) = onMessage(json)
}

/** Holds one WebView and speaks the section 6 protocol over it. Create with [rememberBrowserComputerHost]. */
internal class BrowserComputerHost(private val context: Context) : BrowserComputerExecutor {
    var hostState by mutableStateOf(BoxHostState.Probing); private set
    var status by mutableStateOf(BoxStatus()); private set
    /** A failure the UI should show in plain words (a failed page load). */
    var message by mutableStateOf<String?>(null); private set
    /** Bumped on a restart so the Compose factory builds a new WebView. */
    var generation by mutableStateOf(0); private set

    private var webView: WebView? = null
    private val main = Handler(Looper.getMainLooper())
    private val pending = HashMap<String, CompletableDeferred<JsonObject>>()
    private val json = Json { ignoreUnknownKeys = true }
    /** pty id to the listener that receives its output; the UI sets one for its terminal. */
    var onPtyData: ((ptyId: String, bytes: ByteArray) -> Unit)? = null
    var onPtyExit: ((ptyId: String, code: Int) -> Unit)? = null

    fun restart() {
        failPending("host_error", CRASHED_MESSAGE)
        status = BoxStatus()
        message = null
        hostState = BoxHostState.Probing
        generation++
    }

    fun openWebViewUpdate() {
        val play = Intent(Intent.ACTION_VIEW, Uri.parse(WEBVIEW_PLAY_URL)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val web = Intent(Intent.ACTION_VIEW, Uri.parse(WEBVIEW_PLAY_WEB)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { context.startActivity(play) }.onFailure { runCatching { context.startActivity(web) } }
    }

    @SuppressLint("SetJavaScriptEnabled")
    internal fun createWebView(ctx: Context): WebView = WebView(ctx).apply {
        webView = this
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true // IndexedDB and localStorage persist in the app's WebView profile (the disk).
        settings.allowFileAccess = false
        settings.allowContentAccess = false
        settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
        settings.setSupportMultipleWindows(false)
        layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        val view = this
        addJavascriptInterface(BoxNativeBridge { raw ->
            // WebView thread: only the box origin's top-level page is a peer, then hop to main for state.
            main.post { if (webView === view && view.url.orEmpty().let { it == BOX_ORIGIN || it.startsWith("$BOX_ORIGIN/") }) receive(raw) }
        }, BOX_HANDLER)
        webViewClient = object : WebViewClient() {
            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                if (request.isForMainFrame) { message = "The browser computer could not load. Check the connection and restart it."; hostState = BoxHostState.Failed }
            }
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
                request.url.toString().let { !it.startsWith("$BOX_ORIGIN/") && it != BOX_ORIGIN } // never leave the box origin
            override fun onPageFinished(view: WebView, url: String?) {
                // The native handler exists from page start; the hello retry covers a page that is not listening yet.
                if (hostState == BoxHostState.Loading) hostState = BoxHostState.Ready
            }
            override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                // The renderer was killed (memory) or crashed. Drop this WebView; the UI offers a restart.
                failPending("host_error", CRASHED_MESSAGE)
                hostState = BoxHostState.Crashed
                message = CRASHED_MESSAGE
                (view.parent as? ViewGroup)?.removeView(view)
                view.destroy()
                if (webView === view) webView = null
                return true
            }
        }
        // Capability probe first (JSPI): about:blank already has a JS context. Only a capable WebView loads the box page.
        evaluateJavascript("typeof WebAssembly === 'object' && typeof WebAssembly.Suspending === 'function'") { r ->
            if (webView !== this || hostState == BoxHostState.Crashed) return@evaluateJavascript
            if (r == "true") { hostState = BoxHostState.Loading; loadUrl("$BOX_ORIGIN/") }
            else { hostState = BoxHostState.Unsupported; message = NO_JSPI_MESSAGE }
        }
    }

    fun destroy(view: WebView) {
        if (webView === view) webView = null
        failPending("host_error", CRASHED_MESSAGE)
        view.destroy()
    }

    private fun failPending(code: String, text: String) {
        val err = buildJsonObject { put("v", 1); put("kind", "res"); put("ok", false); put("error", buildJsonObject { put("code", code); put("message", text) }) }
        pending.values.toList().forEach { it.complete(err) }
        pending.clear()
    }

    private fun receive(raw: String) {
        val obj = runCatching { json.parseToJsonElement(raw).jsonObject }.getOrNull() ?: return
        if (obj["v"]?.jsonPrimitive?.contentOrNull != "1") return
        when (obj["kind"]?.jsonPrimitive?.contentOrNull) {
            "res" -> obj["id"]?.jsonPrimitive?.contentOrNull?.let { pending.remove(it)?.complete(obj) }
            "evt" -> {
                val data = obj["data"] as? JsonObject
                when (obj["event"]?.jsonPrimitive?.contentOrNull) {
                    "status" -> if (data != null) {
                        val progress = data["progress"] as? JsonObject
                        status = BoxStatus(
                            state = data.str("state") ?: "idle",
                            computerId = data.str("computer_id"),
                            loaded = progress?.get("loaded")?.jsonPrimitive?.contentOrNull?.toLongOrNull(),
                            total = progress?.get("total")?.jsonPrimitive?.contentOrNull?.toLongOrNull(),
                            diskBytesUsed = data["disk_bytes_used"]?.jsonPrimitive?.contentOrNull?.toLongOrNull(),
                            persisted = data["persisted"]?.jsonPrimitive?.booleanOrNull,
                            error = (data["error"] as? JsonObject)?.str("message"),
                        )
                        if (hostState == BoxHostState.Loading) hostState = BoxHostState.Ready
                    }
                    "pty.data" -> if (data != null) {
                        val id = data.str("pty_id")
                        val b64 = data.str("data_b64")
                        if (id != null && b64 != null) runCatching { android.util.Base64.decode(b64, android.util.Base64.DEFAULT) }.getOrNull()?.let { onPtyData?.invoke(id, it) }
                    }
                    "pty.exit" -> if (data != null) onPtyExit?.invoke(data.str("pty_id").orEmpty(), data["exit_code"]?.jsonPrimitive?.contentOrNull?.toIntOrNull() ?: 0)
                }
            }
        }
    }

    private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull

    private fun send(obj: JsonObject) {
        // A message sent before the page installed __allternitBoxReceive is dropped; hello retries cover that.
        val js = "window.__allternitBoxReceive && window.__allternitBoxReceive(${obj})"
        main.post { webView?.evaluateJavascript(js, null) }
    }

    /** One request and its response. A frame error becomes a [BoxRpcError] with the contract's code. */
    suspend fun call(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 30_000): JsonObject {
        val id = UUID.randomUUID().toString()
        val done = CompletableDeferred<JsonObject>()
        pending[id] = done
        send(buildJsonObject { put("v", 1); put("kind", "req"); put("id", id); put("method", method); put("params", params) })
        val res = withTimeoutOrNull(timeoutMs) { done.await() }
        if (res == null) { pending.remove(id); throw BoxRpcError("timeout", "The browser computer did not answer in time.") }
        if (res["ok"]?.jsonPrimitive?.booleanOrNull != true) {
            val e = res["error"] as? JsonObject
            throw BoxRpcError(e?.str("code") ?: "host_error", e?.str("message") ?: "The browser computer reported an error.")
        }
        return (res["result"] as? JsonObject) ?: JsonObject(emptyMap())
    }

    /** The section 6.3 handshake: `hello` every 250 ms until answered or 10 s pass. */
    suspend fun hello(): JsonObject {
        val deadline = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < deadline) {
            if (hostState == BoxHostState.Crashed || hostState == BoxHostState.Unsupported || hostState == BoxHostState.Failed) break
            if (hostState == BoxHostState.Loading || hostState == BoxHostState.Probing) { delay(250); continue }
            runCatching { return call("hello", buildJsonObject { put("app", "m.allternit.com") }, timeoutMs = 250) }
        }
        throw BoxRpcError("not_ready", message ?: "The browser computer did not start.")
    }

    /** Contract 6.2 `exec`: `{cmd, args, cwd, stdin, timeout_ms}`. [stdin] is base64 text, or null for none. */
    suspend fun exec(cmd: String, args: List<String>, timeoutMs: Long = 60_000, cwd: String? = "/root", stdin: String? = null): JsonObject = call(
        "exec",
        buildJsonObject {
            put("cmd", cmd)
            put("args", kotlinx.serialization.json.JsonArray(args.map { JsonPrimitive(it) }))
            put("cwd", cwd?.let { JsonPrimitive(it) } ?: JsonNull)
            put("stdin", stdin?.let { JsonPrimitive(it) } ?: JsonNull)
            put("timeout_ms", timeoutMs)
        },
        timeoutMs = timeoutMs + 5_000,
    )

    override suspend fun exec(params: BoxExecParams): BoxExecResult {
        val r = exec(params.cmd, params.args, params.timeoutMs, params.cwd, params.stdin)
        return BoxExecResult(
            stdout = r.str("stdout").orEmpty(),
            stderr = r.str("stderr").orEmpty(),
            exitCode = r["exit_code"]?.jsonPrimitive?.contentOrNull?.toIntOrNull() ?: -1,
            timedOut = r["timed_out"]?.jsonPrimitive?.booleanOrNull == true,
        )
    }
}

@Composable
internal fun rememberBrowserComputerHost(): BrowserComputerHost {
    val context = androidx.compose.ui.platform.LocalContext.current.applicationContext
    return remember { BrowserComputerHost(context) }
}

/**
 * The WebView itself. It draws nothing the user needs (the page is the VM host, the app draws the UI), so callers give it
 * a 1 dp strip. Recreated when the host restarts after a renderer loss.
 */
@Composable
internal fun BrowserComputerWebView(host: BrowserComputerHost, modifier: Modifier = Modifier) {
    androidx.compose.runtime.key(host.generation) {
        var view by remember { mutableStateOf<WebView?>(null) }
        DisposableEffect(Unit) { onDispose { view?.let { host.destroy(it) } } }
        if (host.hostState != BoxHostState.Crashed) {
            AndroidView(modifier = modifier, factory = { ctx -> host.createWebView(ctx).also { view = it } })
        }
    }
}
