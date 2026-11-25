package com.allternit.core.design

import android.graphics.Bitmap
import android.provider.Settings
import android.view.View
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import kotlin.math.exp
import kotlin.math.sin

/** The moving pattern of a dot field (the reference clip's `band` and `sine`). */
enum class DotPattern {
    /** A soft light band sweeping diagonally. */
    Band,
    /** Horizontal sine waves (audio). */
    Sine,
}

/** How a real image replaces the dots. */
enum class DotRevealStyle {
    /** Cells bloom into the image in random order. */
    Cells,
    /** The image fades in over whatever was drawn (a halftone preview). */
    Crossfade,
}

/** What a [DotField] draws. Pick one; switch modes as the real state changes. */
sealed interface DotFieldMode {
    /** Dots with a moving pattern; [amplitude] 0..1 (0.45 while queued, 1 while working). */
    data class Field(val pattern: DotPattern = DotPattern.Band, val amplitude: Float = 1f) : DotFieldMode

    /**
     * The [bitmap] drawn as halftone dots, finer as [fraction] (the REAL progress, 0..1) rises: pitch 2.2x to 0.3x the
     * base, solid image at 0.85 and up. A null fraction holds the coarse pitch. Reduced motion draws a fixed medium pitch.
     */
    data class Halftone(val bitmap: Bitmap, val fraction: Float?) : DotFieldMode

    /**
     * The real [bitmap] arrives. [DotRevealStyle.Cells] blooms it over the field; [DotRevealStyle.Crossfade] fades it over
     * the halftone ([fraction] = where the halftone was). Runs for 0.8 s, then calls `onRevealed` once. Reduced motion
     * shows the image at once.
     */
    data class Reveal(val bitmap: Bitmap, val style: DotRevealStyle = DotRevealStyle.Cells, val fraction: Float? = null) : DotFieldMode

    /**
     * The real result (a player, a page) is ready: the moving [pattern] settles flat, then the dots fade out over 0.8 s
     * and `onRevealed` is called once, so the caller can show the real content underneath. Reduced motion: the dots are
     * gone at once.
     */
    data class Settle(val pattern: DotPattern = DotPattern.Sine) : DotFieldMode
}

/** System animator scale 0 = reduce motion (the same check ThinkingOrb uses). */
@Composable
fun rememberDotReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        runCatching { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false)
    }
}

private const val BASE_PITCH_DP = 9f
private const val FRAME_MS = 33L
private const val REVEAL_MS = 800

private fun pattern(p: DotPattern, t: Float, nx: Float, ny: Float): Float {
    val cx = nx - 0.5f
    val cy = ny - 0.5f
    return when (p) {
        DotPattern.Band -> {
            val d = (cx + cy * 0.6f) - (((t * 0.35f) % 1.6f) - 0.8f)
            0.25f + 0.75f * exp(-d * d * 40f)
        }
        DotPattern.Sine -> 0.5f + 0.5f * sin(cx * 18f + sin(cy * 9f) * 1.4f - t * 3f)
    }
}

/** A stable 0..1 value for cell (cx, cy), so the reveal order does not flicker between frames. */
private fun cellRandom(cx: Int, cy: Int): Float {
    var h = cx * 73856093 xor cy * 19349663
    h = h xor (h ushr 13); h *= 1274126177; h = h xor (h ushr 16)
    return (h and 0xFFFF) / 65535f
}

/** The bitmap sampled once per (bitmap, grid), as packed ARGB. */
private class Sampled(val cols: Int, val rows: Int, val argb: IntArray)

private fun sample(bmp: Bitmap, cols: Int, rows: Int): Sampled {
    val c = cols.coerceAtLeast(1)
    val r = rows.coerceAtLeast(1)
    val small = Bitmap.createScaledBitmap(bmp, c, r, true)
    val px = IntArray(c * r)
    small.getPixels(px, 0, c, 0, 0, c, r)
    if (small !== bmp) small.recycle()
    return Sampled(c, r, px)
}

/**
 * One canvas of dots that says "working". Reusable by every waiting state (generation card, downloads, previews,
 * Files thumbnails, audio). Fills its parent: give it a size. Ink comes from the theme (primary text colour), so it
 * reads in light and graphite dark; the card behind it is the caller's.
 *
 * Frames are capped near 30 fps. It draws one still frame, with no loop, when the system animator scale is 0, and the
 * loop stops while the window is hidden, the box is off screen, or the composable has left composition.
 */
