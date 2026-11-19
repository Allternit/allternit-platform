package com.allternit.feature.artifacts

import android.annotation.SuppressLint
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.print.PrintAttributes
import android.print.PrintManager
import android.util.Base64
import android.view.ViewGroup
import android.webkit.RenderProcessGoneDetail
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.JavaScriptReplyProxy
import androidx.webkit.WebMessageCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import java.io.ByteArrayInputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * The three artifact web views (iOS SandboxedArtifactWebView, ArtifactPageWebView,
 * ArtifactLiveWebView) with the same rules:
 *  - no file or content:// access in any of them (iOS WKWebView has none by default);
 *  - the sandbox shows one in-memory document under a deny-all CSP, JS only for html/js types,
 *    every navigation and every sub-request refused;
 *  - the page view keeps the view on its page: a tapped http/https/mailto/tel link opens outside;
 *  - the live view loads only the web app's /a/, /editor/ and /embed/editor/ pages, and its
 *    bridge (allternitClerk, allternitEditor, allternitDownload) answers only the web app origin's
 *    main frame (androidx.webkit origin rules + isMainFrame), which is stricter than iOS was.
 * Platform-forced difference: Android WebView has one storage per app, so the iOS
 * non-persistent stores are approximated by turning DOM storage off in the sandbox and page views.
 */

/** Opens a link outside the app (iOS UIApplication.shared.open). */
internal fun openExternal(context: Context, url: String) {
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: ActivityNotFoundException) {
    }
}

/** Public form for :feature:projects (pad links and quick actions open outside, iOS "Opens in Safari"). */
fun openUrlExternally(context: Context, url: String) {
    val scheme = Uri.parse(url).scheme?.lowercase()
    if (scheme in externalSchemes) openExternal(context, url)
}

private val externalSchemes = setOf("http", "https", "mailto", "tel")

@SuppressLint("SetJavaScriptEnabled")
private fun WebView.lockDown(js: Boolean, domStorage: Boolean) {
    settings.javaScriptEnabled = js
    settings.allowFileAccess = false
    settings.allowContentAccess = false
    @Suppress("DEPRECATION")
    settings.allowFileAccessFromFileURLs = false
    @Suppress("DEPRECATION")
    settings.allowUniversalAccessFromFileURLs = false
    settings.domStorageEnabled = domStorage
    settings.setGeolocationEnabled(false)
    settings.mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
    settings.setSupportMultipleWindows(false)
    settings.javaScriptCanOpenWindowsAutomatically = false
    setBackgroundColor(android.graphics.Color.TRANSPARENT)
    layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
}

// MARK: - HTML helpers (iOS ArtifactHTML)

object ArtifactHTML {
    /** A JS string literal; `</` is escaped so it cannot close a script tag. */
    fun jsString(s: String): String = JsonPrimitive(s).toString().replace("</", "<\\/")

    fun escape(s: String): String = s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")

    private const val BASE_STYLE = ":root { color-scheme: light dark; } body { margin: 0; padding: 16px; " +
        "font: 16px/1.5 -apple-system, Roboto, system-ui, sans-serif; word-wrap: break-word; } img, svg { max-width: 100%; height: auto; } " +
        "pre { white-space: pre-wrap; } table { border-collapse: collapse; } td, th { border: 1px solid rgba(127,127,127,.35); padding: 4px 8px; }"

    fun page(body: String, head: String = ""): String =
        "<!doctype html><html><head><meta charset=\"utf-8\">" +
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">" +
            "<style>$BASE_STYLE</style>$head</head><body>$body</body></html>"

    /** marked + DOMPurify from jsDelivr (the same pinned versions iOS loads), plain text on failure. */
    fun markdown(md: String): String = page(
        "<div id=\"md\"></div><script>(function(){var src=${jsString(md)};var el=document.getElementById('md');" +
            "try{el.innerHTML=DOMPurify.sanitize(marked.parse(src));}catch(e){el.style.whiteSpace='pre-wrap';el.textContent=src;}})();</script>",
        "<script src=\"https://cdn.jsdelivr.net/npm/marked@12.0.2/marked.min.js\"></script>" +
            "<script src=\"https://cdn.jsdelivr.net/npm/dompurify@3.1.6/dist/purify.min.js\"></script>",
    )

