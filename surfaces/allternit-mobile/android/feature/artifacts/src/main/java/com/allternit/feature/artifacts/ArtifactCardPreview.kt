package com.allternit.feature.artifacts

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.pdf.PdfRenderer
import android.media.MediaMetadataRetriever
import android.os.ParcelFileDescriptor
import android.util.Base64
import android.view.View
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import java.io.File
import kotlin.coroutines.resume
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/*
 * Library card previews (iOS ArtifactCardPreview + ArtifactCardContent.decode). The list has no
 * bodies, so each card fetches its record (3 at a time) and draws by kind: a 460dp paper scaled to
 * the card for docs, sheets (6×12), slides (400×225), code (16 lines) and dashboards (6 tiles);
 * a 1000×625 snapshot of design / page / card HTML (scripts off) and mermaid (scripts on); a
 * 640 px thumbnail for images, page 1 of a PDF at 480 px, a video's first frame, and the web
 * app's poster page for motion and ops. Anything else: thumbnail_url, else the kind glyph.
 */

internal sealed interface CardContent {
    data class Lines(val lines: List<String>) : CardContent
    data class Grid(val rows: List<List<String>>) : CardContent
    data class Slide(val lines: List<String>) : CardContent
    data class Code(val lines: List<String>) : CardContent
    data class Dashboard(val titles: List<String>) : CardContent
    data class Picture(val image: ImageBitmap) : CardContent
    data object Art : CardContent
}

private object CardCache {
    val content = mutableStateMapOf<String, CardContent>()
    val fetch = Semaphore(3)
    val snapshot = Mutex()
}

// MARK: - Media (iOS ArtifactMediaLoader)

/** data: inline; `/…` on the API host (`/api/v1/`) or the cloud host, with the bearer; other https without it. */
internal fun mediaBytes(ref: String, cap: Long = 64L * 1024 * 1024): ByteArray? = runCatching {
    when {
        ref.startsWith("data:") -> Base64.decode(ref.substringAfter(','), Base64.DEFAULT)
        ref.startsWith("/") -> LibraryNet.okHttp.newCall(okhttp3.Request.Builder().url(LibraryNet.absolute(ref)).build()).execute().use { r ->
            if (r.isSuccessful) r.body?.let { b -> if (b.contentLength() > cap) null else b.bytes() } else null
        }
        ref.startsWith("https://") || ref.startsWith("http://") -> plainClient.newCall(okhttp3.Request.Builder().url(ref).build()).execute().use { r ->
            if (r.isSuccessful) r.body?.let { b -> if (b.contentLength() > cap) null else b.bytes() } else null
        }
        else -> null
    }
}.getOrNull()

private val plainClient by lazy { okhttp3.OkHttpClient() }

internal fun firstUri(body: String): String? = body.lines().map { it.trim() }.firstOrNull { it.isNotEmpty() && !it.startsWith("#") }

/** iOS videoURL: `meta.media.url`, else a body that is the footage URL. */
internal fun videoUrl(body: String, meta: JsonObject?): String? {
    meta.child("media").str("url")?.takeIf { it.isNotEmpty() }?.let { return it }
    val u = firstUri(body) ?: return null
    if (meta.str("media") == "video") return u
    val l = u.lowercase()
    return u.takeIf { l.startsWith("http://") || l.startsWith("https://") || l.startsWith("/api/") || l.startsWith("data:video") }
}

private fun thumbnail(bytes: ByteArray, max: Int): ImageBitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= max) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
}

private fun pdfFirstPage(context: Context, bytes: ByteArray): ImageBitmap? = runCatching {
    val f = File.createTempFile("card", ".pdf", context.cacheDir).apply { writeBytes(bytes) }
    try {
        ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY).use { pfd ->
            PdfRenderer(pfd).use { r ->
                r.openPage(0).use { page ->
                    val w = 480
                    val h = (480f * page.height / page.width).toInt().coerceAtLeast(1)
                    val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888).apply { eraseColor(android.graphics.Color.WHITE) }
                    page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                    bmp.asImageBitmap()
                }
            }
        }
    } finally { f.delete() }
}.getOrNull()

