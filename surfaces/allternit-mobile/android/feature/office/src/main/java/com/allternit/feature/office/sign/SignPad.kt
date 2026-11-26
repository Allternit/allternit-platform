package com.allternit.feature.office.sign

import android.graphics.Bitmap
import android.graphics.Canvas as AndroidCanvas
import android.graphics.Color as AndroidColor
import android.graphics.Paint
import android.graphics.Path as AndroidPath
import android.graphics.RectF
import android.graphics.Typeface
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.IntSize
import java.io.ByteArrayOutputStream
import kotlin.math.ceil
import kotlin.math.max

/*
 * The signature pad (iOS SignPadModel / PencilKit canvas). The path logic comes from PdfSign.kt `signaturePath` (which stays
 * untouched): strokes are pad-local points, scaled and centred into a box. New here: quadratic smoothing through the stroke
 * midpoints, undo, a trimmed black-on-transparent ink bitmap, typed ink, and the PNG size cap.
 */

/** Strokes drawn on the pad, in pad-local pixels. Hoist it in the sheet; the pad composable only reads and writes it. */
@Stable
class SignPadState {
    val strokes = mutableStateListOf<List<Offset>>()
    var current by mutableStateOf<List<Offset>>(emptyList())
    var size by mutableStateOf(IntSize.Zero)

    val isEmpty: Boolean get() = strokes.isEmpty() && current.isEmpty()

    fun undo() {
        if (current.isNotEmpty()) current = emptyList() else if (strokes.isNotEmpty()) strokes.removeAt(strokes.lastIndex)
    }

    fun clear() {
        strokes.clear()
        current = emptyList()
    }
}

/** Appends [points] to [path] as a smooth curve: quadratic segments whose control points are the sampled points. */
internal fun addSmoothStroke(path: AndroidPath, points: List<Offset>, map: (Offset) -> Offset = { it }) {
    if (points.isEmpty()) return
    val first = map(points[0])
    path.moveTo(first.x, first.y)
    if (points.size == 1) {
        path.lineTo(first.x + 0.1f, first.y + 0.1f)
        return
    }
    for (i in 1 until points.size - 1) {
        val p = map(points[i]); val n = map(points[i + 1])
        path.quadTo(p.x, p.y, (p.x + n.x) / 2f, (p.y + n.y) / 2f)
    }
    val last = map(points.last())
    path.lineTo(last.x, last.y)
}

/** The pad's strokes as one smoothed path, scaled and centred (aspect-fit) into [box]. Null with no ink. */
fun signaturePath(strokes: List<List<Offset>>, box: RectF): AndroidPath? {
    val pts = strokes.flatten()
    if (pts.isEmpty()) return null
    val minX = pts.minOf { it.x }; val maxX = pts.maxOf { it.x }
    val minY = pts.minOf { it.y }; val maxY = pts.maxOf { it.y }
    val w = (maxX - minX).coerceAtLeast(1f); val h = (maxY - minY).coerceAtLeast(1f)
    val k = minOf(box.width() / w, box.height() / h)
    val ox = box.left + (box.width() - w * k) / 2f
    val oy = box.top + (box.height() - h * k) / 2f
    val path = AndroidPath()
    strokes.forEach { s -> addSmoothStroke(path, s) { p -> Offset(ox + (p.x - minX) * k, oy + (p.y - minY) * k) } }
    return path
}

/** Ink only, black on transparent. [boundsInPad] is where the ink sits in the pad (pad-local px), padded. */
class InkImage(val bitmap: Bitmap, val boundsInPad: RectF)

/** Bounds of every point plus [pad] px on each side, or null with no ink. */
internal fun inkBounds(strokes: List<List<Offset>>, pad: Float): RectF? {
    val pts = strokes.flatten()
    if (pts.isEmpty()) return null
    return RectF(pts.minOf { it.x } - pad, pts.minOf { it.y } - pad, pts.maxOf { it.x } + pad, pts.maxOf { it.y } + pad)
}

/** Renders the drawn strokes to a trimmed bitmap (stroke [strokeWidthPx] wide, padded by [strokeWidthPx] * 2). */
fun renderDrawnInk(strokes: List<List<Offset>>, strokeWidthPx: Float): InkImage? {
    val bounds = inkBounds(strokes, strokeWidthPx * 2f) ?: return null
    val w = ceil(bounds.width()).toInt().coerceAtLeast(2)
    val h = ceil(bounds.height()).toInt().coerceAtLeast(2)
    val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
    val path = AndroidPath()
    strokes.forEach { s -> addSmoothStroke(path, s) { p -> Offset(p.x - bounds.left, p.y - bounds.top) } }
    AndroidCanvas(bmp).drawPath(path, inkPaint(strokeWidthPx))
    return InkImage(bmp, bounds)
}