    fun mermaid(src: String): String = page(
        "<pre class=\"mermaid\" id=\"m\"></pre><script>(function(){var s=${jsString(src)};var el=document.getElementById('m');el.textContent=s;" +
            "try{mermaid.initialize({startOnLoad:false,securityLevel:'strict'});mermaid.run({nodes:[el]});}catch(e){}})();</script>",
        "<script src=\"https://cdn.jsdelivr.net/npm/mermaid@10.9.1/dist/mermaid.min.js\"></script>",
    )

    fun svg(svg: String): String = page("<img alt=\"\" src=\"data:image/svg+xml;charset=utf-8,${Uri.encode(svg)}\">")

    fun image(url: String): String = page("<img alt=\"\" style=\"display:block;margin:0 auto\" src=\"${escape(url)}\">", "<style>body{padding:0}</style>")

    fun video(url: String): String = page(
        "<video controls playsinline style=\"width:100%\" src=\"${escape(url)}\"></video>",
        "<style>body{padding:0;background:#000}</style>",
    )
}

// MARK: - Sandboxed (chat-frame previews)

/** iOS SandboxedArtifactWebView: JS for these types only. */
internal fun sandboxAllowsJs(type: String): Boolean {
    val t = type.lowercase()
    return t.contains("html") || t.contains("javascript") || t == "js" || t == "jsx"
}

/** The document the sandbox shows, with the deny-all CSP (plus base-uri / form-action, an iOS fix). */
internal fun sandboxDocument(content: String, type: String): String {
    val js = sandboxAllowsJs(type)
    val csp = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none';" +
        if (js) " script-src 'unsafe-inline';" else ""
    val meta = "<meta http-equiv=\"Content-Security-Policy\" content=\"$csp\">"
    val t = type.lowercase()
    return when {
        t.contains("svg") -> "<!doctype html><html><head><meta charset=\"utf-8\">$meta" +
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">" +
            "<style>html,body{margin:0;height:100%;display:flex;align-items:center;justify-content:center;background:transparent}svg{max-width:100%;max-height:100%}</style>" +
            "</head><body>$content</body></html>"
        t.contains("html") -> {
            val headAt = Regex("<head[^>]*>", RegexOption.IGNORE_CASE).find(content)
            when {
                headAt != null -> content.replaceRange(headAt.range.last + 1, headAt.range.last + 1, meta)
                else -> "<!doctype html><html><head><meta charset=\"utf-8\">$meta</head><body>$content</body></html>"
            }
        }
        else -> "<!doctype html><html><head><meta charset=\"utf-8\">$meta" +
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">" +
            "<style>:root{color-scheme:light dark}body{margin:0;padding:16px}pre{font:13px/1.45 ui-monospace,monospace;white-space:pre-wrap;word-wrap:break-word}</style>" +
            "</head><body><pre>${ArtifactHTML.escape(content)}</pre></body></html>"
    }
}

@Composable
fun SandboxedArtifactWebView(content: String, artifactType: String, modifier: Modifier = Modifier) {
    val doc = remember(content, artifactType) { sandboxDocument(content, artifactType) }
    AndroidView(
        modifier = modifier.testTag("artifactSandboxWeb"),
        factory = { ctx ->
            WebView(ctx).apply {
                lockDown(js = sandboxAllowsJs(artifactType), domStorage = false)
                webViewClient = object : WebViewClient() {
                    // Every navigation is refused: the document is the only thing this view shows.
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest) = true

                    // Every sub-request is refused too (the CSP already blocks them; this is the backstop).
                    override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse =
                        WebResourceResponse("text/plain", "utf-8", 403, "Forbidden", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                }
                tag = doc
                loadDataWithBaseURL(null, doc, "text/html", "utf-8", null)
            }
        },
        update = { web ->
            if (web.tag != doc) {
                web.settings.javaScriptEnabled = sandboxAllowsJs(artifactType)
                web.tag = doc
                web.loadDataWithBaseURL(null, doc, "text/html", "utf-8", null)
            }
        },
        onRelease = { it.destroy() },
    )
}