/** First frame at up to 640 px. `/` URIs carry the bearer (MediaMetadataRetriever reads by range). */
private fun videoFrame(url: String): ImageBitmap? = runCatching {
    val mmr = MediaMetadataRetriever()
    try {
        if (url.startsWith("/")) {
            val token = com.allternit.core.auth.AllternitAuth.tokenProvider.bearerToken()
            mmr.setDataSource(LibraryNet.absolute(url), if (token != null) mapOf("Authorization" to "Bearer $token") else emptyMap())
        } else if (url.startsWith("https://")) {
            mmr.setDataSource(url, emptyMap())
        } else return null
        val frame = mmr.getFrameAtTime(0) ?: return null
        val scale = 640f / maxOf(frame.width, frame.height)
        (if (scale < 1f) Bitmap.createScaledBitmap(frame, (frame.width * scale).toInt(), (frame.height * scale).toInt(), true) else frame).asImageBitmap()
    } finally { mmr.release() }
}.getOrNull()

// MARK: - Snapshots (iOS ArtifactSnapshotJob / ArtifactPosterHost)

private const val SNAP_CSP = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: https: blob:; style-src 'unsafe-inline' https:; font-src data: https:\"><style>html,body{margin:0;overflow:hidden;background:#fff}</style>"

/** Lays out an offscreen 1000×625 WebView, loads [html] (or [url] + [script]), and draws it to a 560 px bitmap. */
@SuppressLint("SetJavaScriptEnabled")
private suspend fun snapshot(context: Context, html: String?, scripts: Boolean, url: String? = null, script: String? = null, settleMs: Long = 400): ImageBitmap? =
    CardCache.snapshot.withLock {
        withContext(Dispatchers.Main) {
            withTimeoutOrNull(20_000) {
                suspendCancellableCoroutine { cont ->
                    val web = WebView(context)
                    web.settings.javaScriptEnabled = scripts
                    web.settings.allowFileAccess = false
                    web.settings.allowContentAccess = false
                    // Offscreen: draw in software so View.draw captures the page without a window.
                    web.setLayerType(View.LAYER_TYPE_SOFTWARE, null)
                    web.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY), View.MeasureSpec.makeMeasureSpec(625, View.MeasureSpec.EXACTLY))
                    web.layout(0, 0, 1000, 625)
                    fun capture() {
                        val bmp = Bitmap.createBitmap(1000, 625, Bitmap.Config.ARGB_8888)
                        web.draw(android.graphics.Canvas(bmp))
                        web.destroy()
                        val out = Bitmap.createScaledBitmap(bmp, 560, 350, true).asImageBitmap()
                        if (cont.isActive) cont.resume(out)
                    }
                    web.webViewClient = object : WebViewClient() {
                        override fun shouldOverrideUrlLoading(view: WebView, request: android.webkit.WebResourceRequest) = true
                        override fun onPageFinished(view: WebView, u: String?) {
                            if (script != null) {
                                view.evaluateJavascript(script) { view.postDelayed({ capture() }, settleMs) }
                            } else view.postDelayed({ capture() }, settleMs)
                        }
                    }
                    if (url != null) web.loadUrl(url) else web.loadDataWithBaseURL(null, html ?: "", "text/html", "utf-8", null)
                    cont.invokeOnCancellation { web.post { web.destroy() } }
                }
            }
        }
    }

// MARK: - Decode (iOS ArtifactCardContent.decode)

private val mdNoise = listOf(Regex("data:[^)\\s]+"), Regex("!?\\[([^\\]]*)]\\([^)]*\\)"), Regex("^#{1,6}\\s*"), Regex("[*_`>]"))

