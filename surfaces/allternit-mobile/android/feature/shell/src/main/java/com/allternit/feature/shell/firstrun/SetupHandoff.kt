package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlinx.coroutines.delay

/**
 * Starts the branded hand-off from setup into the app (iOS SetupHandoff). [sources] are the
 * connected graph nodes in window pixels; [finish] runs exactly once, at about 3.3 s, while the
 * flight keeps going over the freshly mounted workspace. Returns false when no host can draw it
 * (the caller then finishes at once, like iOS).
 */
internal fun interface SetupHandoffHost {
    fun start(sources: List<Offset>, name: String?, finish: () -> Unit): Boolean
}

internal val LocalSetupHandoff = staticCompositionLocalOf { SetupHandoffHost { _, _, _ -> false } }

/** A name as the person gave it, trimmed and capped so it fits under the mark; empty → null. */
internal fun handoffName(raw: String?): String? {
    val name = raw.orEmpty().trim().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
    if (name.isEmpty()) return null
    return if (name.length > 24) name.take(23) + "…" else name
}

internal class SetupHandoffSpec(val sources: List<Offset>, val name: String?, val finish: () -> Unit)

/**
 * The hand-off itself (iOS SetupHandoffView, web `setup/SetupHandoff.tsx` on the locked brand
 * intro), ~4.5 s: every connected node collapses into a block that flies in to build the A://
 * mark, the coral core pops and lands in the A, a shockwave and light sweep run, the name types
 * under the mark, the core heartbeats, "://" dissolves right to left, and the A's blocks arc up to
 * the top bar as the app shows through. A tap skips to the end. Reduce motion shows the still
 * mark and name for about a second.
 *
 * MainActivity draws it above everything, so it survives the swap from setup to the workspace.
 */
@Composable
internal fun SetupHandoffOverlay(spec: SetupHandoffSpec, dark: Boolean, onDone: () -> Unit) {
    val reduced = reducedMotion()
    val finishAt = if (reduced) 0.8 else 3.3
    val end = if (reduced) 1.1 else 4.5
    var t by remember { mutableFloatStateOf(0f) }
    var fired by remember { mutableStateOf(false) }
    var over by remember { mutableStateOf(false) }
    var size by remember { mutableStateOf(IntSize.Zero) }

    fun fire() {
        if (fired) return
        fired = true
        spec.finish()
    }
    fun stop() {
        if (over) return
        over = true
        fire()
        onDone()
    }

    LaunchedEffect(spec) {
        val start = withFrameNanos { it }
        while (!over) {
            val now = withFrameNanos { it }
            t = ((now - start) / 1e9).toFloat()
            if (t >= finishAt) fire()
            if (t >= end) stop()
        }
    }
    // Backstops: finish and tear down even if frames stop.
    LaunchedEffect(spec) {
        delay(((finishAt + 0.7) * 1000).toLong()); fire()
        delay(((end - finishAt + 0.8) * 1000).toLong()); stop()
    }

    val bg = if (dark) Color(0xFF0F0F0F) else Color(0xFFFDF8F3)
    val block = if (dark) Color(0xFFF0EEE6) else Color(0xFF141413)
    val hot = if (dark) Color.White else Color(0xFF7A5D43)
    val accent = Color(0xFFD97757)
    val accent2 = if (dark) Color(0xFFD4B08C) else Color(0xFF7A5D43)
    val grid = Color(0xFFB08D6E)
    val density = LocalDensity.current

    Box(
        Modifier.fillMaxSize()
            .onSizeChanged { size = it }
            .clearAndSetSemantics {}
            // Before finish the overlay catches taps; after it, touches reach the app.
            .then(if (!fired) Modifier.pointerInput(Unit) { detectTapGestures { stop() } } else Modifier),
    ) {
        if (size == IntSize.Zero) return@Box
        val dt = t.toDouble()
        val w = size.width.toDouble()
        val h = size.height.toDouble()
        val dp = density.density.toDouble()
        val u = min(44.0 * dp, (w * 0.62) / 15)
        val ox = w / 2 - 7.5 * u
        val my = h / 2 - 1.1 * u
        val oy = my - 2.5 * u
        val s = u * 0.85
        val k = u / 20
        val fontSp = max(13.0, min(18.0, u / dp * 0.42))
        val label = spec.name?.let { "WELCOME, $it" } ?: "WELCOME"
        val labelY = with(density) { (my + 3 * u + 18 * dp).toFloat().toDp() }

        if (reduced) {
            val op = (P(dt, 0.0, 0.2) * (1 - P(dt, finishAt, end))).toFloat()
            Canvas(Modifier.fillMaxSize().graphicsLayer { alpha = op }) {
                drawRect(bg)
                for ((gx, gy, core) in MARK) {
                    val x = ox + (gx + 0.5) * u - s / 2
                    val y = oy + (gy + 0.5) * u - s / 2
                    drawRoundRect(if (core) accent else block, Offset(x.toFloat(), y.toFloat()), Size(s.toFloat(), s.toFloat()), CornerRadius((s * 0.18).toFloat()))
                }
            }
            HandoffLabel(label, fontSp, accent2, Modifier.offset(y = labelY).graphicsLayer { alpha = op })
        } else {
            Canvas(Modifier.fillMaxSize()) {
                drawFrame(dt, w, h, u, ox, my, oy, s, k, dp, spec.sources, bg, block, hot, accent, accent2, grid, end)
            }
            val typed = (P(dt, 1.5, 2.05) * label.length).toInt()
            Column(
                Modifier.fillMaxWidth().offset(y = labelY).graphicsLayer { alpha = (1 - P(dt, 2.95, 3.2)).toFloat() },
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    HandoffLabel(label.take(typed), fontSp, accent2)
                    if (dt > 1.5 && dt < 2.6 && sin(dt * 18) > 0) {
                        Box(Modifier.padding(start = 2.dp).width((fontSp * 0.6).dp).height(fontSp.dp).background(accent))
                    }
                }
                Box(Modifier.padding(top = 10.dp).width((200 * k / dp * outExpo(P(dt, 1.9, 2.3))).dp).height(2.dp).background(grid))
            }
        }
    }
}

