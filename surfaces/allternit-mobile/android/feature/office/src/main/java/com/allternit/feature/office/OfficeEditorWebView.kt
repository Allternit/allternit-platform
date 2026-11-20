package com.allternit.feature.office

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.view.ViewGroup
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.material3.Icon
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibrarySpinner
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Platform web root: the gateway serves the platform's static UI at its root, so strip `/api...` from the API base (iOS platformBaseURL). */
internal fun platformBaseUrl(): String {
    val base = LibraryNet.config.apiBaseUrl
    val port = if (base.port != okhttp3.HttpUrl.defaultPort(base.scheme)) ":${base.port}" else ""
    return "${base.scheme}://${base.host}$port"
}

/** The cloud API origin, the other origin whose calls get the bearer (iOS AppConfig.cloudAPIBaseURL). */
private fun cloudOrigin(): String {
    val base = LibraryNet.config.cloudBaseUrl
    val port = if (base.port != okhttp3.HttpUrl.defaultPort(base.scheme)) ":${base.port}" else ""
    return "${base.scheme}://${base.host}$port"
}

/** The handler name the editor page posts to for the bearer (iOS AuthenticatedWebBridge.handlerName). */
internal const val AUTH_HANDLER = "allternitAuth"

/**
 * The same bridge iOS installs: a document-start script wraps `fetch` and `XMLHttpRequest` so calls to the page's own
 * origin and the cloud API get `Authorization: Bearer <token>` (unless the page set one) and seeds
 * `localStorage.allternit_token`. The token comes from the app on demand over a reply-capable message handler, so it
 * stays fresh past a Clerk JWT's ~60 s life. Third-party origins never see it.
 */
internal fun authBridgeScript(trusted: List<String>): String {
    val originsJson = JsonArray(trusted.map { JsonPrimitive(it) }).toString()
    return """
(function () {
  var handler = window.$AUTH_HANDLER;
  if (!handler) return;
  var trusted = $originsJson;
  var cached = null, cachedAt = 0, waiting = [];
  handler.onmessage = function (e) {
    var t = null;
    try { t = JSON.parse(e.data).token || null; } catch (x) {}
    cached = t; cachedAt = Date.now();
    try { if (t) localStorage.setItem('allternit_token', t); } catch (x) {}
    var w = waiting; waiting = [];
    w.forEach(function (r) { r(t); });
  };
  function token() {
    if (cached && Date.now() - cachedAt < 30000) return Promise.resolve(cached);
    return new Promise(function (resolve) {
      waiting.push(resolve);
      if (waiting.length === 1) handler.postMessage('token');
      setTimeout(function () { var i = waiting.indexOf(resolve); if (i >= 0) { waiting.splice(i, 1); resolve(null); } }, 8000);
    });
  }
  function isTrusted(url) {
    try { var u = new URL(url, location.href); return u.origin === location.origin || trusted.indexOf(u.origin) >= 0; }
    catch (e) { return false; }
  }
  token();
  var origFetch = window.fetch;
  window.fetch = function (input, init) {
    var url = typeof input === 'string' ? input : (input && input.url) || String(input);
    if (!isTrusted(url)) return origFetch.apply(this, arguments);
    var self = this;
    return token().then(function (t) {
      if (!t) return origFetch.call(self, input, init);
      var opts = Object.assign({}, init || {});
      var headers = new Headers(opts.headers || (typeof input !== 'string' && input && input.headers) || {});
      if (!headers.has('Authorization')) headers.set('Authorization', 'Bearer ' + t);
      opts.headers = headers;
      return origFetch.call(self, input, opts);
    });
  };
  var origOpen = XMLHttpRequest.prototype.open;
  var origSend = XMLHttpRequest.prototype.send;
  var origSet = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__allternitTrusted = isTrusted(url); this.__allternitHasAuth = false;
    return origOpen.apply(this, arguments);
  };
  XMLHttpRequest.prototype.setRequestHeader = function (name) {
    if (String(name).toLowerCase() === 'authorization') this.__allternitHasAuth = true;
    return origSet.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    if (this.__allternitTrusted && !this.__allternitHasAuth && cached) origSet.call(this, 'Authorization', 'Bearer ' + cached);
    return origSend.apply(this, arguments);
  };
})();
""".trimIndent()
}

/**
 * Interactive office editor (iOS OfficeEditorWebView): loads the platform surface's editor route
 * (`/docs|/sheets|/slides|/pdf/:artifactId`) for a record in the sectioned artifact service. The first navigation carries
 * the Bearer; every request the page makes is signed by [authBridgeScript].
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun OfficeEditorWebView(family: OfficeFamily, artifactId: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var attempt by remember { mutableIntStateOf(0) }
    var failed by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(true) }
    val url = platformBaseUrl() + family.editorPath + "/" + LibraryNet.escape(artifactId)

    Box(modifier.fillMaxSize().testTag("officeEditorWeb")) {
        if (failed) {
            FriendlyState(
                FriendlyStyle.Offline, "The editor didn't load", "Check your connection and try again.",
                actionTitle = "Retry", onAction = { failed = false; loading = true; attempt += 1 },
            ) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary) }
        } else {
            androidx.compose.runtime.key(attempt) {
                AndroidView(
                    modifier = Modifier.fillMaxSize(),
                    factory = { ctx ->
                        WebView(ctx).apply {
                            settings.javaScriptEnabled = true
                            settings.domStorageEnabled = true
                            settings.allowFileAccess = false
                            settings.allowContentAccess = false
                            settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
                            settings.setSupportMultipleWindows(false)
                            layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
                            val origins = setOf(platformBaseUrl())
                            if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
                                WebViewCompat.addWebMessageListener(this, AUTH_HANDLER, origins) { _, _: WebMessageCompat, _, isMainFrame, reply: JavaScriptReplyProxy ->
                                    if (!isMainFrame) return@addWebMessageListener
                                    scope.launch {
                                        val t = withContext(Dispatchers.IO) { runCatching { AllternitAuth.tokenProvider.bearerToken() }.getOrNull() }
                                        reply.postMessage(buildJsonObject { put("token", t) }.toString())
                                    }
                                }
                            }
                            if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
                                WebViewCompat.addDocumentStartJavaScript(this, authBridgeScript(listOf(platformBaseUrl(), cloudOrigin())), origins)
                            }
                            webViewClient = object : WebViewClient() {
                                override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) { loading = true }
                                override fun onPageFinished(view: WebView, url: String?) { loading = false }
                                override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                                    if (request.isForMainFrame) { android.util.Log.w("OfficeEditorWebView", "navigation failed: ${error.description}"); failed = true }
                                }
                            }
                            // The first navigation carries the Bearer (iOS Coordinator.load).
                            scope.launch {
                                val t = withContext(Dispatchers.IO) { runCatching { AllternitAuth.tokenProvider.bearerToken() }.getOrNull() }
                                loadUrl(url, if (t != null) mapOf("Authorization" to "Bearer $t") else emptyMap())
                            }
                        }
                    },
                    onRelease = { it.removeAllViews(); it.destroy() },
                )
            }
            if (loading) Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LibrarySpinner("Opening the editor…") }
        }
    }
}