@Composable
fun DotField(
    mode: DotFieldMode,
    modifier: Modifier = Modifier,
    onRevealed: () -> Unit = {},
) {
    val reduce = rememberDotReducedMotion()
    val ink = AllternitTheme.colors.textPrimary
    val view = LocalView.current
    var onScreen by remember { mutableStateOf(true) }
    var t by remember { mutableFloatStateOf(0f) }
    var size by remember { mutableStateOf(IntSize.Zero) }
    val reveal = remember { Animatable(0f) }
    val revealMode = mode as? DotFieldMode.Reveal
    val settleMode = mode as? DotFieldMode.Settle
    val onDone by rememberUpdatedState(onRevealed)

    val animating = !reduce && (mode is DotFieldMode.Field || mode is DotFieldMode.Reveal || mode is DotFieldMode.Settle)
    if (animating) {
        LaunchedEffect(Unit) {
            var last = 0L
            val t0 = withFrameNanos { it }
            while (true) {
                withFrameNanos { now ->
                    // Window hidden (app in the background) or box scrolled away: keep the loop alive but do no work.
                    val live = onScreen && view.windowVisibility == View.VISIBLE
                    if (live && (now - last) / 1_000_000 >= FRAME_MS) { last = now; t = ((now - t0) / 1e9).toFloat() }
                }
            }
        }
    }
    LaunchedEffect(revealMode?.bitmap, revealMode?.style, settleMode != null) {
        if (revealMode == null && settleMode == null) { reveal.snapTo(0f); return@LaunchedEffect }
        if (reduce) reveal.snapTo(1f) else { reveal.snapTo(0f); reveal.animateTo(1f, tween(REVEAL_MS, easing = LinearEasing)) }
        onDone()
    }

    Canvas(
        modifier.onGloballyPositioned { c ->
            size = c.size
            val b = c.boundsInWindow()
            onScreen = c.isAttached && b.width > 0f && b.bottom > 0f && b.top < view.height && b.right > 0f && b.left < view.width
        },
    ) {
        if (size.width <= 0 || size.height <= 0) return@Canvas
        val base = BASE_PITCH_DP.dp.toPx()
        when (mode) {
            is DotFieldMode.Field -> drawField(mode.pattern, mode.amplitude, base, ink, if (reduce) 3.2f else t, reduce)
            is DotFieldMode.Halftone -> drawHalftone(mode.bitmap, mode.fraction, base, ink, reduce)
            is DotFieldMode.Reveal -> drawReveal(mode, base, ink, if (reduce) 3.2f else t, reveal.value, reduce)
            is DotFieldMode.Settle -> drawSettle(mode.pattern, base, ink, t, reveal.value, reduce)
        }
    }
}

private fun DrawScope.drawField(p: DotPattern, amp: Float, pitch: Float, ink: Color, t: Float, still: Boolean) {
    val w = size.width
    val h = size.height
    var y = pitch / 2
    while (y < h) {
        var x = pitch / 2
        while (x < w) {
            val v = if (still) 0.55f else pattern(p, t, x / w, y / h) * amp
            drawCircle(ink.copy(alpha = 0.18f + 0.6f * v), (pitch * 0.16f + pitch * 0.2f * v).coerceAtLeast(0.6f), Offset(x, y))
            x += pitch
        }
        y += pitch
    }
}

/** First 45% of [q]: the pattern's amplitude eases to 0 (flat dots). The rest: the flat dots fade out. */
private fun DrawScope.drawSettle(p: DotPattern, base: Float, ink: Color, t: Float, q: Float, still: Boolean) {
    if (still || q >= 1f) return
    val amp = (1f - q / 0.45f).coerceIn(0f, 1f)
    val fade = (1f - (q - 0.45f) / 0.55f).coerceIn(0f, 1f)
    val w = size.width
    val h = size.height
    var y = base / 2
    while (y < h) {
        var x = base / 2
        while (x < w) {
            val v = pattern(p, t, x / w, y / h) * amp + 0.15f * (1f - amp)
            drawCircle(ink.copy(alpha = (0.18f + 0.6f * v) * fade), (base * 0.16f + base * 0.2f * v).coerceAtLeast(0.6f), Offset(x, y))
            x += base
        }
        y += base
    }
}