/** iOS plainLines: data URIs, code fences, heading marks, link syntax and emphasis removed; `- ` → `• `. */
internal fun plainLines(md: String): List<String> {
    var fenced = false
    return md.lines().mapNotNull { raw ->
        if (raw.trim().startsWith("```")) { fenced = !fenced; return@mapNotNull null }
        if (fenced) return@mapNotNull null
        var l = raw.replace(mdNoise[0], "").replace(mdNoise[1], "$1").replace(mdNoise[2], "").replace(mdNoise[3], "").trim()
        if (l.startsWith("- ") || l.startsWith("+ ")) l = "• " + l.drop(2)
        l.takeIf { it.isNotEmpty() }
    }.take(18)
}

/** The doc's markdown: a markdown body, the doc JSON's `markdown`, or its tabs' markdown. */
internal fun docMarkdown(body: String, format: String): String? {
    if (format.startsWith("text/")) return body
    val o = LibraryJson.parse(body) ?: return null
    o.str("markdown")?.let { return it }
    return o.list("tabs").joinToString("\n\n") { t -> "# ${t.str("title") ?: ""}\n\n${t.str("body") ?: t.str("markdown") ?: ""}" }.ifBlank { null }
}

private fun dashboardTitles(body: String): List<String>? {
    val o = LibraryJson.parse(body) ?: return null
    val fromTabs = o.list("tabs").flatMap { t -> t.list("panels").mapNotNull { it.str("title") } }
    return (fromTabs.ifEmpty { o.list("tiles").mapNotNull { it.str("title") } }).takeIf { it.isNotEmpty() }
}

private fun firstSlideLines(body: String): List<String> = decodeSlides(body)?.firstOrNull()?.let { s -> listOf(s.title) + s.lines.filter { it != s.title } }?.filter { it.isNotBlank() }.orEmpty()

/** iOS designPreviewHTML: the first artboard's design-html layer, else the raw HTML. */
private fun designPreviewHtml(body: String): String? {
    val o = LibraryJson.parse(body)
    val layers = o.list("layers")
    if (o == null || layers.isEmpty()) return body.trim().takeIf { it.startsWith("<") }
    val boardId = o.list("artboards").firstOrNull().str("id")
    val own = layers.filter { it.str("artboard") == boardId }
    return (own + layers).firstNotNullOfOrNull { l -> if (l.str("type") == "design-html") l.child("props").str("html")?.takeIf { it.isNotEmpty() } else null }
}

