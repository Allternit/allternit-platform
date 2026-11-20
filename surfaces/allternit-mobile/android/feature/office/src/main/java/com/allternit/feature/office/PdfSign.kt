package com.allternit.feature.office

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Color as AndroidColor
import android.graphics.Paint
import android.graphics.Path as AndroidPath
import android.graphics.RectF
import android.graphics.pdf.PdfDocument
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.font.FontWeight
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.uploadPdfToLibrary
import java.io.File
import java.util.UUID
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Page size in points of an unsigned blank page (iOS blankPDF, US Letter). */
private const val BLANK_W = 612
private const val BLANK_H = 792
/** The signature box on the first page (iOS: 200 × 80 pt, 40 pt in from the bottom-right corner). */
private const val SIG_W = 200f
private const val SIG_H = 80f
private const val SIG_MARGIN = 40f

/** A page rendered with the platform [PdfRenderer] at [scale] pixels per point, plus its size in points. */
private class RenderedPage(val bitmap: Bitmap, val widthPt: Int, val heightPt: Int)

private fun renderFirstPage(context: Context, pdf: ByteArray?, scale: Float): RenderedPage? {
    if (pdf == null) {
        val bmp = Bitmap.createBitmap((BLANK_W * scale).toInt(), (BLANK_H * scale).toInt(), Bitmap.Config.ARGB_8888)
        bmp.eraseColor(AndroidColor.WHITE)
        return RenderedPage(bmp, BLANK_W, BLANK_H)
    }
    val file = File.createTempFile("sign-src", ".pdf", context.cacheDir)
    return try {
        file.writeBytes(pdf)
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { fd ->
            PdfRenderer(fd).use { r ->
                if (r.pageCount == 0) return@use null
                r.openPage(0).use { page ->
                    val bmp = Bitmap.createBitmap((page.width * scale).toInt().coerceAtLeast(1), (page.height * scale).toInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
                    bmp.eraseColor(AndroidColor.WHITE)
                    page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                    RenderedPage(bmp, page.width, page.height)
                }
            }
        }
    } catch (_: Exception) {
        null
    } finally {
        file.delete()
    }
}

/** The pad's strokes as one path, scaled and centred (aspect-fit) into [box]. */
private fun signaturePath(strokes: List<List<Offset>>, box: RectF): AndroidPath? {
    val pts = strokes.flatten()
    if (pts.isEmpty()) return null
    val minX = pts.minOf { it.x }; val maxX = pts.maxOf { it.x }
    val minY = pts.minOf { it.y }; val maxY = pts.maxOf { it.y }
    val w = (maxX - minX).coerceAtLeast(1f); val h = (maxY - minY).coerceAtLeast(1f)
    val k = minOf(box.width() / w, box.height() / h)
    val ox = box.left + (box.width() - w * k) / 2f
    val oy = box.top + (box.height() - h * k) / 2f
    val path = AndroidPath()
    strokes.forEach { s ->
        s.forEachIndexed { i, p ->
            val x = ox + (p.x - minX) * k; val y = oy + (p.y - minY) * k
            if (i == 0) { path.moveTo(x, y); path.lineTo(x + 0.1f, y + 0.1f) } else path.lineTo(x, y)
        }
    }
    return path
}

/**
 * Composes the signature onto the first page and returns a new one-page PDF (iOS applySignature: the page is
 * flattened, the signature drawn 200 × 80 pt from the bottom-right corner).
 */
private fun composeSigned(context: Context, source: ByteArray?, strokes: List<List<Offset>>): ByteArray? {
    val scale = 2f
    val page = renderFirstPage(context, source, scale) ?: return null
    val canvas = AndroidCanvas(page.bitmap)
    val box = RectF(
        (page.widthPt - SIG_W - SIG_MARGIN) * scale, (page.heightPt - SIG_H - SIG_MARGIN) * scale,
        (page.widthPt - SIG_MARGIN) * scale, (page.heightPt - SIG_MARGIN) * scale,
    )
    val path = signaturePath(strokes, box) ?: return null
    canvas.drawPath(path, Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = AndroidColor.BLACK; style = Paint.Style.STROKE; strokeWidth = 2f * scale; strokeCap = Paint.Cap.ROUND; strokeJoin = Paint.Join.ROUND
    })
    val doc = PdfDocument()
    val info = PdfDocument.PageInfo.Builder(page.widthPt, page.heightPt, 1).create()
    val out = doc.startPage(info)
    out.canvas.drawBitmap(page.bitmap, null, RectF(0f, 0f, page.widthPt.toFloat(), page.heightPt.toFloat()), Paint(Paint.FILTER_BITMAP_FLAG))
    doc.finishPage(out)
    return java.io.ByteArrayOutputStream().also { doc.writeTo(it); doc.close() }.toByteArray()
}

/**
 * PDF signing (iOS PDFSignView): a signature pad on top, the PDF's first page below, Clear and Sign & Share. Signing
 * composes a new PDF, keeps a copy in the app's files (iOS Documents), and offers Save (the system picker, like Share)
 * and Save to library (uploads it to Artifacts as a PDF). [source] is the PDF's bytes, or null for a blank page.
 */