@Composable
private fun HandoffLabel(text: String, fontSp: Double, color: Color, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
        Text(text, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Medium, fontSize = fontSp.sp,
            letterSpacing = (fontSp * 0.38).sp, color = color, maxLines = 1)
    }
}

/** The A:// wordmark: (col, row, isCore). Cols 0–4 are the A-block, 6–14 are "://". */
private val MARK = listOf(
    Triple(2, 2, true), Triple(2, 0, false), Triple(1, 1, false), Triple(3, 1, false), Triple(0, 2, false), Triple(1, 2, false), Triple(3, 2, false),
    Triple(4, 2, false), Triple(0, 3, false), Triple(4, 3, false), Triple(0, 4, false), Triple(4, 4, false), Triple(6, 1, false), Triple(6, 3, false),
    Triple(8, 3, false), Triple(8, 4, false), Triple(9, 2, false), Triple(10, 0, false), Triple(10, 1, false), Triple(12, 3, false), Triple(12, 4, false),
    Triple(13, 2, false), Triple(14, 0, false), Triple(14, 1, false),
)

private fun cl(v: Double) = max(0.0, min(1.0, v))
private fun P(t: Double, a: Double, b: Double) = cl((t - a) / (b - a))
private fun lerpD(a: Double, b: Double, e: Double) = a + (b - a) * e
private fun outExpo(x: Double) = if (x >= 1) 1.0 else 1 - 2.0.pow(-10 * x)
private fun ioExpo(x: Double) = when {
    x <= 0 -> 0.0
    x >= 1 -> 1.0
    x < 0.5 -> 2.0.pow(20 * x - 10) / 2
    else -> (2 - 2.0.pow(-20 * x + 10)) / 2
}
private fun outBack(x: Double): Double { val c = 1.9; return 1 + (c + 1) * (x - 1).pow(3) + c * (x - 1).pow(2) }
private fun inCubic(x: Double) = x * x * x
private fun ioCubic(x: Double) = if (x < 0.5) 4 * x * x * x else 1 - (-2 * x + 2).pow(3) / 2
/** Deterministic per-block noise (same hash as the brand lib). */
private fun rr(i: Int, k: Int): Double { val x = sin(i * 127.1 + k * 311.7) * 43758.5453; return x - floor(x) }

private fun DrawScope.square(x: Double, y: Double, sz: Double, color: Color, alpha: Double, rotation: Double) {
    if (alpha <= 0.005) return
    val half = (sz / 2).toFloat()
    rotate(rotation.toFloat(), Offset(x.toFloat(), y.toFloat())) {
        drawRoundRect(color.copy(alpha = alpha.toFloat().coerceIn(0f, 1f)), Offset(x.toFloat() - half, y.toFloat() - half),
            Size(sz.toFloat(), sz.toFloat()), CornerRadius(max(1.0, sz * 0.18).toFloat()))
    }
}

private fun DrawScope.ring(t: Double, st: Double, dur: Double, r0: Double, color: Color, width: Double, cx: Double, cy: Double) {
    val p = P(t, st, st + dur)
    if (p <= 0 || p >= 1) return
    drawCircle(color.copy(alpha = (1 - p).toFloat()), (r0 * outExpo(p)).toFloat(), Offset(cx.toFloat(), cy.toFloat()), style = Stroke(width.toFloat()))
}