internal suspend fun decodeCard(context: Context, a: ArtifactV2): CardContent = withContext(Dispatchers.IO) {
    val body = a.body
    val t = body.trim()
    when (a.kind) {
        "image" -> {
            if (a.meta.str("media") == "video") return@withContext videoUrl(body, a.meta)?.let(::videoFrame)?.let { CardContent.Picture(it) } ?: CardContent.Art
            firstUri(body)?.let { mediaBytes(it) }?.let { thumbnail(it, 640) }?.let { CardContent.Picture(it) } ?: CardContent.Art
        }
        "video" -> videoUrl(body, a.meta)?.let(::videoFrame)?.let { CardContent.Picture(it) } ?: CardContent.Art
        "doc" -> docMarkdown(body, a.bodyFormat)?.let { CardContent.Lines(plainLines(it)) } ?: CardContent.Art
        "sheet" -> decodeSheet(body)?.let { CardContent.Grid(it.take(12).map { r -> r.take(6) }) } ?: CardContent.Art
        "slides" -> CardContent.Slide(firstSlideLines(body))
        "code" -> CardContent.Code(body.split("\n").take(16))
        "dashboard" -> dashboardTitles(body)?.let { CardContent.Dashboard(it.take(6)) } ?: CardContent.Art
        "design" -> designPreviewHtml(body)?.let { snapshot(context, SNAP_CSP + it, scripts = false) }?.let { CardContent.Picture(it) } ?: CardContent.Art
        "page" -> when {
            a.bodyFormat == "text/markdown" -> CardContent.Lines(plainLines(body))
            a.bodyFormat == "text/uri-list" || t.isEmpty() || t.startsWith("https://") -> CardContent.Art
            else -> snapshot(context, SNAP_CSP + body, scripts = false)?.let { CardContent.Picture(it) } ?: CardContent.Art
        }
        "card" -> if (t.startsWith("<")) snapshot(context, SNAP_CSP + body, scripts = false)?.let { CardContent.Picture(it) } ?: CardContent.Art else CardContent.Art
        "diagram" -> {
            val html = if (t.startsWith("<svg")) "<div style=\"width:1000px;height:625px;display:flex;align-items:center;justify-content:center;background:#fff\"><img style=\"max-width:96%;max-height:96%\" src=\"data:image/svg+xml;charset=utf-8,${android.net.Uri.encode(t)}\"></div>"
            else ArtifactHTML.mermaid(t)
            snapshot(context, html, scripts = !t.startsWith("<svg"), settleMs = if (t.startsWith("<svg")) 400 else 1200)?.let { CardContent.Picture(it) } ?: CardContent.Art
        }
        "motion", "ops" -> {
            val script = "(async function(){try{for(let i=0;i<40&&typeof window.allternitPoster==='undefined';i++){await new Promise(r=>setTimeout(r,300));}" +
                "return await window.allternitPoster.render(${ArtifactHTML.jsString(a.kind)}, ${ArtifactHTML.jsString(body)});}catch(e){return false;}})()"
            snapshot(context, null, scripts = true, url = "${ArtifactWeb.ORIGIN}/poster/artifact", script = script, settleMs = 1200)?.let { CardContent.Picture(it) }
                ?: if (a.kind == "ops") dashboardTitles(body)?.let { CardContent.Dashboard(it.take(6)) } ?: CardContent.Art else CardContent.Art
        }
        "pdf" -> runCatching { Base64.decode(t, Base64.DEFAULT) }.getOrNull()?.let { pdfFirstPage(context, it) }?.let { CardContent.Picture(it) } ?: CardContent.Art
        else -> CardContent.Art
    }
}

// MARK: - Drawing

