package com.allternit.feature.office

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Color as AndroidColor
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.pdf.PdfDocument
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.animateScrollBy
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.produceState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import android.util.Log
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.SignRequest
import com.allternit.core.api.gen.SignSlot
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.uploadPdfToLibrary
import com.allternit.feature.office.sign.SignMode
import com.allternit.feature.office.sign.SignFlight
import com.allternit.feature.office.sign.SignRepository
import com.allternit.feature.office.sign.SignSheet
import com.allternit.feature.office.sign.rememberSignSheetState
import com.allternit.feature.office.sign.fitRect
import com.tom_roush.pdfbox.android.PDFBoxResourceLoader
import com.tom_roush.pdfbox.pdmodel.PDDocument
import com.tom_roush.pdfbox.pdmodel.PDPageContentStream
import com.tom_roush.pdfbox.pdmodel.font.PDType1Font
import com.tom_roush.pdfbox.pdmodel.graphics.image.LosslessFactory
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Page size in points of an unsigned blank page (iOS blankPDF, US Letter). */
private const val BLANK_W = 612
private const val BLANK_H = 792

/** Default slot (iOS defaultSlot): 180 × 56 pt, 36 pt in from the bottom-right corner of the last page. */
private const val SLOT_W = 180.0
private const val SLOT_H = 56.0
private const val SLOT_MARGIN = 36.0

private val Warn = Color(0xFFE0612F)

/** A page rendered with the platform [PdfRenderer], plus its size in points. */
private class RenderedPage(val bitmap: Bitmap, val widthPt: Int, val heightPt: Int)

/** A page's size in points. */
private class PageInfo(val widthPt: Int, val heightPt: Int)

/**
 * The document on screen: one [PdfRenderer] kept open, pages rendered on demand (only the ones the LazyColumn shows) and
 * kept in a small cache, so a 300-page PDF costs a few bitmaps, not all of them. [PdfRenderer] opens one page at a time,
 * so renders are serialized.
 */
