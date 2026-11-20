package com.allternit.feature.computer

import android.annotation.SuppressLint
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.webkit.JavascriptInterface
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiConfig
import com.allternit.core.auth.AllternitAuth
import com.allternit.feature.artifacts.ArtifactWeb
import org.json.JSONObject

internal fun openExternal(context: Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

/** iOS AppScreenTarget: the web app's `/editor/app-screen` page for one app, in the vendor's look. */
internal data class AppScreenTarget(val app: String, val name: String, val connected: Boolean = false, val account: String? = null, val install: String? = null) {
    fun url(dark: Boolean, full: Boolean): String {
        val b = Uri.parse(ArtifactWeb.ORIGIN).buildUpon().path("/editor/app-screen")
            .appendQueryParameter("app", app).appendQueryParameter("name", name).appendQueryParameter("theme", if (dark) "dark" else "light")
        if (connected) b.appendQueryParameter("connected", "1")
        if (!account.isNullOrEmpty()) b.appendQueryParameter("account", account)
        if (!install.isNullOrEmpty()) b.appendQueryParameter("install", install)
        if (full) b.appendQueryParameter("view", "full")
        return b.build().toString()
    }
}

/** The `window.webkit.messageHandlers` the page posts to, as one Android bridge (iOS allternitAppScreen + allternitClerk). */
private class AppScreenBridge(private val onMessage: (JSONObject) -> Unit) {
    private val main = Handler(Looper.getMainLooper())
    @JavascriptInterface fun post(json: String) {
        val o = runCatching { JSONObject(json) }.getOrNull() ?: return
        main.post { onMessage(o) }
    }
    @JavascriptInterface fun token(): String = AllternitAuth.tokenProvider.bearerToken().orEmpty()
}

private fun shim(api: String) = """
(function(){
  if (window.__allternitShim) return; window.__allternitShim = true;
  var B = window.AllternitBridge;
  window.webkit = { messageHandlers: {
    allternitAppScreen: { postMessage: function(m){ B.post(JSON.stringify(m)); } },
    allternitClerk: { postMessage: function(){ return Promise.resolve(B.token() || null); } }
  } };
  window.allternitAppApi = ${JSONObject.quote(api)};
  window.allternit = window.allternit || {};
  if (!window.allternit.auth) window.allternit.auth = { getClerkToken: function(){ return Promise.resolve(B.token() || null); } };
})();
"""

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun AppScreenWebView(url: String, reload: Int, modifier: Modifier, onMessage: (JSONObject) -> Unit, onFail: () -> Unit) {
    val context = LocalContext.current
    val api = remember { ApiConfig().cloudBaseUrl.toString().trimEnd('/') }
    val bridge = remember { AppScreenBridge(onMessage) }
    AndroidView(
        modifier = modifier,
        factory = {
            WebView(context).apply {
                setBackgroundColor(android.graphics.Color.TRANSPARENT)
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = true
                settings.mediaPlaybackRequiresUserGesture = false
                addJavascriptInterface(bridge, "AllternitBridge")
                webViewClient = object : WebViewClient() {
                    override fun onPageStarted(view: WebView, u: String?, favicon: android.graphics.Bitmap?) { view.evaluateJavascript(shim(api), null) }
                    override fun onPageFinished(view: WebView, u: String?) { view.evaluateJavascript(shim(api), null) }
                    /** Links out of a screen ("Open in Linear") go to the browser, never into the page. */
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        val host = request.url.host
                        if (request.hasGesture() && host != Uri.parse(ArtifactWeb.ORIGIN).host) { openExternal(context, request.url.toString()); return true }
                        return false
                    }
                    override fun onReceivedError(view: WebView, request: WebResourceRequest, error: android.webkit.WebResourceError) {
                        if (request.isForMainFrame) onFail()
                    }
                }
                tag = reload
                loadUrl(url)
            }
        },
        update = { v -> if (v.tag != reload) { v.tag = reload; v.loadUrl(url) } },
        onRelease = { it.removeJavascriptInterface("AllternitBridge"); it.destroy() },
    )
}

/** iOS AppScreenSection: the app's own screen, sized to its content; hidden when the app has none. */
@Composable
internal fun AppScreenSection(target: AppScreenTarget, onConnect: (() -> Unit)? = null) {
    val dark = isSystemInDarkTheme()
    var height by remember { mutableIntStateOf(0) }
    var hidden by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var full by remember { mutableStateOf(false) }
    var reload by remember { mutableIntStateOf(0) }
    if (hidden) return
    val c = com.allternit.core.design.AllternitTheme.colors
    Box(Modifier.fillMaxWidth().testTag("aciAppScreen")) {
        AppScreenWebView(
            url = target.url(dark, false), reload = reload,
            modifier = Modifier.fillMaxWidth().height(maxOf(height, 1).dp),
            onMessage = { m ->
                when (m.optString("type")) {
                    "ready" -> if (m.has("hasScreen") && !m.optBoolean("hasScreen", true)) hidden = true
                    "height" -> height = m.optDouble("height", 0.0).toInt()
                    "openFull" -> full = true
                    "connect" -> onConnect?.invoke()
                }
            },
            onFail = { failed = true },
        )
        if (failed) {
            Box(Modifier.fillMaxWidth().padding(vertical = 16.dp), contentAlignment = Alignment.Center) {
                androidx.compose.foundation.layout.Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("Couldn’t load ${target.name}’s screen.", color = c.textSecondary, fontSize = 13.sp)
                    AciPillButton("Try again", "aciAppScreenRetry") { failed = false; height = 0; reload += 1 }
                }
            }
        } else if (height == 0) {
            Box(Modifier.fillMaxWidth().heightIn(min = 120.dp), contentAlignment = Alignment.Center) { Text("Loading…", color = c.textTertiary) }
        }
    }
    if (full) {
        Dialog(onDismissRequest = { full = false }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            AciFullPage {
                AppScreenWebView(
                    url = target.url(dark, true), reload = 0, modifier = Modifier.fillMaxSize(),
                    onMessage = { m ->
                        when (m.optString("type")) {
                            "close" -> full = false
                            "connect" -> { full = false; onConnect?.invoke() }
                        }
                    },
                    onFail = {},
                )
            }
        }
    }
}

/** A plain in-app web page (iOS MiniAppRuntimeView's WebView). */
@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun AciPlainWebView(url: String, modifier: Modifier = Modifier) {
    AndroidView(modifier = modifier, factory = { ctx ->
        WebView(ctx).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.mediaPlaybackRequiresUserGesture = false
            webViewClient = WebViewClient()
            loadUrl(url)
        }
    }, onRelease = { it.destroy() })
}