/** A typed name in [typeface], black on transparent, trimmed to the text with a small margin. */
fun renderTypedInk(text: String, typeface: Typeface, textSizePx: Float): Bitmap? {
    val t = text.trim()
    if (t.isEmpty()) return null
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = AndroidColor.BLACK; this.typeface = typeface; textSize = textSizePx }
    val margin = textSizePx * 0.15f
    val fm = paint.fontMetrics
    val w = ceil(paint.measureText(t) + margin * 2).toInt().coerceAtLeast(2)
    val h = ceil(fm.descent - fm.ascent + margin * 2).toInt().coerceAtLeast(2)
    val bmp = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
    AndroidCanvas(bmp).drawText(t, margin, margin - fm.ascent, paint)
    return bmp
}

private fun inkPaint(widthPx: Float) = Paint(Paint.ANTI_ALIAS_FLAG).apply {
    color = AndroidColor.BLACK; style = Paint.Style.STROKE; strokeWidth = widthPx; strokeCap = Paint.Cap.ROUND; strokeJoin = Paint.Join.ROUND
}

/** Down-scales so the longest side is at most [maxSide] px (never up). */
fun Bitmap.scaledToMaxSide(maxSide: Int): Bitmap {
    val longest = max(width, height)
    if (longest <= maxSide || longest == 0) return this
    val k = maxSide.toFloat() / longest
    return Bitmap.createScaledBitmap(this, (width * k).toInt().coerceAtLeast(1), (height * k).toInt().coerceAtLeast(1), true)
}

/** PNG bytes that fit [maxBytes], shrinking in steps when they do not. Null if it never fits. Returns the bitmap that was encoded. */
fun encodeSignaturePng(bitmap: Bitmap, maxBytes: Int = SignRepository.MAX_SIGNATURE_BYTES): Pair<ByteArray, Bitmap>? {
    var current = bitmap
    repeat(6) {
        val out = ByteArrayOutputStream()
        current.compress(Bitmap.CompressFormat.PNG, 100, out)
        if (out.size() <= maxBytes) return out.toByteArray() to current
        current = current.scaledToMaxSide((max(current.width, current.height) * 0.7f).toInt().coerceAtLeast(1))
    }
    return null
}

/** True when no sampled pixel is transparent: a photo or scan, not ink. The flight skips the gold shimmer for these. */
fun Bitmap.isOpaqueImage(): Boolean {
    if (!hasAlpha()) return true
    val step = 12
    for (iy in 0 until step) for (ix in 0 until step) {
        val x = ((ix + 0.5f) / step * width).toInt().coerceIn(0, width - 1)
        val y = ((iy + 0.5f) / step * height).toInt().coerceIn(0, height - 1)
        if (AndroidColor.alpha(getPixel(x, y)) < 255) return false
    }
    return true
}

/** The drawing surface. Touch (and stylus) draws; [enabled] false freezes it while "Signed" writes. */
@androidx.compose.runtime.Composable
fun SignPad(state: SignPadState, strokeWidthPx: Float, enabled: Boolean = true, modifier: Modifier = Modifier) {
    val paint = androidx.compose.runtime.remember(strokeWidthPx) { inkPaint(strokeWidthPx) }
    Canvas(
        modifier.fillMaxSize().pointerInput(enabled) {
            if (!enabled) return@pointerInput
            detectDragGestures(
                onDragStart = { o -> state.current = listOf(o) },
                onDrag = { change, _ -> change.consume(); state.current = state.current + change.position },
                onDragEnd = { if (state.current.isNotEmpty()) state.strokes.add(state.current); state.current = emptyList() },
                onDragCancel = { if (state.current.isNotEmpty()) state.strokes.add(state.current); state.current = emptyList() },
            )
        },
    ) {
        val path = AndroidPath()
        state.strokes.forEach { addSmoothStroke(path, it) }
        addSmoothStroke(path, state.current)
        drawIntoCanvas { it.nativeCanvas.drawPath(path, paint) }
    }
}
