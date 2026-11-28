package com.allternit.feature.computer

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
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
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
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
 * The VM runs in a WebView that loads https://box.allternit.com/ as the top-level page. The contract's frame
 * protocol (version 1 JSON messages) is carried over an androidx.webkit message listener instead of window.postMessage
 * between a parent and an iframe, because a WebView has no parent window. A document-start script, scoped to the box
 * origin only, makes the unchanged page work at top level:
 *  - frame to parent: `window.parent` is `window` here, so the script wraps `window.postMessage` and sends every
 *    `{v:1}` message to the app instead (the page's own `parent.postMessage(msg, origin)` would otherwise drop it);
 *  - parent to frame: app messages are re-dispatched as a MessageEvent with `origin` m.allternit.com (on the page's
 *    allowlist, section 6.3) and `source` `window` (equal to `window.parent` at top level).
 * The page's origin allowlist and locked-origin rules are not weakened; the app is simply the "m.allternit.com" parent.
 */
internal const val BOX_ORIGIN = "https://box.allternit.com"
private const val BOX_HANDLER = "allternitBox"
private const val PARENT_ORIGIN = "https://m.allternit.com"
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
internal const val CRASHED_MESSAGE = "The browser computer stopped. Restart it to continue."

/** Document-start script for the box origin only. The handler object is the message listener installed below. */
internal fun boxBridgeScript(): String = """
(function () {
  var handler = window.$BOX_HANDLER;
  if (!handler || window.__allternitBoxShim) return;
  window.__allternitBoxShim = true;
  var nativePost = window.postMessage.bind(window);
  window.postMessage = function (msg, target) {
    if (msg && typeof msg === 'object' && msg.v === 1) { handler.postMessage(JSON.stringify(msg)); return; }
    return nativePost.apply(window, arguments);
  };
  handler.onmessage = function (e) {
    var data; try { data = JSON.parse(e.data); } catch (x) { return; }
    window.dispatchEvent(new MessageEvent('message', { data: data, origin: '$PARENT_ORIGIN', source: window }));
  };
  handler.postMessage(JSON.stringify({ v: 1, kind: 'evt', event: 'shim.ready', data: {} }));
})();
""".trimIndent()

/** Holds one WebView and speaks the section 6 protocol over it. Create with [rememberBrowserComputerHost]. */
internal class BrowserComputerHost(private val context: Context) {
    var hostState by mutableStateOf(BoxHostState.Probing); private set
    var status by mutableStateOf(BoxStatus()); private set
    /** A failure the UI should show in plain words (a failed page load). */
    var message by mutableStateOf<String?>(null); private set
    /** Bumped on a restart so the Compose factory builds a new WebView. */
    var generation by mutableStateOf(0); private set

    private var webView: WebView? = null
    private var reply: JavaScriptReplyProxy? = null
    private val pending = HashMap<String, CompletableDeferred<JsonObject>>()
    private val queue = ArrayList<String>()
    private val json = Json { ignoreUnknownKeys = true }
    /** pty id to the listener that receives its output; the UI sets one for its terminal. */
    var onPtyData: ((ptyId: String, bytes: ByteArray) -> Unit)? = null
    var onPtyExit: ((ptyId: String, code: Int) -> Unit)? = null

    fun restart() {
        failPending("host_error", CRASHED_MESSAGE)
        reply = null
        queue.clear()
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
        val origins = setOf(BOX_ORIGIN)
        val bridgeOk = WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) &&
            WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)
        if (bridgeOk) {
            WebViewCompat.addWebMessageListener(this, BOX_HANDLER, origins) { _, message: WebMessageCompat, sourceOrigin, isMainFrame, proxy ->
                if (!isMainFrame || sourceOrigin.toString() != BOX_ORIGIN) return@addWebMessageListener
                reply = proxy
                message.data?.let { receive(it) }
                if (queue.isNotEmpty()) { queue.toList().forEach { proxy.postMessage(it) }; queue.clear() }
            }
            WebViewCompat.addDocumentStartJavaScript(this, boxBridgeScript(), origins)
        }
        webViewClient = object : WebViewClient() {
            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                if (request.isForMainFrame) { message = "The browser computer could not load. Check the connection and restart it."; hostState = BoxHostState.Failed }
            }
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean =
                request.url.toString().let { !it.startsWith("$BOX_ORIGIN/") && it != BOX_ORIGIN } // never leave the box origin
            override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                // The renderer was killed (memory) or crashed. Drop this WebView; the UI offers a restart.
                failPending("host_error", CRASHED_MESSAGE)
                reply = null
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
            if (r == "true" && bridgeOk) { hostState = BoxHostState.Loading; loadUrl("$BOX_ORIGIN/") }
            else { hostState = BoxHostState.Unsupported; message = NO_JSPI_MESSAGE }
        }
    }

    fun destroy(view: WebView) {
        if (webView === view) webView = null
        reply = null
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
                    "shim.ready" -> if (hostState == BoxHostState.Loading) hostState = BoxHostState.Ready
                }
            }
        }
    }

    private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull

    private fun send(obj: JsonObject) {
        val text = obj.toString()
        val proxy = reply
        if (proxy != null) proxy.postMessage(text) else queue.add(text)
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

    suspend fun exec(cmd: String, args: List<String>, timeoutMs: Long = 60_000): JsonObject = call(
        "exec",
        buildJsonObject {
            put("cmd", cmd)
            put("args", kotlinx.serialization.json.JsonArray(args.map { JsonPrimitive(it) }))
            put("cwd", "/root")
            put("stdin", JsonNull)
            put("timeout_ms", timeoutMs)
        },
        timeoutMs = timeoutMs + 5_000,
    )
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