@Composable
fun PdfSignScreen(title: String, source: ByteArray?, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val strokes = remember { mutableStateListOf<List<Offset>>() }
    var current by remember { mutableStateOf<List<Offset>>(emptyList()) }
    var preview by remember { mutableStateOf<RenderedPage?>(null) }
    var previewFailed by remember { mutableStateOf(false) }
    var signed by remember { mutableStateOf<ByteArray?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    BackHandler(onBack = onClose)

    LaunchedEffect(source, signed) {
        val bytes = signed ?: source
        val r = withContext(Dispatchers.IO) { renderFirstPage(context, bytes, 1.5f) }
        preview = r; previewFailed = r == null
    }
    val saveFile = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri: Uri? ->
        val data = signed
        if (uri != null && data != null) {
            scope.launch {
                val ok = withContext(Dispatchers.IO) { runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(data) } != null }.getOrDefault(false) }
                message = if (ok) "Saved." else "Couldn't save the file. Try again."
            }
        }
    }

    fun clear() { strokes.clear(); current = emptyList(); signed = null; message = null }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("pdfSignScreen")) {
        LibraryHeader(title = "Sign PDF", subtitle = title.takeIf { source != null }, onBack = onClose, backTag = "pdfSignCancel")
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text("Draw signature", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text("Clear", color = c.accentPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.clickable { clear() }.padding(4.dp).testTag("pdfSignClearTop"))
            }
            Box(
                Modifier.fillMaxWidth().height(180.dp).clip(RoundedCornerShape(14.dp)).background(c.bgSecondary)
                    .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).testTag("pdfSignPad")
                    .pointerInput(Unit) {
                        detectDragGestures(
                            onDragStart = { o -> signed = null; current = listOf(o) },
                            onDrag = { change, _ -> change.consume(); current = current + change.position },
                            onDragEnd = { if (current.isNotEmpty()) strokes.add(current); current = emptyList() },
                            onDragCancel = { current = emptyList() },
                        )
                    },
            ) {
                Canvas(Modifier.fillMaxSize()) {
                    (strokes + listOf(current)).forEach { s ->
                        if (s.isEmpty()) return@forEach
                        val p = Path().apply { moveTo(s[0].x, s[0].y); lineTo(s[0].x + 0.1f, s[0].y + 0.1f); s.drop(1).forEach { lineTo(it.x, it.y) } }
                        drawPath(p, Color.Black, style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round))
                    }
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
            val page = preview
            when {
                page != null -> Image(page.bitmap.asImageBitmap(), "PDF preview", Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
                previewFailed -> FriendlyState(FriendlyStyle.Empty, "No PDF loaded", "Select a PDF to sign and share.") {
                    Icon(painterResource(DesignR.drawable.deck_pdf), null, tint = c.accentPrimary)
                }
            }
        }
        message?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp).testTag("pdfSignMessage")) }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            val empty = strokes.isEmpty()
            val done = signed
            Text("Clear", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                modifier = Modifier.weight(1f).height(44.dp).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).clickable { clear() }.padding(top = 12.dp).testTag("pdfSignClear"))
            if (done == null) {
                Text(if (working) "Signing…" else "Sign & Share", color = Color.Black, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    modifier = Modifier.weight(1f).height(44.dp).clip(RoundedCornerShape(12.dp)).background(c.accentPrimary.copy(alpha = if (empty) 0.6f else 1f))
                        .clickable(enabled = !empty && !working) {
                            working = true; message = null
                            scope.launch {
                                val data = withContext(Dispatchers.IO) {
                                    composeSigned(context, source, strokes.toList())?.also { bytes ->
                                        // iOS keeps a copy in Documents so it can be reopened later.
                                        runCatching { File(File(context.filesDir, "signed").apply { mkdirs() }, "signed-${UUID.randomUUID()}.pdf").writeBytes(bytes) }
                                    }
                                }
                                working = false
                                if (data == null) message = "Couldn't sign the PDF. Try again." else { signed = data; saveFile.launch("signed-${title.replace('/', '-')}.pdf") }
                            }
                        }.padding(top = 12.dp).testTag("pdfSignApply"))
            } else {
                Text("Save to library", color = Color.Black, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    modifier = Modifier.weight(1f).height(44.dp).clip(RoundedCornerShape(12.dp)).background(c.accentPrimary)
                        .clickable(enabled = !working) {
                            working = true
                            scope.launch {
                                val r = withContext(Dispatchers.IO) {
                                    val f = File(context.cacheDir, "signed-${UUID.randomUUID()}.pdf").also { it.writeBytes(done) }
                                    try { uploadPdfToLibrary(context, Uri.fromFile(f)) } finally { f.delete() }
                                }
                                working = false
                                message = if (r.isSuccess) "Saved to your library." else (r.exceptionOrNull()?.message ?: "Couldn't save to your library.")
                            }
                        }.padding(top = 12.dp).testTag("pdfSignSaveLibrary"))
            }
        }
    }
}