@Composable
internal fun ArtifactCardPreview(summary: ArtifactSummary, modifier: Modifier) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val key = "${summary.id}@${summary.currentVersion}"
    LaunchedEffect(key) {
        if (CardCache.content[key] != null) return@LaunchedEffect
        val full = CardCache.fetch.withPermit { (ArtifactsV2Client.get(summary.id) as? ApiResult.Ok)?.value }
        CardCache.content[key] = full?.let { decodeCard(context, it) } ?: CardContent.Art
    }
    Box(modifier.clipToBounds()) {
        when (val content = CardCache.content[key]) {
            is CardContent.Picture -> Image(content.image, null, contentScale = ContentScale.Crop, alignment = Alignment.TopCenter, modifier = Modifier.fillMaxSize())
            is CardContent.Lines -> Paper(Color.White) {
                Column(Modifier.padding(horizontal = 32.dp).padding(top = 28.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    content.lines.forEachIndexed { i, l ->
                        if (i == 0) Text(l, color = Color(0xFF1F2328), fontSize = 20.sp, fontWeight = FontWeight.SemiBold, maxLines = 3)
                        else Text(l, color = Color(0xFF3C4043), fontSize = 14.sp)
                    }
                }
            }
            is CardContent.Grid -> Paper(Color.White) { SheetPaper(content.rows) }
            is CardContent.Slide -> Paper(Color(0xFFF1F3F4)) {
                Box(Modifier.fillMaxWidth().padding(top = 20.dp), contentAlignment = Alignment.TopCenter) {
                    Column(
                        Modifier.requiredSize(400.dp, 225.dp).shadow(4.dp).background(Color.White).padding(horizontal = 48.dp),
                        verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
                    ) {
                        if (content.lines.isEmpty()) Text("Empty slide", color = Color(0xFF9AA0A6), fontSize = 20.sp)
                        content.lines.firstOrNull()?.let { Text(it, color = Color(0xFF1F2328), fontSize = 26.sp, fontWeight = FontWeight.SemiBold, maxLines = 2) }
                        content.lines.drop(1).take(4).forEach { Text(it, color = Color(0xFF5F6368), fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                    }
                }
            }
            is CardContent.Code -> Paper(Color(0xFF18181B)) {
                Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    content.lines.forEach { Text(it, color = Color(0xFFE4E4E7), fontSize = 14.sp, fontFamily = FontFamily.Monospace, maxLines = 1, softWrap = false) }
                }
            }
            is CardContent.Dashboard -> Paper(Color(0xFFF8F9FA)) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    content.titles.chunked(2).forEach { row ->
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            row.forEach { title ->
                                Column(
                                    Modifier.weight(1f).heightIn(min = 120.dp).background(Color.White, RoundedCornerShape(10.dp)).border(1.dp, Color(0xFFE0E3E7), RoundedCornerShape(10.dp)).padding(14.dp),
                                    verticalArrangement = Arrangement.spacedBy(10.dp),
                                ) {
                                    Text(title, color = Color(0xFF1F2328), fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
                                    Icon(painterResource(DesignR.drawable.deck_dashboard), null, tint = Color(0xFF9AA0A6), modifier = Modifier.size(26.dp))
                                }
                            }
                            if (row.size == 1) Box(Modifier.weight(1f))
                        }
                    }
                }
            }
            CardContent.Art, null -> Box(Modifier.fillMaxSize().background(c.bgPrimary), contentAlignment = Alignment.Center) {
                Icon(painterResource(ArtifactKinds.icon(summary.kind)), null, tint = c.textTertiary, modifier = Modifier.size(32.dp))
            }
        }
    }
}

/** iOS ArtifactCardPaper: content laid out at 460dp wide and scaled to the card, top-left, clipped. */
@Composable
private fun Paper(bg: Color, content: @Composable () -> Unit) {
    BoxWithConstraints(Modifier.fillMaxSize().background(bg)) {
        val scale = maxWidth.value / 460f
        Box(
            Modifier.wrapContentSize(Alignment.TopStart, unbounded = true).width(460.dp)
                .graphicsLayer { scaleX = scale; scaleY = scale; transformOrigin = TransformOrigin(0f, 0f) },
        ) { content() }
    }
}

@Composable
private fun SheetPaper(rows: List<List<String>>) {
    val line = Color(0xFFE0E3E7)
    Column {
        Row {
            Box(Modifier.width(36.dp).heightIn(min = 24.dp).background(Color(0xFFF8F9FA)).border(0.5.dp, line))
            ('A'..'F').forEach { col ->
                Box(Modifier.weight(1f).heightIn(min = 24.dp).background(Color(0xFFF8F9FA)).border(0.5.dp, line), contentAlignment = Alignment.Center) {
                    Text("$col", color = Color(0xFF5F6368), fontSize = 12.sp)
                }
            }
        }
        rows.forEachIndexed { i, r ->
            Row {
                Box(Modifier.width(36.dp).heightIn(min = 26.dp).border(0.5.dp, line), contentAlignment = Alignment.Center) { Text("${i + 1}", color = Color(0xFF5F6368), fontSize = 12.sp) }
                (0 until 6).forEach { j ->
                    Text(
                        r.getOrNull(j).orEmpty(), color = Color(0xFF1F2328), fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        fontWeight = if (i == 0) FontWeight.SemiBold else FontWeight.Normal,
                        modifier = Modifier.weight(1f).heightIn(min = 26.dp).border(0.5.dp, line).padding(horizontal = 6.dp, vertical = 5.dp),
                    )
                }
            }
        }
    }
}
