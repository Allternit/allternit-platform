package com.allternit.feature.code

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.webkit.MimeTypeMap
import android.webkit.WebResourceRequest
import android.webkit.WebResourceResponse
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import java.io.ByteArrayInputStream

/*
 * Dev servers (iOS DevServerPortsView, DevServerPreviewView): the ports a shell's processes
 * listen on, and a preview of one in a WebView. iOS loads the relay URL in a WKWebView; Android's
 * WebView can't send a request through the relay envelope, so every request the page makes is
 * answered by shouldInterceptRequest, fetched through the relay (ChatBackend.runtimeBytes).
 */

private const val PREVIEW_HOST = "https://preview.allternit.invalid"

@Composable
internal fun DevServerPortsScreen(backend: ChatBackend, ptyId: String, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var preview by remember { mutableStateOf<Pair<Int, String>?>(null) }
    var minting by remember { mutableStateOf<Int?>(null) }
    var mintError by remember { mutableStateOf<String?>(null) }
    val open = preview
    if (open != null) {
        DevServerPreviewScreen(backend, ptyId, open.first, open.second, onBack = { preview = null })
        return
    }
    CodeScaffold(title = "Dev servers", subtitle = "Ports your shell is serving", onBack = onBack, backTag = "codePortsBack") {
        CodeLoader(key = ptyId, failedTitle = "Couldn't load ports", load = { CodeApi.ports(backend, ptyId) }) { ports, reload ->
            Column(Modifier.fillMaxSize()) {
                mintError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(16.dp).testTag("codePortsError")) }
                if (ports.isEmpty()) {
                    EmptyState("No dev servers detected", "Start something like `npm run dev` in the terminal — it'll show up here.")
                    CodePill("Refresh", testTag = "codePortsRefresh", onClick = reload)
                } else {
                    LazyColumn(Modifier.fillMaxWidth().testTag("codePortsList")) {
                        items(ports, key = { it.port }) { p ->
                            CodeRow(
                                title = p.command ?: "Unknown process",
                                testTag = "codePortRow-${p.port}",
                                trailing = { Text(":${p.port}", color = c.textSecondary, fontSize = 14.sp, fontFamily = CodeMono) },
                                onClick = {
                                    if (minting == null) scope.launch {
                                        minting = p.port
                                        mintError = null
                                        when (val r = CodeApi.mintPreviewToken(backend, ptyId, p.port)) {
                                            is CodeResult.Ok -> preview = p.port to r.value
                                            CodeResult.NoComputer -> mintError = "Connect a computer to open the preview."
                                            is CodeResult.Failed -> mintError = r.message
                                        }
                                        minting = null
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
internal fun DevServerPreviewScreen(backend: ChatBackend, ptyId: String, port: Int, token: String, onBack: () -> Unit) {
    var reloads by remember { mutableIntStateOf(0) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(true) }
    val startUrl = "$PREVIEW_HOST/v1/pty/${encode(ptyId)}/preview/$port/?token=${encode(token)}"
    CodeScaffold(
        title = ":$port",
        subtitle = "Preview",
        onBack = onBack,
        backTag = "codePreviewClose",
        trailing = { CodePill("Refresh", testTag = "codePreviewRefresh") { loadError = null; reloads++ } },
    ) {
        loadError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(16.dp).testTag("codePreviewError")) }
        AndroidView(
            modifier = Modifier.fillMaxSize().testTag("codePreviewWeb"),
            factory = { ctx ->
                WebView(ctx).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    webViewClient = object : WebViewClient() {
                        override fun onPageStarted(view: WebView, url: String?, favicon: Bitmap?) { loading = true }
                        override fun onPageFinished(view: WebView, url: String?) { loading = false }
                        override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse? {
                            val url = request.url
                            if (!url.toString().startsWith(PREVIEW_HOST)) {
                                // Anything off the dev server is not ours to fetch through the relay.
                                return WebResourceResponse("text/plain", "utf-8", 403, "Blocked", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                            }
                            val path = url.encodedPath.orEmpty() + (url.encodedQuery?.let { "?$it" } ?: "")
                            val result = runBlocking { backend.runtimeBytes(path, "*/*") }
                            if (result == null) {
                                loadError = "Your computer didn't answer. Check it is online, then refresh."
                                return WebResourceResponse("text/plain", "utf-8", 502, "Bad Gateway", emptyMap(), ByteArrayInputStream(ByteArray(0)))
                            }
                            val (status, bytes) = result
                            val ext = MimeTypeMap.getFileExtensionFromUrl(url.lastPathSegment.orEmpty())
                            val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "text/html"
                            val code = if (status in 100..599 && status !in 300..399) status else 200
                            return WebResourceResponse(mime, "utf-8", code, if (code == 200) "OK" else "Error", emptyMap(), ByteArrayInputStream(bytes))
                        }
                    }
                    loadUrl(startUrl)
                }
            },
            update = { web -> if (reloads > 0 && web.tag != reloads) { web.tag = reloads; web.loadUrl(startUrl) } },
        )
    }
}