// MARK: - Page (rendered markdown, mermaid, svg, html, https pages)

sealed interface PageSource {
    data class Html(val html: String) : PageSource
    data class Url(val url: String) : PageSource
}

/**
 * iOS ArtifactPageWebView. Tapped links open outside. Unlike iOS (which only cancelled tapped
 * links), every later main-frame navigation is refused, so a script or form cannot move the view.
 */
@Composable
fun ArtifactPageWebView(source: PageSource, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    AndroidView(
        modifier = modifier.testTag("artifactPageWeb"),
        factory = { ctx ->
            WebView(ctx).apply {
                lockDown(js = true, domStorage = source is PageSource.Url)
                webViewClient = object : WebViewClient() {
                    var settled = false
                    override fun onPageFinished(view: WebView, url: String?) { settled = true }
                    override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                        if (!request.isForMainFrame) return false
                        if (!settled && !request.hasGesture()) return false
                        val scheme = request.url.scheme?.lowercase()
                        if (request.hasGesture() && scheme in externalSchemes) openExternal(context, request.url.toString())
                        return true
                    }
                }
                load(this, source)
            }
        },
        update = { web -> if (web.tag != source) load(web, source) },
        onRelease = { it.destroy() },
    )
}

private fun load(web: WebView, source: PageSource) {
    web.tag = source
    when (source) {
        is PageSource.Html -> web.loadDataWithBaseURL(null, source.html, "text/html", "utf-8", null)
        is PageSource.Url -> if (source.url.startsWith("https://")) web.loadUrl(source.url)
    }
}

// MARK: - Live (the web app's artifact and editor pages)

/** What the live view loads: a viewer (`/a/<id>`, seeded) or an editor route. */
data class LiveRoute(val url: String, val noun: String, val seed: ArtifactV2? = null, val editor: Boolean = false)

private const val MAX_DOWNLOAD_BYTES = 512L * 1024 * 1024

/** The seed iOS ArtifactLiveSeed sends, so the page skips its public fetch. */
private fun seedJson(a: ArtifactV2): String = buildJsonObject {
    put("id", a.id); put("kind", a.kind); put("title", a.title); put("body", a.body); put("body_format", a.bodyFormat)
    put("meta", a.meta ?: JsonObject(emptyMap())); put("version", a.version); put("updated_at", a.summary.updatedAt)
    a.summary.owner.name?.let { put("owner_name", it) }
}.toString()

/**
 * Runs at document start in the web app's main frame (iOS signInScript / seedScript, the
 * AuthenticatedWebBridge fetch wrapper and the blob/data download hook), over the Android
 * web message listeners instead of window.webkit.messageHandlers.
 */