private class PdfDisplay private constructor(
    private val file: File, private val fd: ParcelFileDescriptor, private val renderer: PdfRenderer, val sizes: List<PageInfo>,
) {
    private val lock = Any()
    private val scale = if (sizes.size > 10) 1f else 1.5f
    private val cache = object : android.util.LruCache<Int, Bitmap>(8) {}

    val count: Int get() = sizes.size

    fun cached(i: Int): Bitmap? = cache.get(i)

    fun render(i: Int): Bitmap? {
        cache.get(i)?.let { return it }
        return synchronized(lock) {
            cache.get(i) ?: runCatching {
                renderer.openPage(i).use { page ->
                    val bmp = Bitmap.createBitmap((page.width * scale).toInt().coerceAtLeast(1), (page.height * scale).toInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
                    bmp.eraseColor(AndroidColor.WHITE)
                    page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                    bmp
                }
            }.getOrNull()?.also { cache.put(i, it) }
        }
    }

    fun close() {
        synchronized(lock) {
            runCatching { renderer.close() }; runCatching { fd.close() }; file.delete()
        }
    }

    companion object {
        /** Opens [pdf] and reads every page's size (cheap); null if unreadable or empty. */
        fun open(context: Context, pdf: ByteArray): PdfDisplay? {
            val file = File.createTempFile("sign-view", ".pdf", context.cacheDir)
            return try {
                file.writeBytes(pdf)
                val fd = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
                try {
                    val r = PdfRenderer(fd)
                    if (r.pageCount == 0) { r.close(); fd.close(); file.delete(); return null }
                    val sizes = List(r.pageCount) { i -> r.openPage(i).use { PageInfo(it.width, it.height) } }
                    PdfDisplay(file, fd, r, sizes)
                } catch (e: Exception) { runCatching { fd.close() }; file.delete(); null }
            } catch (_: Exception) { file.delete(); null }
        }
    }
}

private class FlightPlan(val bitmap: Bitmap, val from: Rect, val to: Rect)

private fun blankPdf(): ByteArray {
    val doc = PdfDocument()
    doc.finishPage(doc.startPage(PdfDocument.PageInfo.Builder(BLANK_W, BLANK_H, 1).create()))
    return java.io.ByteArrayOutputStream().also { doc.writeTo(it); doc.close() }.toByteArray()
}

/** Renders every page of [pdf] at 2 px per point for composing the signed copy. Null if unreadable. */
private fun renderPages(context: Context, pdf: ByteArray): List<RenderedPage>? {
    val file = File.createTempFile("sign-src", ".pdf", context.cacheDir)
    return try {
        file.writeBytes(pdf)
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { fd ->
            PdfRenderer(fd).use { r ->
                if (r.pageCount == 0) return@use null
                val scale = 2f
                List(r.pageCount) { i ->
                    r.openPage(i).use { page ->
                        val bmp = Bitmap.createBitmap((page.width * scale).toInt().coerceAtLeast(1), (page.height * scale).toInt().coerceAtLeast(1), Bitmap.Config.ARGB_8888)
                        bmp.eraseColor(AndroidColor.WHITE)
                        page.render(bmp, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                        RenderedPage(bmp, page.width, page.height)
                    }
                }
            }
        }
    } catch (_: Exception) {
        null
    } finally {
        file.delete()
    }
}

private fun defaultSlot(page: PageInfo, index: Int) = SignSlot(
    id = "slot-1", label = "Signature", page = index.toLong(),
    x = page.widthPt - SLOT_W - SLOT_MARGIN, y = SLOT_MARGIN, w = SLOT_W, h = SLOT_H,
)

/**
 * Composes the signed copy (iOS compose). Real PDF: the original is loaded with PdfBox and only the slot's page gets the
 * [ink] drawn into [slot] (keeping its aspect) and a 7 pt footer line; every other page, and the text, stay as they were.
 * Falls back to [composeFlattened] if PdfBox cannot handle the file. Slot units are PDF points, origin bottom-left.
 */
private fun composeSigned(context: Context, source: ByteArray, ink: Bitmap, slot: SignSlot, footer: String): ByteArray? =
    try {
        composeVector(context, source, ink, slot, footer)
    } catch (e: Throwable) {
        Log.w("PdfSign", "Vector signing failed, flattening the pages instead: ${e.message}", e)
        composeFlattened(context, source, ink, slot, footer)
    }

/** Helvetica (a standard 14 font) cannot encode every character: swap those for "?". */
private fun encodable(font: PDType1Font, text: String): String = buildString {
    text.forEach { ch ->
        val ok = ch.code >= 0x20 && runCatching { font.encode(ch.toString()) }.isSuccess
        append(if (ok) ch else '?')
    }
}

private fun composeVector(context: Context, source: ByteArray, ink: Bitmap, slot: SignSlot, footer: String): ByteArray {
    PDFBoxResourceLoader.init(context.applicationContext)
    PDDocument.load(source).use { doc ->
        val pageIndex = slot.page.toInt()
        if (pageIndex !in 0 until doc.numberOfPages) throw IllegalStateException("slot page $pageIndex out of range")
        val page = doc.getPage(pageIndex)
        // Rotated pages need the renderer's rotated frame; leave those to the flatten fallback.
        if (page.rotation % 360 != 0) throw IllegalStateException("rotated page")
        val box = page.mediaBox
        val w = box.width; val h = box.height
        // Same maths as the flatten path, in top-left coordinates, then flipped into PDF space.
        val slotRect = Rect(slot.x.toFloat(), (h - slot.y - slot.h).toFloat(), (slot.x + slot.w).toFloat(), (h - slot.y).toFloat())
        val fit = fitRect(ink.width.toFloat(), ink.height.toFloat(), slotRect)
        val lineY = if (slotRect.bottom + 2f + 9f <= h) slotRect.bottom + 2f else maxOf(slotRect.top - 10f, 0f)
        val image = LosslessFactory.createFromImage(doc, ink)
        val font = PDType1Font.HELVETICA
        PDPageContentStream(doc, page, PDPageContentStream.AppendMode.APPEND, true, true).use { cs ->
            cs.drawImage(image, box.lowerLeftX + fit.left, box.lowerLeftY + h - fit.bottom, fit.right - fit.left, fit.bottom - fit.top)
            cs.beginText()
            cs.setFont(font, 7f)
            cs.setNonStrokingColor(64, 64, 64)
            cs.newLineAtOffset(box.lowerLeftX + slotRect.left, box.lowerLeftY + h - (lineY + 7f))
            cs.showText(encodable(font, footer))
            cs.endText()
        }
        return java.io.ByteArrayOutputStream().also { doc.save(it) }.toByteArray()
    }
}

/**
 * Fallback: the signed copy with every page flattened to an image, [ink] drawn into [slot] keeping its aspect, and a 7 pt
 * footer line under the slot (above it when there is no room). Slot units are PDF points, origin bottom-left.
 */
private fun composeFlattened(context: Context, source: ByteArray, ink: Bitmap, slot: SignSlot, footer: String): ByteArray? {
    val pages = renderPages(context, source) ?: return null
    val doc = PdfDocument()
    val bitmapPaint = Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG)
    val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = AndroidColor.argb(255, 64, 64, 64); textSize = 7f }
    pages.forEachIndexed { i, p ->
        val w = p.widthPt.toFloat(); val h = p.heightPt.toFloat()
        val out = doc.startPage(PdfDocument.PageInfo.Builder(p.widthPt, p.heightPt, i + 1).create())
        out.canvas.drawBitmap(p.bitmap, null, RectF(0f, 0f, w, h), bitmapPaint)
        if (i.toLong() == slot.page) {
            val box = Rect(slot.x.toFloat(), (h - slot.y - slot.h).toFloat(), (slot.x + slot.w).toFloat(), (h - slot.y).toFloat())
            val fit = fitRect(ink.width.toFloat(), ink.height.toFloat(), box)
            out.canvas.drawBitmap(ink, null, RectF(fit.left, fit.top, fit.right, fit.bottom), bitmapPaint)
            val lineY = if (box.bottom + 2f + 9f <= h) box.bottom + 2f else maxOf(box.top - 10f, 0f)
            out.canvas.drawText(footer, box.left, lineY + 7f, textPaint)
        }
        doc.finishPage(out)
    }
    return java.io.ByteArrayOutputStream().also { doc.writeTo(it); doc.close() }.toByteArray()
}

