package com.allternit.feature.bots

import android.annotation.SuppressLint
import android.graphics.Color as AndroidColor
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import com.allternit.core.api.ApiJson
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.ArtifactHTML
import com.allternit.feature.artifacts.ArtifactWeb
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.coroutines.resume

// -----------------------------------------------------------------------------
// Create bot / Edit bot → Look (iOS CreateBot/BotLookEditor.swift). The look
// editor is the web's own AvatarEditor (Library, Describe, Your photo, Initials)
// at /editor/bot-look (allternit-ai src/pages/AppBotLookPage.tsx), so one editor
// serves Desktop, the web phone layout, iOS and this app. It is signed in like
// the artifact editors (window.allternit.auth + the bearer on trusted fetches).
// The page posts to window.webkit.messageHandlers.allternitBotLook; a
// document-start shim forwards those posts to the app's web message listener.
// -----------------------------------------------------------------------------

/** iOS BotLookModel: the page's phase, height and the avatar it reports. */
class BotLookModel {
    enum class Phase { Loading, Ready, Failed }

    var phase by mutableStateOf(Phase.Loading); private set
    var height by mutableIntStateOf(420); private set
    var avatar by mutableStateOf<JsonElement?>(null); private set
    /** The person changed the look (Edit keeps the saved avatar otherwise). */
    var dirty by mutableStateOf(false); private set
    /** A photo or character is being made. */
    var busy by mutableStateOf(false); private set
    /** Your photo / a drawn character is picked but not applied yet. */
    var blocked by mutableStateOf(false); private set
    var error by mutableStateOf<String?>(null)
    var reloadToken by mutableIntStateOf(0); private set
    internal var webView: WebView? = null
    private var initial: MutableMap<String, JsonElement> = mutableMapOf()
    private var inited = false

    /** What the page opens on. Set before it shows, and again for Retry. */
    fun configure(avatar: JsonElement?, accent: String, displayName: String, vendorId: String?) {
        initial = mutableMapOf("accent" to JsonPrimitive(accent), "displayName" to JsonPrimitive(displayName))
        avatar?.let { initial["avatar"] = it }
        vendorId?.let { initial["vendorId"] = JsonPrimitive(it) }
        if (phase == Phase.Ready && !inited) sendInit()
    }

    fun push(accent: String? = null, displayName: String? = null) {
        if (!inited) {
            accent?.let { initial["accent"] = JsonPrimitive(it) }
            displayName?.let { initial["displayName"] = JsonPrimitive(it) }
            return
        }
        call("update", buildJsonObject { accent?.let { put("accent", it) }; displayName?.let { put("displayName", it) } })
    }

    fun retry() {
        phase = Phase.Loading
        inited = false
        error = null
        reloadToken += 1
    }

    /** useCreateBotSubmit syncAvatarAsset: best effort, at most 3 s, never holds up the new bot. */
    suspend fun syncAsset(botId: String) {
        val web = webView ?: return
        if (!inited) return
        withTimeoutOrNull(3_000) {
            suspendCancellableCoroutine { cont ->
                web.evaluateJavascript("(function(){try{var p=window.allternitBotLook&&window.allternitBotLook.syncAsset(${ArtifactHTML.jsString(botId)});if(p&&p.then){p.then(function(){window.__allternitLookSynced=1;},function(){});}}catch(e){}return 1;})()") { if (cont.isActive) cont.resume(Unit) }
            }
            // The page's save is async: give it the rest of the 3 s to land.
            repeat(10) {
                var done = false
                suspendCancellableCoroutine { cont -> web.evaluateJavascript("window.__allternitLookSynced===1") { v -> done = v == "true"; if (cont.isActive) cont.resume(Unit) } }
                if (done) return@withTimeoutOrNull
                delay(250)
            }
        }
    }

    internal fun failed() { phase = Phase.Failed }

    internal fun handle(raw: String) {
        val o = runCatching { ApiJson.default.parseToJsonElement(raw) as? JsonObject }.getOrNull() ?: return
        when (o.str("type")) {
            "ready" -> { phase = Phase.Ready; sendInit() }
            "change" -> {
                o.str("avatarJson")?.let { t -> runCatching { ApiJson.default.parseToJsonElement(t) }.getOrNull()?.let { avatar = it } }
                dirty = (o["dirty"] as? JsonPrimitive)?.content == "true"
                busy = (o["busy"] as? JsonPrimitive)?.content == "true"
                blocked = (o["blocked"] as? JsonPrimitive)?.content == "true"
            }
            "height" -> (o["height"] as? JsonPrimitive)?.content?.toDoubleOrNull()?.takeIf { it > 0 }?.let { height = it.toInt() }
            "error" -> error = o.str("message")?.takeIf { it.isNotEmpty() }
        }
    }

