package com.allternit.feature.office.sign

import android.content.Context
import android.graphics.Bitmap
import android.os.Build
import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.semantics.clearAndSetSemantics
import kotlin.math.min

/** Reduced motion (Settings animator scale 0): the writing, flight and shimmer jump to their end. */
@Composable
internal fun rememberReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember { reduceMotion(context) }
}

internal fun reduceMotion(context: Context): Boolean =
    runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)

/** Success haptic on landing (CONFIRM from API 30, a long press before). */
internal fun View.successHaptic() {
    performHapticFeedback(if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) HapticFeedbackConstants.CONFIRM else HapticFeedbackConstants.LONG_PRESS)
}

private const val FLIGHT_MS = 700
private const val SHIMMER_MS = 1100
private val GOLD = Color(0xFFC9A45C)
private val FlightEase = CubicBezierEasing(0.32f, 0.72f, 0f, 1f)

/** Aspect-fits a [w] x [h] image inside [box], centred. */
fun fitRect(w: Float, h: Float, box: Rect): Rect {
    if (w <= 0f || h <= 0f || box.width <= 0f || box.height <= 0f) return box
    val k = min(box.width / w, box.height / h)
    val fw = w * k; val fh = h * k
    return Rect(box.center.x - fw / 2f, box.center.y - fh / 2f, box.center.x + fw / 2f, box.center.y + fh / 2f)
}

/**
 * Review & sign flight (web `after.tsx` flight, `sign-clip-beats.md` beats 6-7): the signature lifts out of where the pad was,
 * shrinks and flies to its slot, then a gold shimmer sweeps across the ink (masked to the ink's alpha, so a photo or scan
 * with no transparency gets none) and a success haptic fires. [from] and [to] are screen coordinates, the same space as
 * `SignSheet`'s `sourceRectInWindow`. Put it in a full-screen Box above the document, after the sheet has closed:
 *
 *     if (flight != null) SignFlight(flight.bitmap, flight.sourceRect, slotRectOnScreen) { placed = true; flight = null }
 *
 * [onFinished] runs once, after the shimmer; swap the placed signature in there. Reduced motion = jump (haptic and
 * [onFinished] immediately).
 */
@Composable
fun SignFlight(bitmap: Bitmap, from: Rect, to: Rect, modifier: Modifier = Modifier, onFinished: () -> Unit) {
    val view = LocalView.current
    val reduce = rememberReduceMotion()
    val image = remember(bitmap) { bitmap.asImageBitmap() }
    val shimmer = remember(bitmap) { !bitmap.isOpaqueImage() }
    val flight = remember { Animatable(0f) }
    val sweep = remember { Animatable(0f) }
    var origin by remember { mutableStateOf(Offset.Zero) }
    val start = remember(from, bitmap) { fitRect(bitmap.width.toFloat(), bitmap.height.toFloat(), from) }
    val end = remember(to, bitmap) { fitRect(bitmap.width.toFloat(), bitmap.height.toFloat(), to) }

    LaunchedEffect(bitmap, from, to) {
        if (reduce) {
            flight.snapTo(1f)
            view.successHaptic()
            onFinished()
            return@LaunchedEffect
        }
        flight.animateTo(1f, tween(FLIGHT_MS, easing = FlightEase))
        view.successHaptic()
        if (shimmer) sweep.animateTo(1f, tween(SHIMMER_MS, easing = FastOutSlowInEasing))
        onFinished()
    }

    Box(
        modifier.fillMaxSize().clearAndSetSemantics { }.onGloballyPositioned {
            val loc = IntArray(2)
            view.getLocationOnScreen(loc)
            // This overlay's own top-left on screen: the layout is full-screen, so its position in the window plus the window's.
            val p = it.positionInWindow()
            origin = Offset(loc[0] + p.x, loc[1] + p.y)
        },
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val t = flight.value
            val l = lerp(start.left, end.left, t) - origin.x
            val tp = lerp(start.top, end.top, t) - origin.y
            val w = lerp(start.width, end.width, t)
            val h = lerp(start.height, end.height, t)
            if (w <= 0f || h <= 0f) return@Canvas
            // The ink layer is composited offscreen so the shimmer band (SrcAtop) only paints where the ink has alpha.
            drawContext.canvas.saveLayer(androidx.compose.ui.geometry.Rect(l, tp, l + w, tp + h), androidx.compose.ui.graphics.Paint())
            drawImage(image, dstOffset = IntOffset(l.toInt(), tp.toInt()), dstSize = IntSize(w.toInt().coerceAtLeast(1), h.toInt().coerceAtLeast(1)))
            if (shimmer && sweep.value > 0f && sweep.value < 1f) {
                val band = w * 0.3f
                val x = l - band + (w + band * 2f) * sweep.value
                drawRect(
                    brush = Brush.horizontalGradient(listOf(Color.Transparent, GOLD, Color.Transparent), startX = x, endX = x + band),
                    topLeft = Offset(x, tp), size = Size(band, h), blendMode = BlendMode.SrcAtop,
                )
            }
            drawContext.canvas.restore()
        }
    }
}

private fun lerp(a: Float, b: Float, t: Float) = a + (b - a) * t