private fun liveScript(route: LiveRoute): String {
    val seed = route.seed
    val seedPart = if (seed != null) {
        val path = ArtifactHTML.jsString("/api/v2/public/artifacts/" + Uri.encode(seed.id))
        """
        var seed = ${seedJson(seed)}; var seedPath = $path;
        var origFetch0 = window.fetch;
        window.fetch = function (input, init) {
          try { var raw = typeof input === 'string' ? input : (input && input.url) || String(input);
            var u = new URL(raw, location.href);
            if (u.origin === location.origin && u.pathname === seedPath) {
              return Promise.resolve(new Response(JSON.stringify(seed), { status: 200, headers: { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' } }));
            } } catch (e) {}
          return origFetch0.apply(this, arguments);
        };
        function hideHeader() { var s = document.createElement('style'); s.textContent = 'main > header:first-child{display:none!important}'; (document.head || document.documentElement).appendChild(s); }
        if (document.documentElement) hideHeader(); else document.addEventListener('DOMContentLoaded', hideHeader);
        """.trimIndent()
    } else ""
    val apiBase = LibraryNet.config.apiBaseUrl.toString().removeSuffix("/").removeSuffix("/api/v1")
    return """
    (function () {
      if (window.top !== window || typeof allternitClerk === 'undefined') return;
      window.allternitAppApi = ${ArtifactHTML.jsString(apiBase)};
      var pending = {}, next = 1, cached = null, cachedAt = 0;
      allternitClerk.addEventListener('message', function (e) {
        try { var m = JSON.parse(e.data); var cb = pending[m.id]; if (cb) { delete pending[m.id]; cb(m.token || null); } } catch (x) {}
      });
      function token() {
        if (cached && Date.now() - cachedAt < 30000) return Promise.resolve(cached);
        return new Promise(function (res) { var id = next++; pending[id] = function (t) { cached = t; cachedAt = Date.now(); res(t); }; allternitClerk.postMessage(JSON.stringify({ id: id })); });
      }
      window.allternit = window.allternit || {};
      if (!window.allternit.auth) window.allternit.auth = { getClerkToken: token };
      document.addEventListener('DOMContentLoaded', function () {
        var v = document.querySelector('meta[name=viewport]');
        if (v) v.setAttribute('content', 'width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover');
      });
      var trusted = [location.origin, ${ArtifactHTML.jsString(apiBase)}];
      var origFetch = window.fetch;
      window.fetch = function (input, init) {
        try {
          var raw = typeof input === 'string' ? input : (input && input.url) || String(input);
          var u = new URL(raw, location.href);
          var h = new Headers((init && init.headers) || (input && input.headers) || {});
          if (trusted.indexOf(u.origin) >= 0 && !h.has('Authorization')) {
            var self = this, args = arguments;
            return token().then(function (t) {
              if (!t) return origFetch.apply(self, args);
              h.set('Authorization', 'Bearer ' + t);
              return origFetch.call(self, input, Object.assign({}, init || {}, { headers: h }));
            });
          }
        } catch (e) {}
        return origFetch.apply(this, arguments);
      };
      $seedPart
      if (typeof allternitDownload !== 'undefined') {
        document.addEventListener('click', function (ev) {
          var a = ev.target && ev.target.closest ? ev.target.closest('a[href]') : null;
          if (!a) return;
          var href = a.getAttribute('href') || '';
          if (href.indexOf('blob:') !== 0 && href.indexOf('data:') !== 0) return;
          ev.preventDefault();
          allternitDownload.postMessage(JSON.stringify({ stage: 'start' }));
          fetch(href).then(function (r) { return r.blob(); }).then(function (b) {
            if (b.size > $MAX_DOWNLOAD_BYTES) { allternitDownload.postMessage(JSON.stringify({ error: 'tooLarge' })); return; }
            var fr = new FileReader();
            fr.onload = function () { var s = String(fr.result); allternitDownload.postMessage(JSON.stringify({ name: a.getAttribute('download') || 'download', mime: b.type || 'application/octet-stream', data: s.slice(s.indexOf(',') + 1) })); };
            fr.onerror = function () { allternitDownload.postMessage(JSON.stringify({ error: 'read' })); };
            fr.readAsDataURL(b);
          }, function () { allternitDownload.postMessage(JSON.stringify({ error: 'read' })); });
        }, true);
        window.print = function () { allternitDownload.postMessage(JSON.stringify({ kind: 'print' })); };
      }
    })();
    """.trimIndent()
}

private sealed interface LivePhase {
    data object Loading : LivePhase
    data object Ready : LivePhase
    data class Failed(val message: String) : LivePhase
}

/**
 * iOS ArtifactLiveWebView: the web app page for an artifact or an editor. Editors may post
 * `{type:"close"}` and `{type:"chat", text}`. Downloads are saved where the person picks (SAF,
 * the Android form of the iOS share sheet). Three renderer crashes in a row show the iOS copy.
 */