    private fun sendInit() {
        inited = true
        call("init", JsonObject(initial))
    }

    private fun call(method: String, arg: JsonElement) {
        webView?.post { webView?.evaluateJavascript("window.allternitBotLook && window.allternitBotLook.$method($arg)", null) }
    }
}

private fun lookScript(): String {
    val api = LibraryNet.config.apiBaseUrl.toString().removeSuffix("/").removeSuffix("/api/v1")
    return """
    (function () {
      if (window.top !== window || typeof allternitLookApp === 'undefined') return;
      window.allternitAppApi = ${ArtifactHTML.jsString(api)};
      window.webkit = window.webkit || {};
      window.webkit.messageHandlers = window.webkit.messageHandlers || {};
      window.webkit.messageHandlers.allternitBotLook = { postMessage: function (m) { allternitLookApp.postMessage(JSON.stringify(m)); } };
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
      var trusted = [location.origin, ${ArtifactHTML.jsString(api)}];
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
    })();
    """.trimIndent()
}

/** The Look editor at the height the page reports, with its real loading and failed states. */
@SuppressLint("SetJavaScriptEnabled", "RequiresFeature")
@Composable
fun BotLookEditor(model: BotLookModel, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val dark = c.bgPrimary.luminance() < 0.5f
    val supported = WebViewFeature.isFeatureSupported(WebViewFeature.WEB_MESSAGE_LISTENER) && WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)
    LaunchedEffect(model.reloadToken) {
        if (!supported) { model.failed(); return@LaunchedEffect }
        // iOS watchdog: a page that never says "ready" in 30 s shows the failed state.
        delay(30_000)
        if (model.phase == BotLookModel.Phase.Loading) model.failed()
    }
    Box(modifier.fillMaxWidth().height(model.height.dp).testTag("botLookEditor"), contentAlignment = Alignment.Center) {
        if (supported) androidx.compose.runtime.key(model.reloadToken) {
            AndroidView(
                modifier = Modifier.fillMaxWidth().height(model.height.dp).alpha(if (model.phase == BotLookModel.Phase.Ready) 1f else 0f),
                factory = { ctx ->
                    WebView(ctx).apply {
                        settings.javaScriptEnabled = true
                        settings.domStorageEnabled = true
                        settings.allowFileAccess = false
                        settings.allowContentAccess = false
                        setBackgroundColor(AndroidColor.TRANSPARENT)
                        isVerticalScrollBarEnabled = false
                        val origins = setOf(ArtifactWeb.ORIGIN)
                        WebViewCompat.addWebMessageListener(this, "allternitClerk", origins) { _, _, _, isMainFrame, reply ->
                            if (!isMainFrame) return@addWebMessageListener
                            scope.launch {
                                val t = withContext(Dispatchers.IO) { runCatching { AllternitAuth.tokenProvider.bearerToken() }.getOrNull() }
                                reply.postMessage(buildJsonObject { put("token", t) }.toString())
                            }
                        }
                        WebViewCompat.addWebMessageListener(this, "allternitLookApp", origins) { _, msg, _, isMainFrame, _ ->
                            if (isMainFrame) msg.data?.let { model.handle(it) }
                        }
                        WebViewCompat.addDocumentStartJavaScript(this, lookScript(), origins)
                        webViewClient = object : WebViewClient() {
                            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                                if (request.isForMainFrame) model.failed()
                            }
                            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean = !ArtifactWeb.isAppPage(request.url)
                        }
                        model.webView = this
                        loadUrl("${ArtifactWeb.ORIGIN}/editor/bot-look?theme=${if (dark) "dark" else "light"}")
                    }
                },
                onRelease = { if (model.webView === it) model.webView = null; it.destroy() },
            )
        }
        when (model.phase) {
            BotLookModel.Phase.Loading -> Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                CircularProgressIndicator(color = c.textSecondary)
                Text("Loading looks…", color = c.textSecondary, fontSize = 13.sp)
            }
            BotLookModel.Phase.Failed -> Column(Modifier.padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Couldn’t load the looks. The bot keeps the default Gizzi look until they load.", color = c.textSecondary, fontSize = 13.sp, textAlign = TextAlign.Center)
                Pill("Try again", "botLookRetry") { model.retry() }
            }
            BotLookModel.Phase.Ready -> Unit
        }
    }
}