private fun shortDate(d: Date): String = SimpleDateFormat("d MMM yyyy", Locale.getDefault()).format(d)
private fun stamp(d: Date): String = SimpleDateFormat("yyyy-MM-dd HH:mm z", Locale.getDefault()).format(d)
private fun signedFileName(name: String): String {
    val base = name.removeSuffix(".pdf").removeSuffix(".PDF").replace('/', '-')
    return "${base.ifEmpty { "Document" }}-signed.pdf"
}

/**
 * Review & sign for a PDF (iOS PDFSignView, web after.tsx): the document with a dashed "Sign here" slot, an orange
 * "Awaiting your signature" bar that opens [SignSheet], a dim scrim and a "Preview" pill while the sheet is open, then
 * [SignFlight] from the pad into the slot and "Signed <date>" under it.
 *
 * With [requestId] (an agent or app asked) the slot comes from the record and /sign closes it. Without one, a signed-in
 * user signs their own document (a record is created first). Signed out, the signature is placed and saved on this
 * device only, with no record. With no slot in the record, tap the page to move the default one (last page, bottom-right).
 * [source] is the PDF's bytes; null means a blank page, or the request's document when [requestId] is set.
 */
@Composable
fun PdfSignScreen(title: String, source: ByteArray?, onClose: () -> Unit, requestId: String? = null) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()
    val screenHeightPx = LocalConfiguration.current.screenHeightDp * density.density
    val auth by AllternitAuth.state.collectAsState()
    val account = (auth as? AuthState.SignedIn)?.account

    var request by remember { mutableStateOf<SignRequest?>(null) }
    var original by remember { mutableStateOf<ByteArray?>(null) }
    var doc by remember { mutableStateOf<PdfDisplay?>(null) }
    DisposableEffect(doc) { val d = doc; onDispose { d?.close() } }
    val sheetState = rememberSignSheetState()
    val listState = rememberLazyListState()
    var loadError by remember { mutableStateOf<String?>(null) }
    var slot by remember { mutableStateOf<SignSlot?>(null) }
    var showSheet by remember { mutableStateOf(false) }
    var previewing by remember { mutableStateOf(false) }
    var submitting by remember { mutableStateOf(false) }
    var placed by remember { mutableStateOf(false) }
    var ink by remember { mutableStateOf<Bitmap?>(null) }
    var flight by remember { mutableStateOf<FlightPlan?>(null) }
    var signedAt by remember { mutableStateOf<Date?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    var signedPdf by remember { mutableStateOf<ByteArray?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var slotCoords by remember { mutableStateOf<LayoutCoordinates?>(null) }
    var flightJob by remember { mutableStateOf<Job?>(null) }

    val req = request
    val docName = req?.documentName ?: if (title.endsWith(".pdf", true)) title else "$title.pdf"
    val mode = when { req != null || requestId != null -> SignMode.Request; account != null -> SignMode.Self; else -> SignMode.Local }
    val isPending = req?.status?.let { it == "pending" } ?: true
    val hasRequestSlot = req?.slots?.isNotEmpty() == true
    val canPlace = !hasRequestSlot && !placed && flight == null && !submitting && !showSheet

    LaunchedEffect(source, requestId) {
        loadError = null
        val rec = if (requestId != null) {
            when (val r = SignRepository.get(requestId)) {
                is ApiResult.Ok -> r.value
                else -> { loadError = SignRepository.errorMessage(r); return@LaunchedEffect }
            }
        } else null
        request = rec
        val bytes = source ?: if (rec != null) SignRepository.download(rec.documentUrl) else withContext(Dispatchers.IO) { blankPdf() }
        if (bytes == null) { loadError = "The document could not be downloaded."; return@LaunchedEffect }
        val opened = withContext(Dispatchers.IO) { PdfDisplay.open(context, bytes) }
        if (opened == null) { loadError = "The PDF could not be loaded."; return@LaunchedEffect }
        original = bytes; doc = opened
        // The default slot sits on the real last page, however long the document is.
        slot = rec?.slots?.firstOrNull() ?: defaultSlot(opened.sizes.last(), opened.count - 1)
    }

    val saveFile = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/pdf")) { uri: Uri? ->
        val data = signedPdf
        if (uri != null && data != null) {
            scope.launch {
                val ok = withContext(Dispatchers.IO) { runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(data) } != null }.getOrDefault(false) }
                message = if (ok) "Saved." else "Couldn't save the file. Try again."
            }
        }
    }

    fun fail(text: String) {
        flightJob?.cancel(); flight = null
        failure = text; placed = false; ink = null; signedAt = null
    }

    suspend fun submit(png: ByteArray, bmp: Bitmap, method: String, s: SignSlot, at: Date) {
        try {
            val src = original ?: throw IllegalStateException("The document could not be read.")
            val name = account?.displayName ?: "this device"
            val data: ByteArray
            if (mode == SignMode.Local) {
                data = withContext(Dispatchers.IO) { composeSigned(context, src, bmp, s, "Signed on this device by $name · ${stamp(at)}") }
                    ?: throw IllegalStateException("Couldn't sign the PDF. Try again.")
            } else {
                val record = req ?: SignRepository.createSelf(docName, src, docName, listOf(s)).getOrThrow()
                data = withContext(Dispatchers.IO) { composeSigned(context, src, bmp, s, "Signed electronically by $name · ${stamp(at)} · ${record.id.take(8)}") }
                    ?: throw IllegalStateException("Couldn't sign the PDF. Try again.")
                SignRepository.submitSignature(record, data, png, method, s, signedFileName(docName)).getOrThrow()
            }
            withContext(Dispatchers.IO) {
                // iOS keeps a copy on the device so it can be reopened later.
                runCatching { File(File(context.filesDir, "signed").apply { mkdirs() }, "signed-${UUID.randomUUID()}.pdf").writeBytes(data) }
            }
            signedPdf = data
            sheetState.reset()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            fail(e.message ?: "Couldn't sign the PDF. Try again.")
        } finally {
            submitting = false
        }
    }

    fun handleSigned(png: ByteArray, bmp: Bitmap, method: String, from: Rect) {
        val s = slot ?: return
        val at = Date()
        ink = bmp; signedAt = at; showSheet = false; previewing = false; failure = null; message = null; submitting = true
        flightJob = scope.launch {
            // Bring the slot into view, let the sheet slide away, then lift the ink out of where the pad was.
            slot?.let { sl -> listState.animateScrollToItem(sl.page.toInt().coerceIn(0, (doc?.count ?: 1) - 1)) }
            slotCoords?.takeIf { it.isAttached }?.let { co ->
                val target = screenHeightPx * 0.32f
                listState.animateScrollBy(co.boundsInWindow().top - target)
            }
            delay(350)
            val to = slotCoords?.takeIf { it.isAttached }?.boundsInWindow() ?: Rect(from.left, from.top - 200f, from.left + 540f, from.top - 30f)
            flight = FlightPlan(bmp, from, to)
        }
        scope.launch { submit(png, bmp, method, s, at) }
    }

    fun movedSlot(pageIndex: Int, page: PageInfo, xPt: Float, yFromTopPt: Float) {
        val cur = slot ?: return
        val x = (xPt - cur.w.toFloat() / 2f).coerceIn(0f, (page.widthPt - cur.w.toFloat()).coerceAtLeast(0f))
        val y = (page.heightPt - yFromTopPt - cur.h.toFloat() / 2f).coerceIn(0f, (page.heightPt - cur.h.toFloat()).coerceAtLeast(0f))
        slot = cur.copy(page = pageIndex.toLong(), x = x.toDouble(), y = y.toDouble())
    }

    val dim by animateFloatAsState(if (showSheet) 0.28f else 0f, tween(220), label = "signDim")

    Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("pdfSignScreen")) {
        Column(Modifier.fillMaxSize().statusBarsPadding()) {
            LibraryHeader(title = req?.title ?: "Sign PDF", subtitle = docName.takeIf { req == null && source != null }, onBack = onClose, backTag = "pdfSignCancel")
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when {
                    doc != null -> {
                        val d = doc!!
                        LazyColumn(Modifier.fillMaxSize(), state = listState, contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            items(d.count, key = { it }) { index ->
                                val p = d.sizes[index]
                                val bitmap by produceState<Bitmap?>(d.cached(index), d, index) { if (value == null) value = withContext(Dispatchers.IO) { d.render(index) } }
                                BoxWithConstraints(Modifier.fillMaxWidth().aspectRatio(p.widthPt.toFloat() / p.heightPt).shadow(2.dp, RoundedCornerShape(2.dp)).background(Color.White)) {
                                    val k = constraints.maxWidth.toFloat() / p.widthPt
                                    bitmap?.let { bmp ->
                                        Image(
                                            bmp.asImageBitmap(), "Page ${index + 1}", Modifier.fillMaxSize().pointerInput(canPlace, p) {
                                                if (canPlace) detectTapGestures { o -> movedSlot(index, p, o.x / k, o.y / k) }
                                            },
                                            contentScale = ContentScale.FillBounds,
                                        )
                                    }
                                val s = slot
                                if (s != null && s.page == index.toLong()) {
                                    val left = (s.x.toFloat() * k).toInt()
                                    val top = ((p.heightPt - s.y - s.h).toFloat() * k).toInt()
                                    val wDp = with(density) { (s.w.toFloat() * k).toDp() }
                                    val hDp = with(density) { (s.h.toFloat() * k).toDp() }
                                    Box(
                                        Modifier.offset { IntOffset(left, top) }.size(wDp, hDp)
                                            .onGloballyPositioned { slotCoords = it }
                                            .background(Warn.copy(alpha = if (placed) 0f else 0.06f), RoundedCornerShape(8.dp))
                                            .drawBehind {
                                                if (!placed) drawRoundRect(Warn, cornerRadius = CornerRadius(8.dp.toPx()), style = Stroke(1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(10f, 8f))))
                                            }
                                            .testTag("pdfSignSlot"),
                                        contentAlignment = Alignment.Center,
                                    ) {
                                        val shown = ink
                                        if (placed && shown != null) Image(shown.asImageBitmap(), "Your signature", Modifier.fillMaxSize(), contentScale = ContentScale.Fit)
                                        else if (!placed) Text(s.label.ifBlank { "Sign here" }.let { if (it == "Signature") "Sign here" else it }, color = Warn, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                                    }
                                    val at = signedAt
                                    if (placed && at != null) {
                                        Text("Signed ${shortDate(at)}", color = c.textSecondary, fontSize = 10.sp, modifier = Modifier.offset { IntOffset(left, top + (s.h.toFloat() * k).toInt() + 2) })
                                    }
                                }
                            }
                        }
                    }
                    }
                    loadError != null -> FriendlyState(FriendlyStyle.Empty, "Couldn't open the PDF", loadError ?: "") {
                        Icon(painterResource(DesignR.drawable.deck_pdf), null, tint = c.accentPrimary)
                    }
                    else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text("Loading document…", color = c.textSecondary, fontSize = 14.sp) }
                }
            }
            // Bottom bar: error line, then the peek bar (or the result).
            Column(
                Modifier.fillMaxWidth().shadow(6.dp, RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp)).background(c.bgPrimary, RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp))
                    .navigationBarsPadding().padding(top = 8.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                failure?.let { Text(it, color = Warn, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(horizontal = 20.dp).testTag("pdfSignError")) }
                message?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("pdfSignMessage")) }
                if (doc != null && !placed && !showSheet) {
                    if (isPending) {
                        if (previewing) PreviewPill("Back to signing", Modifier.align(Alignment.CenterHorizontally)) { previewing = false; failure = null; showSheet = true }
                        else if (canPlace) Text("Tap the page to move the signing spot.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
                        Row(
                            Modifier.fillMaxWidth().height(48.dp).clickable(enabled = !submitting) { failure = null; previewing = false; showSheet = true }.testTag("pdfSignPeek"),
                            horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Icon(painterResource(DesignR.drawable.ph_warning), null, tint = Warn, modifier = Modifier.size(15.dp))
                            Text("  Awaiting your signature", color = Warn, fontSize = 13.5.sp, fontWeight = FontWeight.Medium)
                        }
                    } else {
                        Text("This request is ${req?.status}.", color = c.textSecondary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth().height(56.dp).padding(top = 18.dp))
                    }
                } else if (placed) {
                    Text(
                        when {
                            submitting -> "Saving your signed copy…"
                            mode == SignMode.Local -> "Placed on this device. No signing record was made."
                            else -> "Signed and saved to your files."
                        },
                        color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(horizontal = 20.dp),
                    )
                    val done = signedPdf
                    if (done != null) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        BarButton("Save", false, Modifier.weight(1f).testTag("pdfSignSave")) { saveFile.launch(signedFileName(docName)) }
                        BarButton("Save to library", true, Modifier.weight(1f).testTag("pdfSignSaveLibrary")) {
                            scope.launch {
                                val r = withContext(Dispatchers.IO) {
                                    val f = File(context.cacheDir, "signed-${UUID.randomUUID()}.pdf").also { it.writeBytes(done) }
                                    try { uploadPdfToLibrary(context, Uri.fromFile(f)) } finally { f.delete() }
                                }
                                message = if (r.isSuccess) "Saved to your library." else (r.exceptionOrNull()?.message ?: "Couldn't save to your library.")
                            }
                        }
                    } else Box(Modifier.height(8.dp))
                }
            }
        }
        // The document dims while the sheet is open.
        if (dim > 0f) Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = dim)))
        if (showSheet) {
            // The sheet is its own window; the pill is a popup (above it) at about the sheet's top.
            Popup(alignment = Alignment.BottomCenter, offset = IntOffset(0, -(screenHeightPx * 0.52f).toInt())) {
                PreviewPill("Preview", Modifier) { showSheet = false; previewing = true }
            }
            SignSheet(
                title = req?.title ?: docName, summary = req?.summary, details = req?.details ?: emptyList(),
                sendTo = req?.sendTo?.let { it.name ?: it.email }, mode = mode, state = sheetState,
                onSign = { png, bmp, method, rect -> handleSigned(png, bmp, method, rect) },
                onClose = { showSheet = false },
            )
        }
        flight?.let { f ->
            SignFlight(f.bitmap, f.from, f.to, Modifier.fillMaxSize()) { flight = null; placed = failure == null }
        }
    }
}

@Composable
private fun PreviewPill(label: String, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        label, color = c.textPrimary, fontSize = 12.5.sp, fontWeight = FontWeight.Medium,
        modifier = modifier.shadow(5.dp, RoundedCornerShape(50)).clip(RoundedCornerShape(50)).background(c.bgPrimary)
            .clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 7.dp).testTag("pdfSignPreview"),
    )
}

@Composable
private fun BarButton(label: String, filled: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        label, color = if (filled) Color.White else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
        modifier = modifier.height(44.dp).clip(RoundedCornerShape(22.dp)).background(if (filled) Color.Black else c.bgSecondary).clickable(onClick = onClick).padding(top = 12.dp),
    )
}