@SuppressLint("RequiresFeature")
@Composable
fun ArtifactLiveWebView(
    route: LiveRoute,
    modifier: Modifier = Modifier,
    onClose: (() -> Unit)? = null,
    onChat: ((String) -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var phase by remember(route.url) { mutableStateOf<LivePhase>(LivePhase.Loading) }
    var reload by remember { mutableIntStateOf(0) }
    var crashes by remember(route.url) { mutableIntStateOf(0) }
    var preparing by remember { mutableStateOf(false) }
    var saveError by remember { mutableStateOf<String?>(null) }
    var pendingBytes by remember { mutableStateOf<ByteArray?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { uri ->
        val bytes = pendingBytes
        pendingBytes = null
        if (uri != null && bytes != null) {
            scope.launch {
                val ok = withContext(Dispatchers.IO) {
                    runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(bytes) } != null }.getOrDefault(false)
                }
                if (!ok) saveError = "The file couldn't be saved on this phone."
            }
        }
    }
    val dark = c.bgPrimary.luminance() < 0.5f
    val handlers = remember { LiveHandlers() }
    handlers.onClose = onClose
    handlers.onChat = onChat

    Box(modifier.fillMaxSize().testTag("artifactLiveWeb")) {
        androidx.compose.runtime.key(reload, crashes) {
            AndroidView(
                modifier = Modifier.fillMaxSize(),
                factory = { ctx ->
                    WebView(ctx).apply {
                        lockDown(js = true, domStorage = true)
                        settings.mediaPlaybackRequiresUserGesture = false
                        val origins = setOf(ArtifactWeb.ORIGIN)
                        if (WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER)) {
                            WebViewCompat.addWebMessageListener(this, "allternitClerk", origins) { _, _: WebMessageCompat, _, isMainFrame, reply: JavaScriptReplyProxy ->
                                if (!isMainFrame) return@addWebMessageListener
                                scope.launch {
                                    val t = withContext(Dispatchers.IO) { runCatching { AllternitAuth.tokenProvider.bearerToken() }.getOrNull() }
                                    reply.postMessage(buildJsonObject { put("token", t) }.toString())
                                }
                            }
                            if (route.editor) {
                                WebViewCompat.addWebMessageListener(this, "allternitEditor", origins) { _, msg, _, isMainFrame, _ ->
                                    if (!isMainFrame) return@addWebMessageListener
                                    val o = runCatching { LibraryJson.parse(msg.data ?: "") }.getOrNull()
                                    when (o.str("type")) {
                                        "close" -> handlers.onClose?.invoke()
                                        "chat" -> o.str("text")?.takeIf { it.isNotBlank() }?.let { handlers.onChat?.invoke(it) }
                                    }
                                }
                            }
                            WebViewCompat.addWebMessageListener(this, "allternitDownload", origins) { view, msg, _, isMainFrame, _ ->
                                if (!isMainFrame) return@addWebMessageListener
                                val o = runCatching { LibraryJson.parse(msg.data ?: "") }.getOrNull()
                                when {
                                    o.str("kind") == "print" -> printPage(context, view, route.noun)
                                    o.str("stage") == "start" -> preparing = true
                                    o.str("error") == "tooLarge" -> { preparing = false; saveError = "This file is too large to save on the phone. Download it on Desktop or the web." }
                                    o.str("error") != null -> { preparing = false; saveError = "The file couldn't be read. Try the export again." }
                                    o.str("data") != null -> {
                                        preparing = false
                                        val bytes = runCatching { Base64.decode(o.str("data"), Base64.DEFAULT) }.getOrNull()
                                        when {
                                            bytes == null || bytes.isEmpty() -> saveError = "The file couldn't be read. Try the export again."
                                            bytes.size > MAX_DOWNLOAD_BYTES -> saveError = "This file is too large to save on the phone. Download it on Desktop or the web."
                                            else -> {
                                                pendingBytes = bytes
                                                save.launch(safeFileName(o.str("name") ?: "download"))
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if (WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
                            WebViewCompat.addDocumentStartJavaScript(this, liveScript(route), origins)
                        }
                        setDownloadListener { url, _, _, _, _ -> if (url.startsWith("https://")) openExternal(context, url) }
                        webViewClient = object : WebViewClient() {
                            override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) {
                                if (phase !is LivePhase.Ready) phase = LivePhase.Loading
                            }
                            override fun onPageFinished(view: WebView, url: String?) { phase = LivePhase.Ready }
                            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                                if (request.isForMainFrame) phase = LivePhase.Failed(FriendlyErrorMessage.from(error.description?.toString() ?: "network error"))
                            }
                            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                                if (!request.isForMainFrame) return false
                                val url = request.url
                                if (ArtifactWeb.isAppPage(url) || url.toString() == "about:blank") return false
                                if (url.scheme?.lowercase() in externalSchemes) openExternal(context, url.toString())
                                return true
                            }
                            override fun onRenderProcessGone(view: WebView, detail: RenderProcessGoneDetail): Boolean {
                                (view.parent as? ViewGroup)?.removeView(view)
                                view.destroy()
                                if (crashes + 1 >= 3) {
                                    // Stop recreating: the third crash in a row shows the iOS copy until Retry.
                                    phase = LivePhase.Failed("This is too large to open on this phone right now. Your saved work is safe; try again, or open it on Desktop.")
                                } else {
                                    crashes += 1
                                }
                                return true
                            }
                        }
                        val sep = if (route.url.contains('?')) '&' else '?'
                        loadUrl(if (route.editor || route.url.contains("theme=")) route.url else "${route.url}${sep}theme=${if (dark) "dark" else "light"}")
                    }
                },
                onRelease = { it.destroy() },
            )
        }
        when (val p = phase) {
            LivePhase.Loading -> Column(
                Modifier.fillMaxSize().background(c.bgPrimary),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
            ) {
                CircularProgressIndicator(Modifier.size(24.dp).semantics { contentDescription = "Opening ${route.noun}" }, strokeWidth = 2.dp, color = c.textSecondary)
                Text("Opening ${route.noun}…", color = c.textSecondary, fontSize = 14.sp)
            }
            is LivePhase.Failed -> Box(Modifier.fillMaxSize().background(c.bgPrimary)) {
                FriendlyState(
                    style = FriendlyStyle.Error,
                    title = "Couldn't open this ${route.noun}",
                    message = p.message,
                    actionTitle = "Retry",
                    onAction = { crashes = 0; phase = LivePhase.Loading; reload += 1 },
                    modifier = Modifier.padding(horizontal = 20.dp).testTag("artifactLiveError"),
                ) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary) }
            }
            LivePhase.Ready -> Unit
        }
        if (preparing) {
            Text(
                "Preparing download…", color = c.textPrimary, fontSize = 13.sp,
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 24.dp)
                    .background(c.bgPanel, androidx.compose.foundation.shape.CircleShape).padding(horizontal = 14.dp, vertical = 8.dp),
            )
        }
        saveError?.let { msg ->
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { saveError = null },
                confirmButton = { androidx.compose.material3.TextButton(onClick = { saveError = null }) { Text("OK") } },
                title = { Text("Couldn't save the file") },
                text = { Text(msg) },
            )
        }
    }
    DisposableEffect(Unit) { onDispose { handlers.onClose = null; handlers.onChat = null } }
}

private class LiveHandlers {
    var onClose: (() -> Unit)? = null
    var onChat: ((String) -> Unit)? = null
}

/** iOS ArtifactFileShare name sanitising: no path separators or control characters. */
internal fun safeFileName(raw: String): String =
    raw.replace(Regex("[/\\\\:\\u0000-\\u001F]"), "_").trim().take(120).ifEmpty { "download" }

/** The page's print (iOS print sheet): Android's print dialog, which also saves a PDF. */
internal fun printPage(context: Context, web: WebView, title: String) {
    val pm = context.getSystemService(Context.PRINT_SERVICE) as? PrintManager ?: return
    pm.print(title, web.createPrintDocumentAdapter(title), PrintAttributes.Builder().build())
}

/** A tolerant JSON parse for bridge messages. */
internal object LibraryJson {
    fun parse(s: String): JsonObject? = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(s) as? JsonObject }.getOrNull()
}