private fun DrawScope.drawSolid(bmp: Bitmap, alpha: Float = 1f) {
    drawImage(bmp.asImageBitmap(), dstSize = androidx.compose.ui.unit.IntSize(size.width.toInt(), size.height.toInt()), alpha = alpha)
}

private fun DrawScope.drawHalftone(bmp: Bitmap, fraction: Float?, base: Float, ink: Color, still: Boolean, alpha: Float = 1f) {
    val f = if (still) 0.5f else fraction
    if (f != null && f >= 0.85f) { drawSolid(bmp, alpha); return }
    val pitch = base * (if (f == null) 2.2f else 2.2f - 1.9f * (f / 0.85f).coerceIn(0f, 1f))
    drawHalftoneAt(bmp, pitch.coerceAtLeast(base * 0.3f), alpha)
}

private fun DrawScope.drawHalftoneAt(bmp: Bitmap, pitch: Float, alpha: Float) {
    val w = size.width
    val h = size.height
    val cols = (w / pitch).toInt() + 1
    val rows = (h / pitch).toInt() + 1
    val s = sample(bmp, cols, rows)
    for (gy in 0 until rows) for (gx in 0 until cols) {
        val argb = s.argb[gy * s.cols + gx]
        val r = (argb shr 16 and 0xFF) / 255f
        val g = (argb shr 8 and 0xFF) / 255f
        val b = (argb and 0xFF) / 255f
        val l = r * 0.3f + g * 0.59f + b * 0.11f
        val radius = pitch * 0.5f * (0.35f + 0.65f * (1f - l * 0.6f))
        drawCircle(Color(r, g, b, alpha), radius, Offset(gx * pitch + pitch / 2, gy * pitch + pitch / 2))
    }
}

private fun DrawScope.drawReveal(m: DotFieldMode.Reveal, base: Float, ink: Color, t: Float, q: Float, still: Boolean) {
    if (still || q >= 1f) { drawSolid(m.bitmap); return }
    when (m.style) {
        DotRevealStyle.Crossfade -> {
            drawHalftone(m.bitmap, m.fraction, base, ink, false)
            drawSolid(m.bitmap, q)
        }
        DotRevealStyle.Cells -> {
            val w = size.width
            val h = size.height
            val cols = (w / base).toInt() + 1
            val rows = (h / base).toInt() + 1
            val s = sample(m.bitmap, cols, rows)
            for (gy in 0 until rows) for (gx in 0 until cols) {
                val x = gx * base + base / 2
                val y = gy * base + base / 2
                if (q > cellRandom(gx, gy)) {
                    drawRect(Color(s.argb[gy * s.cols + gx]), Offset(gx * base, gy * base), Size(base + 1f, base + 1f))
                } else {
                    val v = pattern(DotPattern.Band, t, x / w, y / h)
                    drawCircle(ink.copy(alpha = 0.18f + 0.6f * v), (base * 0.16f + base * 0.2f * v).coerceAtLeast(0.6f), Offset(x, y))
                }
            }
            if (q > 0.75f) drawSolid(m.bitmap, (q - 0.75f) * 4f)
        }
    }
}

/**
 * The band dot field over a box that is still loading (a WebView can't be snapshotted reliably, so there is no cells reveal).
 * Shown while [loading]; when the real ready event flips it to false the field fades out over 0.3 s, then leaves composition.
 * Reduced motion: still dots, and they vanish at once. Fills its parent; the caller's background sits behind it.
 */
@Composable
fun DotLoadingOverlay(loading: Boolean, modifier: Modifier = Modifier) {
    val reduce = rememberDotReducedMotion()
    val alpha by androidx.compose.animation.core.animateFloatAsState(
        targetValue = if (loading) 1f else 0f,
        animationSpec = tween(if (reduce) 0 else 300, easing = LinearEasing),
        label = "dotLoadingAlpha",
    )
    if (loading || alpha > 0f) {
        DotField(DotFieldMode.Field(DotPattern.Band), modifier.graphicsLayer { this.alpha = alpha })
    }
}