private fun DrawScope.drawFrame(
    t: Double, w: Double, h: Double, u: Double, ox: Double, my: Double, oy: Double, s: Double, k: Double, dp: Double,
    sources: List<Offset>, bg: Color, block: Color, hot: Color, accent: Color, accent2: Color, grid: Color, end: Double,
) {
    val bgFade = 1 - P(t, 3.25, 3.7)
    drawRect(bg.copy(alpha = (bgFade * ioCubic(P(t, 0.1, 0.7))).toFloat()))
    // Soft warm glow behind the mark.
    val glow = bgFade * 0.22 * ioCubic(P(t, 0.4, 1.6))
    if (glow > 0.001) {
        drawRect(Brush.radialGradient(listOf(grid.copy(alpha = (0.9 * glow).toFloat()), Color.Transparent),
            Offset((ox + 2.5 * u).toFloat(), my.toFloat()), (max(w, h) * 0.6).toFloat()))
    }
    val flash = if (t > 1.1) 0.18 * (1 - P(t, 1.1, 1.4)) else 0.0
    if (flash > 0) drawRect(accent.copy(alpha = flash.toFloat()))
    ring(t, 1.1, 0.6, 260 * k, accent, 4 * dp, ox + 2.5 * u, my)
    ring(t, 1.16, 0.7, 380 * k, accent2, 2 * dp, ox + 2.5 * u, my)
    ring(t, 2.55, 0.5, u * 6, accent, 2 * dp, ox + 2.5 * u, my)
    // No Home mark to land on: the A fades as it reaches the top bar.
    val tx0 = w / 2
    val ty0 = 38.0 * dp
    val tsize = 44.0 * dp
    val cell = tsize * 0.2
    val landSize = tsize * 0.17
    val sweepX = lerpD(ox - 120 * k, ox + 15 * u + 120 * k, ioCubic(P(t, 1.45, 2.05)))
    val beat = 1 + 0.2 * max(0.0, sin((t - 2.55) * 10)) * (if (t > 2.55 && t < 2.87) 1 else 0)
    val outro = 1 - P(t, end - 0.2, end)
    val far = max(520 * k, hypot(w, h) * 0.6)
    MARK.forEachIndexed { i, (gx, gy, core) ->
        val tx = ox + (gx + 0.5) * u
        val ty = oy + (gy + 0.5) * u
        val d = tx - sweepX + (ty - oy) * 0.8
        val spec = exp(-d.pow(2) / (2 * 40 * k * 40 * k)) * (if (t > 1.45 && t < 2.1) 1 else 0)
        val inA = gx < 5
        val rip = max(abs(gx - 2), abs(gy - 2)).toDouble()
        val fp = if (inA) ioExpo(P(t, 3.3 + rip * 0.05, 4.25 + rip * 0.05)) else 0.0
        val fx = tx0 + (gx - 2) * cell
        val fy = ty0 + (gy - 2) * cell
        val arc = sin(PI * fp) * -70 * k
        val dz = if (inA) 1.0 else 1 - inCubic(P(t, 2.95 + ((14 - gx) / 8.0) * 0.35, 3.15 + ((14 - gx) / 8.0) * 0.35))
        val tail = 1 - fp
        if (core) {
            val pop = outBack(P(t, 0.25, 0.6))
            val pre = 1 + 1.6 * (1 - P(t, 0.35, 0.75))
            val mv = ioExpo(P(t, 0.6, 0.95))
            val x = lerpD(lerpD(w / 2, tx, mv), fx, fp) + arc
            val y = lerpD(lerpD(my, ty, mv), fy, fp)
            val sz = lerpD(s * pop * pre * beat, landSize, fp)
            if (sz > 0) square(x, y, sz, accent, tail * outro, 0.0)
            return@forEachIndexed
        }
        val fromNode = i - 1 < sources.size
        val src = if (fromNode) sources[i - 1].x.toDouble() to sources[i - 1].y.toDouble()
        else (tx + cos(rr(i, 2) * 6.28) * far) to (ty + sin(rr(i, 2) * 6.28) * far)
        val ts = 0.15 + (if (i < 9) 0.0 else 0.45) + hypot((gx - 2).toDouble(), (gy - 2).toDouble()) * 0.04 + rr(i, 1) * 0.06
        // tr = 2, 1: short accent trails; tr = 0: the block itself.
        for (tr in 2 downTo 0) {
            val p = P(t - tr * 0.03, ts, ts + 0.6)
            if (p <= 0) continue
            if (tr > 0 && (p >= 1 || fp > 0)) continue
            val e = outExpo(p)
            val land = 1 - P(t, ts + 0.15, ts + 0.5)
            var x = lerpD(src.first, tx, e)
            var y = lerpD(src.second, ty, e)
            var sz = s * lerpD(if (fromNode) 1.6 else 0.4, 1.0, e)
            if (tr == 0 && fp > 0) { x = lerpD(tx, fx, fp) + arc; y = lerpD(ty, fy, fp); sz = lerpD(s, landSize, fp) }
            sz *= dz
            if (sz < 0.5) continue
            val lit = min(1.0, land * 0.6 + spec * 0.9)
            val color = if (tr > 0) accent2 else lerp(block, hot, lit.toFloat())
            val alpha = (if (tr > 0) 0.22 * (1 - tr / 3.0) else tail) * outro
            square(x, y, sz, color, alpha, (1 - e) * (rr(i, 4) - 0.5) * 240)
        }
    }
}

