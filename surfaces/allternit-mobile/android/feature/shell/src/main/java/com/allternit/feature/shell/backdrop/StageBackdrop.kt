package com.allternit.feature.shell.backdrop

import android.provider.Settings
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import com.allternit.core.design.AllternitTheme
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.sin

enum class BackdropSize { Full, Compact, Tile }

/**
 * The animated background behind a bot screen (iOS StageBackdropView). Everything moves slowly on
 * purpose so it never pulls the eye from the conversation. Colours come from the screen: a bot's
 * colour, a group's member colours. System reduce motion (animator scale 0), Movement: Still and
 * [still] draw one frame. Hidden from accessibility and never takes touches.
 */
@Composable
fun StageBackdrop(
    variant: String,
    modifier: Modifier = Modifier,
    colors: List<String?> = emptyList(),
    size: BackdropSize = BackdropSize.Full,
    still: Boolean = false,
    veil: Boolean = true,
    minVeil: Float = 0f,
    veilColor: Color = AllternitTheme.colors.bgPrimary,
    darkOverride: Boolean? = null,
) {
    val context = LocalContext.current
    val store = remember(context) { StageBackdropStore.get(context) }
    val dp = LocalDensity.current.density
    val systemReduceMotion = remember(context) {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val isStill = still || systemReduceMotion || store.motion == BackdropMotion.Still
    val rate = if (store.motion == BackdropMotion.Slow) 0.5 else 1.0
    val isDark = darkOverride ?: (AllternitTheme.colors.bgPrimary.luminance() < 0.5f)
    val accentColor = AllternitTheme.colors.accentPrimary

    // The clock: t = 3 s at rest (still), else 3 s + elapsed * rate at ~30 fps.
    var clock by remember { mutableStateOf(3.0) }
    LaunchedEffect(isStill, rate) {
        if (isStill) {
            clock = 3.0
        } else {
            val startNs = withFrameNanos { it }
            var last = 0L
            while (true) {
                withFrameNanos { now ->
                    if (now - last >= FRAME_NANOS) {
                        last = now
                        clock = 3.0 + (now - startNs) / 1_000_000_000.0 * rate
                    }
                }
            }
        }
    }

    Box(modifier.background(veilColor).clearAndSetSemantics {}) {
        if (variant != "plain") {
            val scene = AsciiScenes.scene(variant)
            if (scene != null) {
                val baseFont = when (size) {
                    BackdropSize.Full -> 13.0
                    BackdropSize.Compact -> 12.0
                    BackdropSize.Tile -> 5.0
                }
                val options = AsciiRenderOptions(
                    fontPx = max(4.0, Ascii.round(baseFont * store.fontScale)),
                    density = store.density,
                    colour = store.colour,
                    chars = store.chars,
                    accent = colors.filterNotNull().firstOrNull() ?: "#D97757",
                    dark = isDark,
                )
                val renderer = remember(scene.id, options, dp) { AsciiRenderer(scene, options, dp) }
                Canvas(Modifier.fillMaxSize()) {
                    val t = clock
                    drawIntoCanvas { canvas -> renderer.paint(canvas.nativeCanvas, this.size.width, this.size.height, t) }
                }
                if (veil) {
                    val level = (max(store.veil, minVeil.toDouble()) / 100).toFloat().coerceIn(0f, 1f)
                    Box(Modifier.fillMaxSize().background(veilColor.copy(alpha = veilColor.alpha * level)))
                }
            } else {
                val palette = StageBackdrops.palette(colors, accentColor)
                val scale = when (size) {
                    BackdropSize.Full -> 1.0
                    BackdropSize.Compact -> 0.7
                    BackdropSize.Tile -> 0.22
                }
                Canvas(Modifier.fillMaxSize()) {
                    drawClassic(variant, palette, isDark, scale, clock, dp.toDouble())
                }
                if (minVeil > 0f) {
                    val level = (minVeil / 100f * 0.6f).coerceIn(0f, 1f)
                    Box(Modifier.fillMaxSize().background(veilColor.copy(alpha = veilColor.alpha * level)))
                }
            }
        }
    }
}

private const val FRAME_NANOS = 33_000_000L

// MARK: - Classic

private fun off(x: Double, y: Double): Offset = Offset(x.toFloat(), y.toFloat())

private fun Color.op(a: Double): Color = copy(alpha = (alpha * a).toFloat().coerceIn(0f, 1f))

private fun Color.clear(): Color = copy(alpha = 0f)

/** 0..1..0 over [period] seconds. */
private fun swing(t: Double, period: Double, phase: Double = 0.0): Double = 0.5 - 0.5 * cos((t / period + phase) * PI * 2)

private class ClassicBlob(val color: Int, val x0: Double, val x1: Double, val y0: Double, val y1: Double, val size: Double, val period: Double)

/**
 * A soft blurred disc: a radial gradient that stays solid to (radius - blur) and fades out by
 * (radius + blur), which is what a gaussian blur of the disc looks like and works on every API.
 */
private fun DrawScope.blob(center: Offset, radius: Double, color: Color, opacity: Double, blur: Double) {
    val b = max(0.0, blur)
    val outer = max(1.0, radius + b)
    val inner = (max(0.0, radius - b) / outer).toFloat().coerceIn(0f, 0.999f)
    val solid = color.op(opacity)
    drawCircle(
        brush = Brush.radialGradient(
            0f to solid, inner to solid, 1f to solid.clear(),
            center = center,
            radius = outer.toFloat(),
        ),
        radius = outer.toFloat(),
        center = center,
    )
}

/** The thirteen Classic looks (iOS ClassicBackdropCanvas): soft shapes from the screen's colours, the web's CSS keyframes as slow sines. */
private fun DrawScope.drawClassic(variant: String, palette: List<Color>, dark: Boolean, scale: Double, t: Double, dp: Double) {
    fun pick(i: Int): Color = palette[i % max(1, palette.size)]
    val w = size.width.toDouble()
    val h = size.height.toDouble()
    val m = max(w, h)
    val u = dp
    when (variant) {
        "aurora" -> {
            val blobs = listOf(
                ClassicBlob(0, -0.10, 0.25, -0.05, 0.20, 0.60, 26.0),
                ClassicBlob(1, 0.55, 0.30, 0.10, 0.45, 0.55, 32.0),
                ClassicBlob(2, 0.20, 0.60, 0.55, 0.30, 0.50, 29.0),
            )
            for (b in blobs) {
                val k = swing(t, b.period)
                val x = (b.x0 + (b.x1 - b.x0) * k) * w + b.size * w / 2
                val y = (b.y0 + (b.y1 - b.y0) * k) * h + b.size * w / 2
                blob(off(x, y), b.size * w / 2, pick(b.color), if (dark) 0.32 else 0.28, 60 * scale * u)
            }
        }
        "mesh" -> {
            val pct = if (dark) 0.33 else 0.25
            val drift = sin(t / 20) * 0.05
            val spots = listOf(Offset((0.0 + drift).toFloat() * size.width, 0f), Offset((1.0 - drift).toFloat() * size.width, 0f), Offset(0.5f * size.width, (1.0 + drift).toFloat() * size.height))
            for ((i, at) in spots.withIndex()) {
                drawRect(
                    Brush.radialGradient(listOf(pick(i).op(pct), pick(i).clear()), center = at, radius = (m * 0.6).toFloat()),
                    size = size,
                )
            }
        }
        "constellation" -> {
            val n = if (scale >= 1) 46 else if (scale >= 0.5) 24 else 14
            val rand = Ascii.seeded(7)
            val px = DoubleArray(n)
            val py = DoubleArray(n)
            for (i in 0 until n) {
                val x = rand()
                val y = rand()
                val vx = (rand() - 0.5) * 0.03
                val vy = (rand() - 0.5) * 0.03
                px[i] = Ascii.wrap(x + vx * t, 1.0) * w
                py[i] = Ascii.wrap(y + vy * t, 1.0) * h
            }
            val reach = 95 * max(scale, 0.35) * u
            val line = if (dark) Color.White else Color(0.12f, 0.12f, 0.12f)
            for (i in 0 until n) {
                for (j in (i + 1) until max(i + 1, n)) {
                    val d = hypot(px[i] - px[j], py[i] - py[j])
                    if (d >= reach) continue
                    drawLine(line, off(px[i], py[i]), off(px[j], py[j]), strokeWidth = (0.7 * u).toFloat(), alpha = ((1 - d / reach) * 0.22).toFloat())
                }
            }
            for (i in 0 until n) {
                val r = 1.6 * max(scale, 0.5) * u
                drawCircle(pick(i), radius = r.toFloat(), center = off(px[i], py[i]), alpha = 0.75f)
            }
        }
        "spotlight" -> {
            val k = swing(t, 8.0)
            drawRect(
                Brush.radialGradient(
                    listOf(pick(0).op(0.30 + 0.1 * k), pick(0).op(0.08), pick(0).clear()),
                    center = off(w * 0.5, h * 0.3), radius = (m * (0.45 + 0.05 * k)).toFloat(),
                ),
                size = size,
            )
        }
        "horizon" -> {
            for (i in 0 until 3) {
                val id = i.toDouble()
                val base = h * (0.62 + id * 0.1)
                val path = Path()
                path.moveTo(0f, h.toFloat())
                var x = 0.0
                while (x <= w) {
                    path.lineTo(x.toFloat(), (base + sin(x / w * PI * 2 + t * (0.25 + id * 0.08) + id) * h * 0.03).toFloat())
                    x += max(4.0 * u, w / 60)
                }
                path.lineTo(w.toFloat(), h.toFloat())
                path.close()
                drawPath(path, pick(i), alpha = if (dark) 0.22f else 0.16f)
            }
        }
        "silk" -> {
            for (i in 0 until 3) {
                val id = i.toDouble()
                val path = Path()
                var x = -0.1 * w
                val y0 = h * (0.25 + id * 0.22)
                path.moveTo(x.toFloat(), y0.toFloat())
                while (x <= w * 1.1) {
                    path.lineTo(x.toFloat(), (y0 + sin(x / w * PI * 1.4 + t * 0.18 + id * 1.7) * h * 0.08).toFloat())
                    x += max(4.0 * u, w / 50)
                }
                // Three passes of narrowing width stand in for the blur of the ribbon.
                val a = if (dark) 0.30 else 0.22
                val ribbon = 70 * scale * u
                drawPath(path, pick(i), alpha = (a * 0.4).toFloat(), style = Stroke(width = (ribbon * 1.3).toFloat(), cap = StrokeCap.Round))
                drawPath(path, pick(i), alpha = (a * 0.4).toFloat(), style = Stroke(width = ribbon.toFloat(), cap = StrokeCap.Round))
                drawPath(path, pick(i), alpha = (a * 0.4).toFloat(), style = Stroke(width = (ribbon * 0.6).toFloat(), cap = StrokeCap.Round))
            }
        }
        "bokeh" -> {
            val rand = Ascii.seeded(11)
            for (i in 0 until (if (scale >= 0.5) 14 else 8)) {
                val x = rand() * w
                val speed = 0.01 + rand() * 0.02
                val r = (12 + rand() * 30) * scale * u
                val p = rand()
                val y = h - Ascii.wrap(p + t * speed, 1.0) * (h + r * 2) + r
                blob(off(x, y), r, pick(i), if (dark) 0.35 else 0.25, 6 * scale * u)
            }
        }
        "grid" -> {
            val gap = max(6.0 * u, 22 * scale * u)
            val band = Ascii.wrap(t * 0.06, 1.4) - 0.2
            var y = gap / 2
            while (y < h) {
                var x = gap / 2
                while (x < w) {
                    val d = abs((x / w + y / h) / 2 - band)
                    val glow = max(0.0, 1 - d * 6)
                    val r = (1 + glow * 1.2) * u
                    val color = if (glow > 0.05) pick(0) else if (dark) Color.White else Color.Black
                    drawCircle(color, radius = r.toFloat(), center = off(x, y), alpha = (0.14 + glow * 0.5).toFloat().coerceIn(0f, 1f))
                    x += gap
                }
                y += gap
            }
        }
        "lava" -> {
            for (i in 0 until 5) {
                val id = i.toDouble()
                val x = w * (0.2 + 0.6 * swing(t, 23 + id * 4, id * 0.2))
                val y = h * (0.15 + 0.7 * swing(t, 31 + id * 3, id * 0.37))
                blob(off(x, y), w * (0.18 + 0.05 * sin(t / 7 + id)), pick(i), if (dark) 0.34 else 0.26, 34 * scale * u)
            }
        }
        "prism" -> {
            val colors = listOf(pick(0), pick(1), pick(2), pick(0))
            rotate(degrees = (t * 3).toFloat(), pivot = off(w / 2, h / 2)) {
                drawRect(
                    Brush.sweepGradient(colors, center = off(w / 2, h / 2)),
                    topLeft = off(-w / 2, -h / 2), size = Size((w * 2).toFloat(), (h * 2).toFloat()),
                    alpha = if (dark) 0.26f else 0.2f,
                )
            }
        }
        "fireflies" -> {
            val rand = Ascii.seeded(23)
            val warm = Color(0xFFFFD166.toInt())
            for (i in 0 until (if (scale >= 0.5) 26 else 12)) {
                val x0 = rand()
                val y0 = rand()
                val p = rand() * Ascii.tau
                val x = Ascii.wrap(x0 + sin(t * 0.1 + p) * 0.05, 1.0) * w
                val y = Ascii.wrap(y0 + cos(t * 0.08 + p) * 0.05, 1.0) * h
                val on = max(0.0, sin(t * 0.9 + p))
                blob(off(x, y), 3 * max(scale, 0.5) * u, if (i % 3 == 0) warm else pick(i), 0.2 + on * 0.7, 2 * u)
            }
        }
        "dusk" -> {
            val k = swing(t, 40.0)
            drawRect(
                Brush.verticalGradient(listOf(pick(1).clear(), pick(1).op(0.10 + 0.05 * k), pick(0).op(if (dark) 0.30 else 0.24))),
                size = size,
            )
            blob(off(w / 2, h * 1.02), w * 0.5, pick(0), 0.28 + 0.08 * k, 50 * scale * u)
        }
        else -> Unit
    }
}
