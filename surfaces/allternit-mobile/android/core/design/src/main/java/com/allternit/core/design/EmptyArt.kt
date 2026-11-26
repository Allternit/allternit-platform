package com.allternit.core.design

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableFloatState
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.MotionDurationScale
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.isSpecified
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.floor
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * Empty-state art. A port of reference/EmptyArt.reference.tsx (PLAN-v2 1.1): same paths, same 3.4 s loop,
 * same per-segment easings and still frames. Never redraw; change the reference first.
 * One colour (text primary), 1.5 unit stroke, round caps and joins, paper fill = the screen background.
 * Animator scale 0 (reduce motion) shows the still frame and nothing moves.
 *
 * v2 (empty-art-v2, 2026-11-25): the 13 kinds after Coins are ported mechanically from empty-art-v2/spec.json
 * (generated from art.py): every rect/circle/ellipse is its SVG path, every CSS animation is sampled per property
 * with the same keyframes, easings and delays, and group transforms use the fill-box origins of the spec.
 * Search and Model draw inside the Gizzi scene (viewBox 120 x 72, width = round(size * 1.67)); Gizzi is the mascot
 * walking in; ConnectScene is the Connect sheet scene asleep. Sizes: full 96, panel 64, inline 36.
 */
enum class EmptyArtKind {
    Sheets, Bell, Envelope, Coins,
    Search, Phone, Log, Key, Apps, Memory, Model, Chart, People, Archive, Flow, Network, Calendar,
    Gizzi, ConnectScene,
}

private const val LOOP_S = 3.4f
private const val VIEWBOX = 96f

/** Width of the scenes (Search, Model, ConnectScene): round(size * 1.67), height = size. */
private fun sceneWidth(size: Dp): Dp = (size.value * 1.67f).roundToInt().dp

/** Every v2 loop (3.4 s and 0.5 s) repeats every 17 s; delays (max 1.9 s) only hold the first pass. */
private const val CLOCK_WRAP = 17.0

private val EaOut = CubicBezierEasing(.16f, 1f, .3f, 1f)
private val EaIn = CubicBezierEasing(.7f, 0f, .84f, 0f)
private val EaIo = CubicBezierEasing(.65f, 0f, .35f, 1f)
private val EaDefault = CubicBezierEasing(.25f, .1f, .25f, 1f) // CSS `ease`

/** One keyframe: [p] percent of the loop, [e] easing of the segment that starts here. */
private class K(val p: Float, val v: Float, val e: Easing? = null)

/** CSS-style keyframe sampling. [ph] is 0..1 of the loop. */
private fun kf(ph: Float, def: Easing, vararg k: K): Float {
    val pct = ph * 100f
    if (pct <= k.first().p) return k.first().v
    if (pct >= k.last().p) return k.last().v
    for (i in 0 until k.size - 1) {
        val a = k[i]; val b = k[i + 1]
        if (pct >= a.p && pct < b.p) {
            val span = b.p - a.p
            if (span <= 0f) return b.v
            val f = (a.e ?: def).transform((pct - a.p) / span)
            return a.v + (b.v - a.v) * f
        }
    }
    return k.last().v
}

private fun kf(ph: Float, vararg k: K) = kf(ph, EaDefault, *k)

private val pathCache = HashMap<String, Path>()
private fun pd(d: String): Path = pathCache.getOrPut(d) { PathParser().parsePathString(d).toPath() }

private fun slabTop(y: Int) = "M48 ${y - 10} L72 $y L48 ${y + 10} L24 $y Z"
private fun slabLeft(y: Int) = "M24 $y V${y + 5} L48 ${y + 15} V${y + 10}"
private fun slabRight(y: Int) = "M72 $y V${y + 5} L48 ${y + 15}"
private fun coinEdge(cx: Float, cy: Float, rx: Float, ry: Float) =
    "M${cx - rx} $cy v5 a$rx $ry 0 0 0 ${rx * 2} 0 v-5"

private class Ink(val ink: Color, val paper: Color) {
    val stroke = Stroke(width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round)
    val dashed = Stroke(width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round, pathEffect = PathEffect.dashPathEffect(floatArrayOf(3f, 3f)))
    private val dashes = HashMap<String, Stroke>()

    /** An SVG stroke-dasharray ("2 3"); an odd list repeats, as in SVG. */
    fun dash(spec: String): Stroke = dashes.getOrPut(spec) {
        val a = spec.trim().split(' ', ',').filter { it.isNotEmpty() }.map { it.toFloat() }
        val even = if (a.size % 2 == 1) a + a else a
        Stroke(width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round, pathEffect = PathEffect.dashPathEffect(even.toFloatArray()))
    }
}

/**
 * Phase 0..1 of a CSS animation [dur] s long that starts [delay] s in, at time [t] s; null = the still frame
 * (reduced motion, or the delay has not run out yet: CSS shows the base style then, as there is no fill-mode).
 */
private fun ph(t: Float?, dur: Float, delay: Float = 0f): Float? {
    if (t == null || t < delay) return null
    val x = (t - delay) / dur
    return x - floor(x)
}

/** One property's keyframes at phase [p]; [base] is its still value. [def] is the animation's own easing. */
private fun kv(p: Float?, base: Float, def: Easing, vararg k: K): Float = if (p == null) base else kf(p, def, *k)

/** One SVG shape (as its path): paper fill, ink fill ([fillInk] = fill-opacity), then the 1.5 stroke. */
private fun DrawScope.shape(
    k: Ink, d: String, paper: Boolean = false, dash: String? = null, alpha: Float = 1f, fillInk: Float = 0f, stroke: Boolean = true,
) {
    if (alpha <= 0.001f) return
    val p = pd(d)
    if (paper) drawPath(p, k.paper, alpha = alpha, style = Fill)
    if (fillInk > 0f) drawPath(p, k.ink, alpha = fillInk * alpha, style = Fill)
    if (stroke) drawPath(p, k.ink, alpha = alpha, style = if (dash != null) k.dash(dash) else k.stroke)
}

private val lengthCache = HashMap<String, Float>()

/** A stroke with an animated dash in pathLength units ([pathLength] 0 = user units). A zero gap draws it solid. */
private fun DrawScope.measured(k: Ink, d: String, pathLength: Float, dash: FloatArray, offset: Float, alpha: Float = 1f) {
    if (alpha <= 0.001f) return
    val p = pd(d)
    val len = lengthCache.getOrPut(d) { PathMeasure().apply { setPath(p, false) }.length }
    val u = if (pathLength > 0f) len / pathLength else 1f
    val solid = dash.indices.filter { it % 2 == 1 }.all { dash[it] <= 0f }
    val style = if (solid) k.stroke else Stroke(
        width = 1.5f, cap = StrokeCap.Round, join = StrokeJoin.Round,
        pathEffect = PathEffect.dashPathEffect(FloatArray(dash.size) { dash[it] * u }, offset * u),
    )
    drawPath(p, k.ink, alpha = alpha, style = style)
}

private fun DrawScope.line(k: Ink, d: String, paper: Boolean = false, dashed: Boolean = false) {
    val p = pd(d)
    if (paper) drawPath(p, k.paper, style = Fill)
    drawPath(p, k.ink, style = if (dashed) k.dashed else k.stroke)
}

private fun DrawScope.oval(k: Ink, cx: Float, cy: Float, rx: Float, ry: Float, paper: Boolean = false) {
    val tl = Offset(cx - rx, cy - ry); val sz = Size(rx * 2, ry * 2)
    if (paper) drawOval(k.paper, tl, sz, style = Fill)
    drawOval(k.ink, tl, sz, style = k.stroke)
}

private fun DrawScope.circle(k: Ink, cx: Float, cy: Float, r: Float, paper: Boolean = false) = oval(k, cx, cy, r, r, paper)

/** Group opacity: the whole group composites once, like an SVG group. */
private inline fun DrawScope.layer(alpha: Float, block: DrawScope.() -> Unit) {
    if (alpha >= 0.999f) { block(); return }
    if (alpha <= 0.001f) return
    val paint = Paint().apply { this.alpha = alpha }
    val canvas = drawContext.canvas
    canvas.saveLayer(Rect(-200f, -200f, 300f, 300f), paint)
    block()
    canvas.restore()
}

private fun DrawScope.sheets(k: Ink, ph: Float?) {
    // ghost: 0..1 hover -6, lift -14, land +16, squash 14, rebound 16, settle -6
    val ghostY = if (ph == null) -6f else kf(
        ph, K(0f, -6f), K(29.4f, -6f, EaOut), K(41.2f, -14f, EaIn), K(52f, 16f), K(57.9f, 14f, EaIo), K(64.7f, 16f, EaIo), K(100f, -6f),
    )
    val stackY = if (ph == null) 0f else kf(ph, K(0f, 0f), K(52f, 0f), K(57.9f, 2f, EaIo), K(64.7f, 0f), K(100f, 0f))
    val impOp = if (ph == null) 0f else kf(ph, K(0f, 0f), K(52f, 0f), K(57.9f, 1f, EaOut), K(64.7f, 0f), K(100f, 0f))
    val impSc = if (ph == null) 1f else kf(ph, K(0f, 1f), K(52f, 1f), K(57.9f, 1.12f, EaOut), K(64.7f, 1.12f), K(100f, 1.12f))
    translate(0f, stackY) {
        for (y in intArrayOf(70, 62, 54)) {
            line(k, slabTop(y), paper = true); line(k, slabLeft(y)); line(k, slabRight(y))
        }
    }
    line(k, "M18 76 l5 3 M74 79 l5 -3")
    layer(impOp) {
        withTransform({ scale(impSc, impSc, Offset(48f, 62f)) }) {
            line(k, "M20 56 l-5 -2 M20 62 l-6 0 M20 68 l-5 2 M76 56 l5 -2 M76 62 l6 0 M76 68 l5 2")
        }
    }
    translate(0f, ghostY) {
        line(k, slabTop(38), dashed = true)
        line(k, "M42 35 l12 6 M54 35 l-12 6")
    }
}

private fun DrawScope.bell(k: Ink, ph: Float?) {
    val domeY = if (ph == null) 0f else kf(ph, K(0f, 0f), K(14.7f, 0f, EaIn), K(22f, 3f, EaOut), K(36.8f, 0f), K(100f, 0f))
    val rippleOp = if (ph == null) 0f else kf(ph, K(0f, .6f), K(14.7f, .6f), K(36.8f, 0f, EaOut), K(100f, 0f))
    val rippleSc = if (ph == null) 1f else kf(ph, K(0f, 1f), K(14.7f, 1f), K(36.8f, 1.45f, EaOut), K(100f, 1.45f))
    val raySc = if (ph == null) 1f else kf(ph, K(0f, 1f), K(14.7f, 1f), K(29f, 1.18f, EaIo), K(44.1f, 1f), K(100f, 1f))
    val rayOp = if (ph == null) .9f else kf(ph, K(0f, .9f), K(14.7f, .9f), K(29f, 1f, EaIo), K(44.1f, .9f), K(100f, .9f))
    val chkOp = if (ph == null) 1f else kf(
        ph, K(0f, 0f, EaOut), K(51.5f, 0f, EaOut), K(57.4f, 1f, EaIn), K(75f, 1f, EaIn), K(88.2f, 0f), K(100f, 0f),
    )
    val chkY = if (ph == null) 0f else kf(
        ph, K(0f, 4f, EaOut), K(51.5f, 4f, EaOut), K(57.4f, 0f, EaIn), K(75f, 0f, EaIn), K(88.2f, -3f), K(100f, -3f),
    )
    oval(k, 48f, 70f, 26f, 8f, paper = true)
    layer(rippleOp) {
        withTransform({ scale(rippleSc, rippleSc, Offset(48f, 70f)) }) { oval(k, 48f, 70f, 26f, 8f) }
    }
    translate(0f, domeY) {
        line(k, "M28 66 A20 20 0 0 1 68 66 Z", paper = true)
        line(k, "M36 56 A12 12 0 0 1 44 47")
        line(k, "M48 46 V42")
        circle(k, 48f, 40f, 3f, paper = true)
    }
    layer(rayOp) {
        withTransform({ scale(raySc, raySc, Offset(48f, 40f)) }) {
            line(k, "M48 26 V20 M34 30 L30 25 M62 30 L66 25 M24 40 L18 37 M72 40 L78 37")
        }
    }
    layer(chkOp) {
        translate(0f, chkY) {
            circle(k, 48f, 10f, 7f, paper = true)
            line(k, "M44.5 10 l2.5 2.5 l5 -5")
        }
    }
}

private fun DrawScope.envelope(k: Ink, ph: Float?, dot: FloatArray) {
    val flap = if (ph == null) -1f else kf(ph, K(0f, 1f), K(22f, 1f, EaOut), K(36.8f, -1f, EaIo), K(85.3f, -1f, EaIo), K(97f, 1f), K(100f, 1f))
    val bubOp = if (ph == null) 1f else kf(ph, K(0f, 0f), K(22f, 0f, EaOut), K(39.7f, 1f, EaIn), K(85.3f, 1f, EaIn), K(94.1f, 0f), K(100f, 0f))
    val bubY = if (ph == null) 0f else kf(ph, K(0f, 10f), K(22f, 10f, EaOut), K(39.7f, 0f, EaIn), K(85.3f, 0f, EaIn), K(94.1f, 0f), K(100f, 0f))
    val tilt = if (ph == null) 0f else kf(ph, K(0f, 0f), K(58.8f, 0f), K(69f, -4f, EaIo), K(79.4f, 3f, EaIo), K(100f, 0f))
    val s1x = if (ph == null) 0f else kf(ph, K(0f, 0f), K(58.8f, 0f), K(69f, 2f), K(100f, 0f))
    val s1y = if (ph == null) 0f else kf(ph, K(0f, 0f), K(58.8f, 0f), K(69f, -4f), K(100f, 0f))
    val s2x = -s1x
    val s2y = -s1y
    val sOp = if (ph == null) 1f else kf(ph, K(0f, .4f), K(58.8f, .4f), K(69f, 1f), K(100f, .4f))
    withTransform({ translate(12f, 19f); scale(.75f, .75f, Offset.Zero) }) {
        for (y in intArrayOf(76, 70)) { line(k, slabTop(y), paper = true); line(k, slabLeft(y)); line(k, slabRight(y)) }
    }
    withTransform({ rotate(tilt, Offset(48f, 70f)) }) {
        drawRoundRect(k.paper, Offset(22f, 40f), Size(52f, 30f), androidx.compose.ui.geometry.CornerRadius(3f, 3f), style = Fill)
        drawRoundRect(k.ink, Offset(22f, 40f), Size(52f, 30f), androidx.compose.ui.geometry.CornerRadius(3f, 3f), style = k.stroke)
        line(k, "M22 43 L48 60 L74 43")
        withTransform({ scale(1f, flap, Offset(48f, 40f)) }) { line(k, "M22 40 L48 58 L74 40 Z", paper = true) }
    }
    layer(bubOp) {
        translate(0f, bubY) {
            drawRoundRect(k.paper, Offset(34f, 14f), Size(28f, 14f), androidx.compose.ui.geometry.CornerRadius(7f, 7f), style = Fill)
            drawRoundRect(k.ink, Offset(34f, 14f), Size(28f, 14f), androidx.compose.ui.geometry.CornerRadius(7f, 7f), style = k.stroke)
            for (i in 0..2) drawCircle(k.ink, 1.6f, Offset(42f + 6f * i, 21f + dot[i]))
        }
    }
    layer(sOp) {
        translate(s1x, s1y) { line(k, "M80 27 V33 M77 30 H83") }
        translate(s2x, s2y) { line(k, "M14 47 V53 M11 50 H17") }
    }
}

private fun DrawScope.coins(k: Ink, ph: Float?) {
    val fy = if (ph == null) 0f else kf(ph, K(0f, 0f), K(14.7f, 0f, EaOut), K(25f, -20f), K(35.3f, -20f, EaIn), K(55.9f, 0f), K(100f, 0f))
    val fs = if (ph == null) 1f else kf(ph, K(0f, 1f), K(14.7f, 1f, EaOut), K(25f, .05f), K(35.3f, -1f, EaIn), K(55.9f, 1f), K(100f, 1f))
    val sy = if (ph == null) 0f else kf(ph, K(0f, 0f), K(55.9f, 0f), K(61f, 2f, EaOut), K(64.7f, -1f), K(67.6f, 0f), K(100f, 0f))
    val lean = if (ph == null) -18f else kf(ph, K(0f, -18f), K(55.9f, -18f), K(61f, -14f, EaOut), K(67.6f, -18f), K(100f, -18f))
    val spX = if (ph == null) 0f else kf(ph, EaIo, K(0f, 0f), K(33f, 4f), K(66f, -2f), K(100f, 0f))
    val spY = if (ph == null) 0f else kf(ph, EaIo, K(0f, 0f), K(33f, 4f), K(66f, 6f), K(100f, 0f))
    val spR = if (ph == null) 0f else kf(ph, EaIo, K(0f, 0f), K(33f, 30f), K(66f, 60f), K(100f, 90f))
    val spO = if (ph == null) 1f else kf(ph, EaIo, K(0f, .5f), K(33f, 1f), K(66f, .6f), K(100f, .5f))
    withTransform({ rotate(lean, Offset(74f, 70f)) }) {
        line(k, coinEdge(72f, 66f, 13f, 5f))
        oval(k, 72f, 66f, 13f, 5f, paper = true)
    }
    translate(0f, sy) {
        for (cy in floatArrayOf(68f, 62f)) {
            line(k, coinEdge(40f, cy, 16f, 6f), paper = true)
            oval(k, 40f, cy, 16f, 6f, paper = true)
        }
    }
    // The flipping coin turns about its own centre (fill-box centre: x 40, y 58.5).
    withTransform({ translate(0f, fy); scale(1f, fs, Offset(40f, 58.5f)) }) {
        line(k, coinEdge(40f, 56f, 16f, 6f), paper = true)
        oval(k, 40f, 56f, 16f, 6f, paper = true)
        line(k, "M37 56 h6")
    }
    layer(spO) {
        withTransform({ translate(spX, spY); rotate(spR, Offset(80f, 26f)) }) { line(k, "M80 22 l3 4 l-3 4 l-3 -4 Z") }
    }
}

/**
 * The empty-state art. [size] is the render height: 96 dp full, 64 dp panel, 36 dp inline (square drawings; the
 * scenes Search, Model and ConnectScene are round(size * 1.67) wide). Decorative, so it is hidden from accessibility;
 * the headline and line under it carry the meaning. [still] forces the still frame. [paper] is the fill of the solid
 * parts (default the screen background); set it only when the art sits on a filled card.
 */
@Composable
fun EmptyArt(kind: EmptyArtKind, modifier: Modifier = Modifier, size: Dp = 96.dp, still: Boolean = false, paper: Color = Color.Unspecified) {
    val reduce = rememberDotReducedMotion() || still
    val c = AllternitTheme.colors
    val paperColor = if (paper.isSpecified) paper else c.bgPrimary
    when (kind) {
        EmptyArtKind.Gizzi -> GizziWalkBox(modifier, (size.value * 0.75f).roundToInt().dp, size, reduce)
        EmptyArtKind.ConnectScene ->
            ConnectScene(ConnectSceneState.Asleep, modifier, width = sceneWidth(size), paper = paperColor, still = reduce)
        else -> {
            val ink = remember(c.textPrimary, paperColor) { Ink(c.textPrimary, paperColor) }
            val clock = rememberArtClock(!reduce)
            if (kind == EmptyArtKind.Search || kind == EmptyArtKind.Model) {
                GizziScene(kind, ink, clock, reduce, modifier, size)
            } else {
                Canvas(modifier.size(size).clearAndSetSemantics {}) {
                    val s = this.size.width / VIEWBOX
                    withTransform({ scale(s, s, Offset.Zero) }) { drawKind(kind, ink, if (reduce) null else clock.floatValue) }
                }
            }
        }
    }
}

/** Seconds since the art appeared (read only while drawing, so a frame redraws without recomposing). */
@Composable
private fun rememberArtClock(run: Boolean): MutableFloatState {
    val t = remember { mutableFloatStateOf(0f) }
    if (run) {
        LaunchedEffect(Unit) {
            val speed = (coroutineContext[MotionDurationScale]?.scaleFactor ?: 1f).takeIf { it > 0f } ?: 1f
            val start = withFrameNanos { it }
            while (true) {
                withFrameNanos { now ->
                    val s = (now - start) / 1e9 / speed
                    t.floatValue = (if (s < CLOCK_WRAP) s else CLOCK_WRAP + (s - CLOCK_WRAP) % CLOCK_WRAP).toFloat()
                }
            }
        }
    }
    return t
}

/**
 * The Gizzi scene for Search and Model (reference GizziScene): viewBox 120 x 72 fitted into round(size * 1.67) x size,
 * Gizzi (36 units) centred in the slot x 2 y 14 40 x 50, the drawing in a 78 x 78 box at (42, -3).
 * Native has no attention target, and Android has one mascot image, so Gizzi shows its shipped still face.
 */
@Composable
private fun GizziScene(kind: EmptyArtKind, ink: Ink, clock: MutableFloatState, reduce: Boolean, modifier: Modifier, size: Dp) {
    val w = sceneWidth(size)
    val sc = min(w.value / 120f, size.value / 72f)
    val ox = (w.value - 120f * sc) / 2f
    val oy = (size.value - 72f * sc) / 2f
    Box(modifier.size(w, size).clearAndSetSemantics {}) {
        Image(
            painter = painterResource(R.drawable.gizzi_mascot), contentDescription = null,
            modifier = Modifier.offset(x = (ox + (22f - 18f) * sc).dp, y = (oy + (39f - 18f) * sc).dp).size((36f * sc).dp),
        )
        Canvas(Modifier.matchParentSize()) {
            val u = sc.dp.toPx()
            val dx = ox.dp.toPx()
            val dy = oy.dp.toPx()
            withTransform({
                translate(dx, dy); scale(u, u, Offset.Zero)
                translate(42f, -3f); scale(78f / VIEWBOX, 78f / VIEWBOX, Offset.Zero)
            }) { drawKind(kind, ink, if (reduce) null else clock.floatValue) }
        }
    }
}

/** Draws one line drawing in its 96-unit box at time [t] s (null = the still frame). */
private fun DrawScope.drawKind(kind: EmptyArtKind, ink: Ink, t: Float?) {
    val phase = t?.let { (it % LOOP_S) / LOOP_S }
    when (kind) {
        EmptyArtKind.Sheets -> sheets(ink, phase)
        EmptyArtKind.Bell -> bell(ink, phase)
        EmptyArtKind.Coins -> coins(ink, phase)
        EmptyArtKind.Envelope -> {
            // ea-dot: 0.5 s bob, 0 -> -2.5 -> 0, staggered 0.12 s; still frame has no bob.
            val dotPhase = if (t == null) 0f else (t % 0.5f) / 0.5f
            val dots = FloatArray(3) { i ->
                if (phase == null) 0f else {
                    val q = ((dotPhase - 0.24f * i) % 1f + 1f) % 1f
                    kf(q, K(0f, 0f), K(50f, -2.5f), K(100f, 0f))
                }
            }
            envelope(ink, phase, dots)
        }
        EmptyArtKind.Search -> search(ink, t)
        EmptyArtKind.Phone -> phone(ink, t)
        EmptyArtKind.Log -> log(ink, t)
        EmptyArtKind.Key -> key(ink, t)
        EmptyArtKind.Apps -> apps(ink, t)
        EmptyArtKind.Memory -> memory(ink, t)
        EmptyArtKind.Model -> model(ink, t)
        EmptyArtKind.Chart -> chart(ink, t)
        EmptyArtKind.People -> people(ink, t)
        EmptyArtKind.Archive -> archive(ink, t)
        EmptyArtKind.Flow -> flow(ink, t)
        EmptyArtKind.Network -> network(ink, t)
        EmptyArtKind.Calendar -> calendar(ink, t)
        EmptyArtKind.Gizzi, EmptyArtKind.ConnectScene -> Unit // their own composables
    }
}

// ---- v2 drawings, generated from empty-art-v2/spec.json (do not hand-edit geometry or timing) ----

// search: every "No X match / found" after a search or filter (Gizzi scene)
// glass sweeps right 0.3-1.3, settles; "?" badge 1.5-2.95; glass back 2.6-3.4. Still: .es-scan opacity:0; .es-q opacity:1
private fun DrawScope.search(k: Ink, t: Float?) {
    val p7 = ph(t, 3.4f)
    val tx7 = kv(p7, 0f, EaDefault, K(0f, 0f, EaIo), K(8.8f, 0f, EaIo), K(38.2f, 14f, EaOut), K(44.1f, 12f, EaIo), K(76.5f, 12f, EaIo), K(100f, 0f))
    val ty7 = kv(p7, 0f, EaDefault, K(0f, 0f, EaIo), K(8.8f, 0f, EaIo), K(38.2f, -2f, EaOut), K(44.1f, -1f, EaIo), K(76.5f, -1f, EaIo), K(100f, 0f))
    val o11 = kv(p7, 0f, EaDefault, K(0f, 0f), K(8.8f, 0f), K(14.7f, 1f), K(38.2f, 1f), K(44.1f, 0f), K(100f, 0f))
    val ty11 = kv(p7, 0f, EaDefault, K(0f, -5f), K(8.8f, -5f), K(26.5f, 5f), K(38.2f, -5f), K(44.1f, -5f), K(100f, -5f))
    val o13 = kv(p7, 1f, EaDefault, K(0f, 0f, EaOut), K(44.1f, 0f, EaOut), K(54.4f, 1f, EaIn), K(76.5f, 1f, EaIn), K(86.8f, 0f), K(100f, 0f))
    val ty13 = kv(p7, 0f, EaDefault, K(0f, 4f, EaOut), K(44.1f, 4f, EaOut), K(54.4f, 0f, EaIn), K(76.5f, 0f, EaIn), K(86.8f, -3f), K(100f, -3f))
    val p16 = ph(t, 0.5f)
    val ty16 = kv(p16, 0f, EaDefault, K(0f, 0f), K(50f, -2.5f), K(100f, 0f))
    shape(k, "M48 60 L72 70 L48 80 L24 70 Z", paper = true)
    shape(k, "M24 70 V75 L48 85 V80")
    shape(k, "M72 70 V75 L48 85")
    shape(k, "M40 66 l10 5 M44 64 l10 5", alpha = 0.5f)
    // es-glass
    translate(tx7, ty7) {
        shape(k, "M49.2 52.8 L56.5 60.1 a2.2 2.2 0 0 0 3.1 -3.1 L52.3 49.7 Z", paper = true)
        shape(k, "M53 42 A13 13 0 0 1 40 55 A13 13 0 0 1 27 42 A13 13 0 0 1 40 29 A13 13 0 0 1 53 42 Z", paper = true)
        shape(k, "M33 38 a8 8 0 0 1 5 -5")
        // es-scan
        layer(o11) {
            translate(0f, ty11) {
                shape(k, "M31 44 h18", dash = "2 3")
            }
        }
    }
    // es-q
    layer(o13) {
        translate(0f, ty13) {
            shape(k, "M83 18 A8 8 0 0 1 75 26 A8 8 0 0 1 67 18 A8 8 0 0 1 75 10 A8 8 0 0 1 83 18 Z", paper = true)
            shape(k, "M72.4 15.6 a2.7 2.7 0 1 1 3.7 2.5 c-.8 .3 -1.1 .9 -1.1 1.6 v.4")
            // ea-dot
            translate(0f, ty16) {
                shape(k, "M75.9 22.6 A0.9 0.9 0 0 1 75 23.5 A0.9 0.9 0 0 1 74.1 22.6 A0.9 0.9 0 0 1 75 21.7 A0.9 0.9 0 0 1 75.9 22.6 Z", fillInk = 1f, stroke = false)
            }
        }
    }
}

// phone: linked phones, texts, phone apps
// waves pulse out; two dashed (empty) bubbles rise; phone tilts 2.0-2.7 like the envelope. Still: .ep-w1 opacity:0.9; .ep-w2 opacity:0.9; .ep-w3 opacity:0.9
private fun DrawScope.phone(k: Ink, t: Float?) {
    val p3 = ph(t, 3.4f)
    val r3 = kv(p3, 0f, EaDefault, K(0f, 0f), K(58.8f, 0f), K(69.1f, -4f, EaIo), K(79.4f, 3f, EaIo), K(88.2f, 0f), K(100f, 0f))
    val o7 = kv(p3, 1f, EaDefault, K(0f, 0f, EaOut), K(22.1f, 0f, EaOut), K(38.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(86.8f, 0f), K(100f, 0f))
    val ty7 = kv(p3, 0f, EaDefault, K(0f, 4f, EaOut), K(22.1f, 4f, EaOut), K(38.2f, 0f, EaIn), K(76.5f, 0f, EaIn), K(86.8f, 0f), K(100f, 0f))
    val p9 = ph(t, 3.4f, 0.25f)
    val o9 = kv(p9, 1f, EaDefault, K(0f, 0f, EaOut), K(22.1f, 0f, EaOut), K(38.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(86.8f, 0f), K(100f, 0f))
    val ty9 = kv(p9, 0f, EaDefault, K(0f, 4f, EaOut), K(22.1f, 4f, EaOut), K(38.2f, 0f, EaIn), K(76.5f, 0f, EaIn), K(86.8f, 0f), K(100f, 0f))
    val o11 = kv(p3, 0.9f, EaIo, K(0f, 0.15f), K(8.8f, 1f, EaIo), K(26.5f, 0.15f), K(100f, 0.15f))
    val p12 = ph(t, 3.4f, 0.2f)
    val o12 = kv(p12, 0.9f, EaIo, K(0f, 0.15f), K(8.8f, 1f, EaIo), K(26.5f, 0.15f), K(100f, 0.15f))
    val p13 = ph(t, 3.4f, 0.4f)
    val o13 = kv(p13, 0.9f, EaIo, K(0f, 0.15f), K(8.8f, 1f, EaIo), K(26.5f, 0.15f), K(100f, 0.15f))
    shape(k, "M67 66 A18 4.5 0 0 1 49 70.5 A18 4.5 0 0 1 31 66 A18 4.5 0 0 1 49 61.5 A18 4.5 0 0 1 67 66 Z", paper = true)
    // ep-body
    withTransform({ translate(49f, 64f); rotate(r3, Offset.Zero); translate(-49f, -64f) }) {
        shape(k, "M41 14 H57 A5 5 0 0 1 62 19 V59 A5 5 0 0 1 57 64 H41 A5 5 0 0 1 36 59 V19 A5 5 0 0 1 41 14 Z", paper = true)
        shape(k, "M41 22 H57 A2 2 0 0 1 59 24 V54 A2 2 0 0 1 57 56 H41 A2 2 0 0 1 39 54 V24 A2 2 0 0 1 41 22 Z")
        shape(k, "M45 18 h8 M44 60 h10")
        // ep-b ep-b1
        layer(o7) {
            translate(0f, ty7) {
                shape(k, "M45 27 H51 A3.5 3.5 0 0 1 54.5 30.5 V30.5 A3.5 3.5 0 0 1 51 34 H45 A3.5 3.5 0 0 1 41.5 30.5 V30.5 A3.5 3.5 0 0 1 45 27 Z", dash = "2 2")
            }
        }
        // ep-b ep-b2
        layer(o9) {
            translate(0f, ty9) {
                shape(k, "M47 38 H53 A3.5 3.5 0 0 1 56.5 41.5 V41.5 A3.5 3.5 0 0 1 53 45 H47 A3.5 3.5 0 0 1 43.5 41.5 V41.5 A3.5 3.5 0 0 1 47 38 Z", dash = "2 2")
            }
        }
    }
    shape(k, "M66 21 a6 6 0 0 1 5 5", alpha = o11)
    shape(k, "M67 14 a12 12 0 0 1 11 11", alpha = o12)
    shape(k, "M68 7 a18 18 0 0 1 17 17", alpha = o13)
}

// log: logs, events, steps, traces, activity, history, versions, deploys
// a dot runs down the line 0.3-2.3; each node pops and its dashed row lights as the dot passes. Still: .el-r1 opacity:0.6; .el-r2 opacity:0.6; .el-r3 opacity:0.6; .el-dot opacity:0
private fun DrawScope.log(k: Ink, t: Float?) {
    val p3 = ph(t, 3.4f, 0.57f)
    val o3 = kv(p3, 0.6f, EaDefault, K(0f, 0.35f), K(5.3f, 1f), K(29.4f, 0.35f), K(100f, 0.35f))
    val p5 = ph(t, 3.4f, 1.23f)
    val o5 = kv(p5, 0.6f, EaDefault, K(0f, 0.35f), K(5.3f, 1f), K(29.4f, 0.35f), K(100f, 0.35f))
    val p7 = ph(t, 3.4f, 1.9f)
    val o7 = kv(p7, 0.6f, EaDefault, K(0f, 0.35f), K(5.3f, 1f), K(29.4f, 0.35f), K(100f, 0.35f))
    val s9 = kv(p3, 1f, EaDefault, K(0f, 1f, EaOut), K(4.1f, 1.4f, EaIo), K(11.8f, 1f), K(100f, 1f))
    val s11 = kv(p5, 1f, EaDefault, K(0f, 1f, EaOut), K(4.1f, 1.4f, EaIo), K(11.8f, 1f), K(100f, 1f))
    val s13 = kv(p7, 1f, EaDefault, K(0f, 1f, EaOut), K(4.1f, 1.4f, EaIo), K(11.8f, 1f), K(100f, 1f))
    val p15 = ph(t, 3.4f)
    val o15 = kv(p15, 0f, EaDefault, K(0f, 0f, LinearEasing), K(8.8f, 0f, LinearEasing), K(11.8f, 1f, LinearEasing), K(67.6f, 1f), K(76.5f, 0f), K(100f, 0f))
    val ty15 = kv(p15, 0f, EaDefault, K(0f, 0f, LinearEasing), K(8.8f, 0f, LinearEasing), K(67.6f, 58f), K(76.5f, 58f), K(100f, 58f))
    val p16 = ph(t, 0.5f)
    val ty16 = kv(p16, 0f, EaDefault, K(0f, 0f), K(50f, -2.5f), K(100f, 0f))
    shape(k, "M30 16 V78")
    // el-r el-r1
    layer(o3) {
        shape(k, "M43 21 H71 A3 3 0 0 1 74 24 V28 A3 3 0 0 1 71 31 H43 A3 3 0 0 1 40 28 V24 A3 3 0 0 1 43 21 Z", dash = "3 3")
    }
    // el-r el-r2
    layer(o5) {
        shape(k, "M43 41 H71 A3 3 0 0 1 74 44 V48 A3 3 0 0 1 71 51 H43 A3 3 0 0 1 40 48 V44 A3 3 0 0 1 43 41 Z", dash = "3 3")
    }
    // el-r el-r3
    layer(o7) {
        shape(k, "M43 61 H71 A3 3 0 0 1 74 64 V68 A3 3 0 0 1 71 71 H43 A3 3 0 0 1 40 68 V64 A3 3 0 0 1 43 61 Z", dash = "3 3")
    }
    // el-n el-n1
    withTransform({ translate(30f, 26f); scale(s9, s9, Offset.Zero); translate(-30f, -26f) }) {
        shape(k, "M33.2 26 A3.2 3.2 0 0 1 30 29.2 A3.2 3.2 0 0 1 26.8 26 A3.2 3.2 0 0 1 30 22.8 A3.2 3.2 0 0 1 33.2 26 Z", paper = true)
    }
    // el-n el-n2
    withTransform({ translate(30f, 46f); scale(s11, s11, Offset.Zero); translate(-30f, -46f) }) {
        shape(k, "M33.2 46 A3.2 3.2 0 0 1 30 49.2 A3.2 3.2 0 0 1 26.8 46 A3.2 3.2 0 0 1 30 42.8 A3.2 3.2 0 0 1 33.2 46 Z", paper = true)
    }
    // el-n el-n3
    withTransform({ translate(30f, 66f); scale(s13, s13, Offset.Zero); translate(-30f, -66f) }) {
        shape(k, "M33.2 66 A3.2 3.2 0 0 1 30 69.2 A3.2 3.2 0 0 1 26.8 66 A3.2 3.2 0 0 1 30 62.8 A3.2 3.2 0 0 1 33.2 66 Z", paper = true)
    }
    // el-dot
    layer(o15) {
        translate(0f, ty15) {
            // ea-dot
            translate(0f, ty16) {
                shape(k, "M32 18 A2 2 0 0 1 30 20 A2 2 0 0 1 28 18 A2 2 0 0 1 30 16 A2 2 0 0 1 32 18 Z", fillInk = 1f, stroke = false)
            }
        }
    }
    shape(k, "M26 78 h8")
}

// key: API keys, SSH keys, tokens, secrets, permissions
// slides in 0.4-1.15, turns (squash) 1.15-1.45, shackle opens 1.45-1.75, holds, closes 2.5-2.8, key out 2.8-3.4. Still: drawn as listed (no state differs from the loop start)
private fun DrawScope.key(k: Ink, t: Float?) {
    val p3 = ph(t, 3.4f)
    val tx3 = kv(p3, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(33.8f, 13f), K(38.2f, 13f), K(42.6f, 13f, EaIo), K(82.4f, 13f, EaIo), K(100f, 0f))
    val sy3 = kv(p3, 1f, EaDefault, K(0f, 1f, EaOut), K(11.8f, 1f, EaOut), K(33.8f, 1f), K(38.2f, 0.35f), K(42.6f, 1f, EaIo), K(82.4f, 1f, EaIo), K(100f, 1f))
    val ty7 = kv(p3, 0f, EaDefault, K(0f, 0f, EaOut), K(42.6f, 0f, EaOut), K(51.5f, -5f, EaIo), K(73.5f, -5f, EaIo), K(82.4f, 0f), K(100f, 0f))
    shape(k, "M80 72 A30 5 0 0 1 50 77 A30 5 0 0 1 20 72 A30 5 0 0 1 50 67 A30 5 0 0 1 80 72 Z", paper = true)
    // ek-key
    withTransform({ translate(32.5f, 56f); translate(tx3, 0f); scale(1f, sy3, Offset.Zero); translate(-32.5f, -56f) }) {
        shape(k, "M27 56 A7 7 0 0 1 20 63 A7 7 0 0 1 13 56 A7 7 0 0 1 20 49 A7 7 0 0 1 27 56 Z", paper = true)
        shape(k, "M22.5 56 A2.5 2.5 0 0 1 20 58.5 A2.5 2.5 0 0 1 17.5 56 A2.5 2.5 0 0 1 20 53.5 A2.5 2.5 0 0 1 22.5 56 Z")
        shape(k, "M27 56 H52 M44 56 v4 M48 56 v3")
    }
    // ek-shackle
    translate(0f, ty7) {
        shape(k, "M57 46 V38 a8 8 0 0 1 16 0 V46")
    }
    shape(k, "M55 44 H75 A3 3 0 0 1 78 47 V65 A3 3 0 0 1 75 68 H55 A3 3 0 0 1 52 65 V47 A3 3 0 0 1 55 44 Z", paper = true)
    shape(k, "M67.4 54 A2.4 2.4 0 0 1 65 56.4 A2.4 2.4 0 0 1 62.6 54 A2.4 2.4 0 0 1 65 51.6 A2.4 2.4 0 0 1 67.4 54 Z")
    shape(k, "M65 56.4 V61")
}

// apps: apps, skills, connectors, plugins, MCP, extensions, mini apps
// a block slides into the empty dashed slot 0.5-1.3, impact lines, holds, lifts back out 2.6-3.4. Still: .eb-impact opacity:0; .eb-fly transform:translate(14,-12)
private fun DrawScope.apps(k: Ink, t: Float?) {
    val p14 = ph(t, 3.4f)
    val o14 = kv(p14, 0f, EaDefault, K(0f, 0f), K(38.2f, 0f), K(44.1f, 1f, EaOut), K(51.5f, 0f), K(100f, 0f))
    val s14 = kv(p14, 1f, EaDefault, K(0f, 1f), K(38.2f, 1f), K(44.1f, 1.12f, EaOut), K(51.5f, 1.12f), K(100f, 1.12f))
    val tx16 = kv(p14, 14f, EaDefault, K(0f, 14f, EaOut), K(14.7f, 14f, EaOut), K(38.2f, 0f), K(41.2f, 0f), K(45.6f, 0f, EaIo), K(76.5f, 0f, EaIo), K(100f, 14f))
    val ty16 = kv(p14, -12f, EaDefault, K(0f, -12f, EaOut), K(14.7f, -12f, EaOut), K(38.2f, 0f), K(41.2f, 1f), K(45.6f, 0f, EaIo), K(76.5f, 0f, EaIo), K(100f, -12f))
    val o20 = kv(p14, 1f, EaIo, K(0f, 0.4f), K(38.2f, 0.4f), K(45.6f, 1f), K(58.8f, 0.4f), K(100f, 0.4f))
    val s20 = kv(p14, 1f, EaIo, K(0f, 0.8f), K(38.2f, 0.8f), K(45.6f, 1.15f), K(58.8f, 0.8f), K(100f, 0.8f))
    shape(k, "M36 49 L48 56 L36 63 L24 56 Z", paper = true)
    shape(k, "M24 56 V68 L36 75 V63", paper = true)
    shape(k, "M48 56 V68 L36 75", paper = true)
    shape(k, "M60 49 L72 56 L60 63 L48 56 Z", paper = true)
    shape(k, "M48 56 V68 L60 75 V63", paper = true)
    shape(k, "M72 56 V68 L60 75", paper = true)
    shape(k, "M48 30 L60 37 L48 44 L36 37 Z", dash = "3 3")
    shape(k, "M36 37 V49 L48 56 V44", dash = "3 3")
    shape(k, "M60 37 V49 L48 56", dash = "3 3")
    // eb-impact
    layer(o14) {
        withTransform({ translate(48f, 44f); scale(s14, s14, Offset.Zero); translate(-48f, -44f) }) {
            shape(k, "M30 42 l-5 -2 M30 48 l-6 0 M66 42 l5 -2 M66 48 l6 0")
        }
    }
    // eb-fly
    translate(tx16, ty16) {
        shape(k, "M48 30 L60 37 L48 44 L36 37 Z", paper = true)
        shape(k, "M36 37 V49 L48 56 V44", paper = true)
        shape(k, "M60 37 V49 L48 56", paper = true)
    }
    // eb-plus
    layer(o20) {
        withTransform({ translate(80f, 21.5f); scale(s20, s20, Offset.Zero); translate(-80f, -21.5f) }) {
            shape(k, "M80 18 v7 M76.5 21.5 h7")
        }
    }
}

// memory: memory, remembered notes, brains
// a page turns 0.4-1.1, thought dots and a cloud rise 1.0-1.5, hold, fade 2.7-3.1, page turns back. Still: drawn as listed (no state differs from the loop start)
private fun DrawScope.memory(k: Ink, t: Float?) {
    val p7 = ph(t, 3.4f)
    val sx7 = kv(p7, 1f, EaDefault, K(0f, 1f, EaIo), K(11.8f, 1f, EaIo), K(32.4f, -1f, EaIo), K(79.4f, -1f, EaIo), K(97.1f, 1f), K(100f, 1f))
    val o9 = kv(p7, 1f, EaDefault, K(0f, 0f, EaOut), K(29.4f, 0f, EaOut), K(36.8f, 1f), K(79.4f, 1f), K(88.2f, 0f), K(100f, 0f))
    val ty9 = kv(p7, 0f, EaDefault, K(0f, 3f, EaOut), K(29.4f, 3f, EaOut), K(36.8f, 0f), K(79.4f, 0f), K(100f, 0f))
    val p11 = ph(t, 3.4f, 0.15f)
    val o11 = kv(p11, 1f, EaDefault, K(0f, 0f, EaOut), K(29.4f, 0f, EaOut), K(36.8f, 1f), K(79.4f, 1f), K(88.2f, 0f), K(100f, 0f))
    val ty11 = kv(p11, 0f, EaDefault, K(0f, 3f, EaOut), K(29.4f, 3f, EaOut), K(36.8f, 0f), K(79.4f, 0f), K(100f, 0f))
    val o13 = kv(p7, 1f, EaDefault, K(0f, 0f, EaOut), K(38.2f, 0f, EaOut), K(47.1f, 1f, EaIn), K(79.4f, 1f, EaIn), K(89.7f, 0f), K(100f, 0f))
    val s13 = kv(p7, 1f, EaDefault, K(0f, 0.6f, EaOut), K(38.2f, 0.6f, EaOut), K(47.1f, 1f, EaIn), K(79.4f, 1f, EaIn), K(89.7f, 0.9f), K(100f, 0.9f))
    shape(k, "M78 72 A30 5 0 0 1 48 77 A30 5 0 0 1 18 72 A30 5 0 0 1 48 67 A30 5 0 0 1 78 72 Z", paper = true)
    shape(k, "M48 70 L22 60 V36 L48 46 Z", paper = true)
    shape(k, "M48 70 L74 60 V36 L48 46 Z", paper = true)
    shape(k, "M27 44 l16 6 M27 50 l16 6 M27 56 l10 4", alpha = 0.6f)
    shape(k, "M53 50 l16 -6 M53 56 l16 -6", alpha = 0.6f)
    // em-page
    withTransform({ translate(48f, 53f); scale(sx7, 1f, Offset.Zero); translate(-48f, -53f) }) {
        shape(k, "M48 70 L74 60 V36 L48 46 Z", paper = true)
    }
    // em-t em-t1
    layer(o9) {
        translate(0f, ty9) {
            shape(k, "M59.8 34 A1.8 1.8 0 0 1 58 35.8 A1.8 1.8 0 0 1 56.2 34 A1.8 1.8 0 0 1 58 32.2 A1.8 1.8 0 0 1 59.8 34 Z", paper = true)
        }
    }
    // em-t em-t2
    layer(o11) {
        translate(0f, ty11) {
            shape(k, "M65.8 27 A2.8 2.8 0 0 1 63 29.8 A2.8 2.8 0 0 1 60.2 27 A2.8 2.8 0 0 1 63 24.2 A2.8 2.8 0 0 1 65.8 27 Z", paper = true)
        }
    }
    // em-cloud
    layer(o13) {
        withTransform({ translate(72.4077f, 16.1108f); scale(s13, s13, Offset.Zero); translate(-72.4077f, -16.1108f) }) {
            shape(k, "M64 18 a4 4 0 0 1 4 -6 a5 5 0 0 1 9 0 a4 4 0 0 1 3 7 a3.5 3.5 0 0 1 -4 4 h-9 a3.5 3.5 0 0 1 -3 -5 Z", paper = true)
            shape(k, "M71.5 12.5 v4 M69.5 14.5 h4")
        }
    }
}

// model: models, providers, on-device models (Gizzi scene)
// an arrow drops into the chip 0.3-1.3, the core pops, the pins light. Still: drawn as listed (no state differs from the loop start)
private fun DrawScope.model(k: Ink, t: Float?) {
    val p2 = ph(t, 3.4f)
    val o2 = kv(p2, 1f, EaDefault, K(0f, 0.45f), K(41.2f, 0.45f), K(47.1f, 1f, EaIo), K(76.5f, 0.45f), K(100f, 0.45f))
    val s5 = kv(p2, 1f, EaDefault, K(0f, 1f, EaOut), K(38.2f, 1f, EaOut), K(45.6f, 1.12f, EaIo), K(55.9f, 1f), K(100f, 1f))
    val o8 = kv(p2, 1f, EaDefault, K(0f, 0f, EaIn), K(8.8f, 0f, EaIn), K(17.6f, 1f), K(38.2f, 1f), K(44.1f, 0f), K(100f, 0f))
    val ty8 = kv(p2, 0f, EaDefault, K(0f, -6f, EaIn), K(8.8f, -6f, EaIn), K(38.2f, 8f), K(44.1f, 10f), K(100f, 10f))
    // eM-pins
    layer(o2) {
        shape(k, "M38.00 34 v-5 M38.00 66 v5 M32 42.00 h-5 M64 42.00 h5 M44.67 34 v-5 M44.67 66 v5 M32 48.67 h-5 M64 48.67 h5 M51.33 34 v-5 M51.33 66 v5 M32 55.33 h-5 M64 55.33 h5 M58.00 34 v-5 M58.00 66 v5 M32 62.00 h-5 M64 62.00 h5")
    }
    shape(k, "M36 34 H60 A4 4 0 0 1 64 38 V62 A4 4 0 0 1 60 66 H36 A4 4 0 0 1 32 62 V38 A4 4 0 0 1 36 34 Z", paper = true)
    // eM-core
    withTransform({ translate(48f, 50f); scale(s5, s5, Offset.Zero); translate(-48f, -50f) }) {
        shape(k, "M42 42 H54 A2 2 0 0 1 56 44 V56 A2 2 0 0 1 54 58 H42 A2 2 0 0 1 40 56 V44 A2 2 0 0 1 42 42 Z")
        shape(k, "M48 46 l3 4 l-3 4 l-3 -4 Z")
    }
    // eM-arrow
    layer(o8) {
        translate(0f, ty8) {
            shape(k, "M48 8 V20 M43.5 15.5 L48 20 L52.5 15.5")
        }
    }
}

// chart: charts, benchmarks, metrics, values, rows to plot
// bars grow into the dashed outlines 0.4-1.1, a trend line shows, bars drop back 2.6-3.1. Still: .ech-b1 transform:scaleY(0); .ech-b2 transform:scaleY(0); .ech-b3 transform:scaleY(0); .ech-line opacity:0
private fun DrawScope.chart(k: Ink, t: Float?) {
    val p6 = ph(t, 3.4f)
    val sy6 = kv(p6, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(32.4f, 1f, EaIn), K(76.5f, 1f, EaIn), K(91.2f, 0f), K(100f, 0f))
    val p8 = ph(t, 3.4f, 0.12f)
    val sy8 = kv(p8, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(32.4f, 1f, EaIn), K(76.5f, 1f, EaIn), K(91.2f, 0f), K(100f, 0f))
    val p10 = ph(t, 3.4f, 0.24f)
    val sy10 = kv(p10, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(32.4f, 1f, EaIn), K(76.5f, 1f, EaIn), K(91.2f, 0f), K(100f, 0f))
    val o12 = kv(p6, 0f, EaDefault, K(0f, 0f), K(32.4f, 0f), K(41.2f, 1f), K(73.5f, 1f), K(82.4f, 0f), K(100f, 0f))
    shape(k, "M24 18 V70 H78")
    shape(k, "M33.5 40 H39.5 A1.5 1.5 0 0 1 41 41.5 V68.5 A1.5 1.5 0 0 1 39.5 70 H33.5 A1.5 1.5 0 0 1 32 68.5 V41.5 A1.5 1.5 0 0 1 33.5 40 Z", dash = "3 3")
    shape(k, "M47.5 52 H53.5 A1.5 1.5 0 0 1 55 53.5 V68.5 A1.5 1.5 0 0 1 53.5 70 H47.5 A1.5 1.5 0 0 1 46 68.5 V53.5 A1.5 1.5 0 0 1 47.5 52 Z", dash = "3 3")
    shape(k, "M61.5 32 H67.5 A1.5 1.5 0 0 1 69 33.5 V68.5 A1.5 1.5 0 0 1 67.5 70 H61.5 A1.5 1.5 0 0 1 60 68.5 V33.5 A1.5 1.5 0 0 1 61.5 32 Z", dash = "3 3")
    // ech-b ech-b1
    withTransform({ translate(36.5f, 70f); scale(1f, sy6, Offset.Zero); translate(-36.5f, -70f) }) {
        shape(k, "M33.5 40 H39.5 A1.5 1.5 0 0 1 41 41.5 V68.5 A1.5 1.5 0 0 1 39.5 70 H33.5 A1.5 1.5 0 0 1 32 68.5 V41.5 A1.5 1.5 0 0 1 33.5 40 Z", fillInk = 0.12f)
    }
    // ech-b ech-b2
    withTransform({ translate(50.5f, 70f); scale(1f, sy8, Offset.Zero); translate(-50.5f, -70f) }) {
        shape(k, "M47.5 52 H53.5 A1.5 1.5 0 0 1 55 53.5 V68.5 A1.5 1.5 0 0 1 53.5 70 H47.5 A1.5 1.5 0 0 1 46 68.5 V53.5 A1.5 1.5 0 0 1 47.5 52 Z", fillInk = 0.12f)
    }
    // ech-b ech-b3
    withTransform({ translate(64.5f, 70f); scale(1f, sy10, Offset.Zero); translate(-64.5f, -70f) }) {
        shape(k, "M61.5 32 H67.5 A1.5 1.5 0 0 1 69 33.5 V68.5 A1.5 1.5 0 0 1 67.5 70 H61.5 A1.5 1.5 0 0 1 60 68.5 V33.5 A1.5 1.5 0 0 1 61.5 32 Z", fillInk = 0.12f)
    }
    // ech-line
    layer(o12) {
        shape(k, "M36.5 36 L50.5 24 L64.5 44", dash = "2 3")
    }
}

// people: contacts, calls, teams, members, test users
// the empty (dashed) seat breathes, the front person nods, a + badge pops 1.0-2.6. Still: .epp-ghost opacity:0.6
private fun DrawScope.people(k: Ink, t: Float?) {
    val p3 = ph(t, 3.4f)
    val o3 = kv(p3, 0.6f, EaIo, K(0f, 0.35f), K(50f, 0.9f), K(100f, 0.35f))
    val ty6 = kv(p3, 0f, EaIo, K(0f, 0f), K(26.5f, 0f), K(32.4f, -2f), K(39.7f, 0f), K(100f, 0f))
    val o9 = kv(p3, 1f, EaDefault, K(0f, 0f, EaOut), K(29.4f, 0f, EaOut), K(38.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(88.2f, 0f), K(100f, 0f))
    val s9 = kv(p3, 1f, EaDefault, K(0f, 0.6f, EaOut), K(29.4f, 0.6f, EaOut), K(38.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(88.2f, 0.6f), K(100f, 0.6f))
    shape(k, "M78 70 A28 5 0 0 1 50 75 A28 5 0 0 1 22 70 A28 5 0 0 1 50 65 A28 5 0 0 1 78 70 Z", paper = true)
    // epp-ghost
    layer(o3) {
        shape(k, "M68 45 A7 7 0 0 1 61 52 A7 7 0 0 1 54 45 A7 7 0 0 1 61 38 A7 7 0 0 1 68 45 Z", dash = "3 3")
        shape(k, "M49 66 a12 12 0 0 1 24 0", dash = "3 3")
    }
    // epp-front
    translate(0f, ty6) {
        shape(k, "M26 68 a14 14 0 0 1 28 0 Z", paper = true)
        shape(k, "M48.5 43 A8.5 8.5 0 0 1 40 51.5 A8.5 8.5 0 0 1 31.5 43 A8.5 8.5 0 0 1 40 34.5 A8.5 8.5 0 0 1 48.5 43 Z", paper = true)
    }
    // epp-plus
    layer(o9) {
        withTransform({ translate(76f, 28f); scale(s9, s9, Offset.Zero); translate(-76f, -28f) }) {
            shape(k, "M83 28 A7 7 0 0 1 76 35 A7 7 0 0 1 69 28 A7 7 0 0 1 76 21 A7 7 0 0 1 83 28 Z", paper = true)
            shape(k, "M76 24.5 v7 M72.5 28 h7")
        }
    }
}

// archive: trash, archived, clean up, tidy
// lid lifts and tilts 0.4-1.0, an empty puff rises 0.9-1.8, lid drops 2.2-2.6 with impact lines. Still: .ear-puff opacity:0; .ear-impact opacity:0; .ear-lid transform:translate(0,0)
private fun DrawScope.archive(k: Ink, t: Float?) {
    val p6 = ph(t, 3.4f)
    val o6 = kv(p6, 0f, EaDefault, K(0f, 0f, EaOut), K(26.5f, 0f, EaOut), K(41.2f, 1f), K(55.9f, 0f), K(100f, 0f))
    val ty6 = kv(p6, 0f, EaDefault, K(0f, 4f, EaOut), K(26.5f, 4f, EaOut), K(41.2f, -4f), K(55.9f, -10f), K(100f, -10f))
    val s6 = kv(p6, 1f, EaDefault, K(0f, 0.7f, EaOut), K(26.5f, 0.7f, EaOut), K(41.2f, 1f), K(55.9f, 1.1f), K(100f, 1.1f))
    val o10 = kv(p6, 0f, EaDefault, K(0f, 0f), K(75f, 0f), K(79.4f, 1f, EaOut), K(88.2f, 0f), K(100f, 0f))
    val s10 = kv(p6, 1f, EaDefault, K(0f, 1f), K(75f, 1f), K(79.4f, 1.12f, EaOut), K(88.2f, 1.12f), K(100f, 1.12f))
    val tx12 = kv(p6, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(29.4f, -14f, EaIn), K(64.7f, -14f, EaIn), K(75f, 0f), K(80.9f, 0f), K(100f, 0f))
    val ty12 = kv(p6, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(29.4f, -14f, EaIn), K(64.7f, -14f, EaIn), K(75f, 1f), K(80.9f, 0f), K(100f, 0f))
    val r12 = kv(p6, 0f, EaDefault, K(0f, 0f, EaOut), K(11.8f, 0f, EaOut), K(29.4f, -10f, EaIn), K(64.7f, -10f, EaIn), K(75f, 0f), K(80.9f, 0f), K(100f, 0f))
    shape(k, "M76 78 A28 5 0 0 1 48 83 A28 5 0 0 1 20 78 A28 5 0 0 1 48 73 A28 5 0 0 1 76 78 Z", paper = true)
    shape(k, "M24 52 L48 42 L72 52 L48 62 Z", paper = true)
    shape(k, "M24 52 V68 L48 78 V62 M72 52 V68 L48 78", paper = true)
    shape(k, "M40 66 l8 3.4 l8 -3.4", alpha = 0.6f)
    // ear-puff
    layer(o6) {
        withTransform({ translate(52.3f, 40.1f); translate(0f, ty6); scale(s6, s6, Offset.Zero); translate(-52.3f, -40.1f) }) {
            shape(k, "M48 44 A2 2 0 0 1 46 46 A2 2 0 0 1 44 44 A2 2 0 0 1 46 42 A2 2 0 0 1 48 44 Z", paper = true)
            shape(k, "M55.8 37 A2.8 2.8 0 0 1 53 39.8 A2.8 2.8 0 0 1 50.2 37 A2.8 2.8 0 0 1 53 34.2 A2.8 2.8 0 0 1 55.8 37 Z", paper = true)
            shape(k, "M60.6 44 A1.6 1.6 0 0 1 59 45.6 A1.6 1.6 0 0 1 57.4 44 A1.6 1.6 0 0 1 59 42.4 A1.6 1.6 0 0 1 60.6 44 Z", paper = true)
        }
    }
    // ear-impact
    layer(o10) {
        withTransform({ translate(48f, 52f); scale(s10, s10, Offset.Zero); translate(-48f, -52f) }) {
            shape(k, "M18 50 l-5 -2 M18 56 l-6 0 M78 50 l5 -2 M78 56 l6 0")
        }
    }
    // ear-lid
    withTransform({ translate(48f, 49.5f); translate(tx12, ty12); rotate(r12, Offset.Zero); translate(-48f, -49.5f) }) {
        shape(k, "M48 37 L72 47 L48 57 L24 47 Z", paper = true)
        shape(k, "M24 47 V52 L48 62 V57")
        shape(k, "M72 47 V52 L48 62")
    }
}

// flow: DAGs, flows, plans, maps, recipes, iterations
// first links draw 0.3-0.8, middle nodes pop, second links draw 1.0-1.5, the empty end node pulses; all fade back 2.6-3.1. Still: .efl-l1 opacity:0.55; .efl-l2 opacity:0.55; .efl-end opacity:0.8
private fun DrawScope.flow(k: Ink, t: Float?) {
    val p2 = ph(t, 3.4f)
    val o2 = kv(p2, 0.55f, EaDefault, K(0f, 1f, EaOut), K(8.8f, 1f, EaOut), K(23.5f, 1f), K(76.5f, 1f), K(91.2f, 0f), K(100f, 0f))
    val do2 = kv(p2, 0f, EaDefault, K(0f, 1f, EaOut), K(8.8f, 1f, EaOut), K(23.5f, 0f), K(76.5f, 0f), K(91.2f, 0f), K(100f, 0f))
    val o3 = kv(p2, 0.55f, EaDefault, K(0f, 1f, EaOut), K(8.8f, 1f, EaOut), K(23.5f, 1f), K(76.5f, 1f), K(91.2f, 0f), K(100f, 0f))
    val do3 = kv(p2, 0f, EaDefault, K(0f, 1f, EaOut), K(8.8f, 1f, EaOut), K(23.5f, 0f), K(76.5f, 0f), K(91.2f, 0f), K(100f, 0f))
    val o4 = kv(p2, 0.55f, EaDefault, K(0f, 1f, EaOut), K(29.4f, 1f, EaOut), K(44.1f, 1f), K(76.5f, 1f), K(91.2f, 0f), K(100f, 0f))
    val do4 = kv(p2, 0f, EaDefault, K(0f, 1f, EaOut), K(29.4f, 1f, EaOut), K(44.1f, 0f), K(76.5f, 0f), K(91.2f, 0f), K(100f, 0f))
    val o5 = kv(p2, 0.55f, EaDefault, K(0f, 1f, EaOut), K(29.4f, 1f, EaOut), K(44.1f, 1f), K(76.5f, 1f), K(91.2f, 0f), K(100f, 0f))
    val do5 = kv(p2, 0f, EaDefault, K(0f, 1f, EaOut), K(29.4f, 1f, EaOut), K(44.1f, 0f), K(76.5f, 0f), K(91.2f, 0f), K(100f, 0f))
    val s8 = kv(p2, 1f, EaDefault, K(0f, 1f, EaOut), K(23.5f, 1f, EaOut), K(27.9f, 1.15f), K(33.8f, 1f), K(100f, 1f))
    val s10 = kv(p2, 1f, EaDefault, K(0f, 1f, EaOut), K(23.5f, 1f, EaOut), K(27.9f, 1.15f), K(33.8f, 1f), K(100f, 1f))
    val o12 = kv(p2, 0.8f, EaDefault, K(0f, 0.5f, EaOut), K(44.1f, 0.5f, EaOut), K(50f, 1f), K(58.8f, 1f), K(76.5f, 1f), K(100f, 0.5f))
    val s12 = kv(p2, 1f, EaDefault, K(0f, 1f, EaOut), K(44.1f, 1f, EaOut), K(50f, 1.12f), K(58.8f, 1f), K(76.5f, 1f), K(100f, 1f))
    measured(k, "M32 46 C 36 46 36 30 40 30", 1f, if (p2 == null) floatArrayOf(1f, 0f) else floatArrayOf(1f, 1f), do2, alpha = o2)
    measured(k, "M32 46 C 36 46 36 62 40 62", 1f, if (p2 == null) floatArrayOf(1f, 0f) else floatArrayOf(1f, 1f), do3, alpha = o3)
    measured(k, "M58 30 C 62 30 62 46 66 46", 1f, if (p2 == null) floatArrayOf(1f, 0f) else floatArrayOf(1f, 1f), do4, alpha = o4)
    measured(k, "M58 62 C 62 62 62 46 66 46", 1f, if (p2 == null) floatArrayOf(1f, 0f) else floatArrayOf(1f, 1f), do5, alpha = o5)
    shape(k, "M17 40 H29 A3 3 0 0 1 32 43 V49 A3 3 0 0 1 29 52 H17 A3 3 0 0 1 14 49 V43 A3 3 0 0 1 17 40 Z", paper = true)
    // efl-n2
    withTransform({ translate(49f, 30f); scale(s8, s8, Offset.Zero); translate(-49f, -30f) }) {
        shape(k, "M43 24 H55 A3 3 0 0 1 58 27 V33 A3 3 0 0 1 55 36 H43 A3 3 0 0 1 40 33 V27 A3 3 0 0 1 43 24 Z", paper = true)
    }
    // efl-n2
    withTransform({ translate(49f, 62f); scale(s10, s10, Offset.Zero); translate(-49f, -62f) }) {
        shape(k, "M43 56 H55 A3 3 0 0 1 58 59 V65 A3 3 0 0 1 55 68 H43 A3 3 0 0 1 40 65 V59 A3 3 0 0 1 43 56 Z", paper = true)
    }
    // efl-end
    layer(o12) {
        withTransform({ translate(75f, 46f); scale(s12, s12, Offset.Zero); translate(-75f, -46f) }) {
            shape(k, "M69 40 H81 A3 3 0 0 1 84 43 V49 A3 3 0 0 1 81 52 H69 A3 3 0 0 1 66 49 V43 A3 3 0 0 1 69 40 Z", dash = "3 3")
            shape(k, "M75 43 v6 M72 46 h6")
        }
    }
}

// network: nodes, channels, endpoints, instances, connections
// a ring goes out from the hub 0.3-1.3; each dashed link and empty node lights in turn, then dims (nobody joined). Still: .enw-l1 opacity:0.5; .enw-l2 opacity:0.5; .enw-l3 opacity:0.5; .enw-l4 opacity:0.5; .enw-s1 opacity:0.7; .enw-s2 opacity:0.7; .enw-s3 opacity:0.7; .enw-s4 opacity:0.7; .enw-ring opacity:0
private fun DrawScope.network(k: Ink, t: Float?) {
    val p2 = ph(t, 3.4f)
    val o2 = kv(p2, 0.5f, EaDefault, K(0f, 0.3f), K(17.6f, 0.3f), K(25f, 1f, EaIo), K(44.1f, 0.3f), K(100f, 0.3f))
    val p3 = ph(t, 3.4f, 0.15f)
    val o3 = kv(p3, 0.5f, EaDefault, K(0f, 0.3f), K(17.6f, 0.3f), K(25f, 1f, EaIo), K(44.1f, 0.3f), K(100f, 0.3f))
    val p4 = ph(t, 3.4f, 0.3f)
    val o4 = kv(p4, 0.5f, EaDefault, K(0f, 0.3f), K(17.6f, 0.3f), K(25f, 1f, EaIo), K(44.1f, 0.3f), K(100f, 0.3f))
    val p5 = ph(t, 3.4f, 0.45f)
    val o5 = kv(p5, 0.5f, EaDefault, K(0f, 0.3f), K(17.6f, 0.3f), K(25f, 1f, EaIo), K(44.1f, 0.3f), K(100f, 0.3f))
    val o6 = kv(p2, 0.7f, EaDefault, K(0f, 0.45f), K(23.5f, 0.45f), K(29.4f, 1f, EaIo), K(47.1f, 0.45f), K(100f, 0.45f))
    val o8 = kv(p3, 0.7f, EaDefault, K(0f, 0.45f), K(23.5f, 0.45f), K(29.4f, 1f, EaIo), K(47.1f, 0.45f), K(100f, 0.45f))
    val o10 = kv(p4, 0.7f, EaDefault, K(0f, 0.45f), K(23.5f, 0.45f), K(29.4f, 1f, EaIo), K(47.1f, 0.45f), K(100f, 0.45f))
    val o12 = kv(p5, 0.7f, EaDefault, K(0f, 0.45f), K(23.5f, 0.45f), K(29.4f, 1f, EaIo), K(47.1f, 0.45f), K(100f, 0.45f))
    val o14 = kv(p2, 0f, EaDefault, K(0f, 0.6f, EaOut), K(8.8f, 0.6f, EaOut), K(38.2f, 0f), K(100f, 0f))
    val s14 = kv(p2, 1f, EaDefault, K(0f, 1f, EaOut), K(8.8f, 1f, EaOut), K(38.2f, 2.4f), K(100f, 2.4f))
    val p16 = ph(t, 0.5f)
    val ty16 = kv(p16, 0f, EaDefault, K(0f, 0f), K(50f, -2.5f), K(100f, 0f))
    shape(k, "M48 48 L18 24", dash = "2 3", alpha = o2)
    shape(k, "M48 48 L78 24", dash = "2 3", alpha = o3)
    shape(k, "M48 48 L18 72", dash = "2 3", alpha = o4)
    shape(k, "M48 48 L78 72", dash = "2 3", alpha = o5)
    // enw-s enw-s1
    layer(o6) {
        shape(k, "M23.5 24 A5.5 5.5 0 0 1 18 29.5 A5.5 5.5 0 0 1 12.5 24 A5.5 5.5 0 0 1 18 18.5 A5.5 5.5 0 0 1 23.5 24 Z", paper = true, dash = "3 2.4")
    }
    // enw-s enw-s2
    layer(o8) {
        shape(k, "M83.5 24 A5.5 5.5 0 0 1 78 29.5 A5.5 5.5 0 0 1 72.5 24 A5.5 5.5 0 0 1 78 18.5 A5.5 5.5 0 0 1 83.5 24 Z", paper = true, dash = "3 2.4")
    }
    // enw-s enw-s3
    layer(o10) {
        shape(k, "M23.5 72 A5.5 5.5 0 0 1 18 77.5 A5.5 5.5 0 0 1 12.5 72 A5.5 5.5 0 0 1 18 66.5 A5.5 5.5 0 0 1 23.5 72 Z", paper = true, dash = "3 2.4")
    }
    // enw-s enw-s4
    layer(o12) {
        shape(k, "M83.5 72 A5.5 5.5 0 0 1 78 77.5 A5.5 5.5 0 0 1 72.5 72 A5.5 5.5 0 0 1 78 66.5 A5.5 5.5 0 0 1 83.5 72 Z", paper = true, dash = "3 2.4")
    }
    // enw-ring
    withTransform({ translate(48f, 48f); scale(s14, s14, Offset.Zero); translate(-48f, -48f) }) {
        shape(k, "M58 48 A10 10 0 0 1 48 58 A10 10 0 0 1 38 48 A10 10 0 0 1 48 38 A10 10 0 0 1 58 48 Z", alpha = o14)
    }
    shape(k, "M58 48 A10 10 0 0 1 48 58 A10 10 0 0 1 38 48 A10 10 0 0 1 48 38 A10 10 0 0 1 58 48 Z", paper = true)
    // ea-dot
    translate(0f, ty16) {
        shape(k, "M51 48 A3 3 0 0 1 48 51 A3 3 0 0 1 45 48 A3 3 0 0 1 48 45 A3 3 0 0 1 51 48 Z", fillInk = 1f, stroke = false)
    }
}

// calendar: deadlines, next two weeks, schedules, runs today
// the top band flips up 0.4-0.9 and back, a day ring pops 1.1-1.4, holds, fades 2.6-3.0. Still: .ecal-mark opacity:1
private fun DrawScope.calendar(k: Ink, t: Float?) {
    val p17 = ph(t, 3.4f)
    val o17 = kv(p17, 1f, EaDefault, K(0f, 0f, EaOut), K(32.4f, 0f, EaOut), K(41.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(88.2f, 0f), K(100f, 0f))
    val s17 = kv(p17, 1f, EaDefault, K(0f, 0.5f, EaOut), K(32.4f, 0.5f, EaOut), K(41.2f, 1f, EaIn), K(76.5f, 1f, EaIn), K(88.2f, 0.8f), K(100f, 0.8f))
    val sy19 = kv(p17, 1f, EaDefault, K(0f, 1f, EaIo), K(11.8f, 1f, EaIo), K(19.1f, -0.6f), K(26.5f, 1f), K(100f, 1f))
    shape(k, "M76 78 A28 4.5 0 0 1 48 82.5 A28 4.5 0 0 1 20 78 A28 4.5 0 0 1 48 73.5 A28 4.5 0 0 1 76 78 Z", paper = true)
    shape(k, "M28 24 H68 A4 4 0 0 1 72 28 V70 A4 4 0 0 1 68 74 H28 A4 4 0 0 1 24 70 V28 A4 4 0 0 1 28 24 Z", paper = true)
    shape(k, "M24 35 H72")
    shape(k, "M31.2 42 H34.8 A1.2 1.2 0 0 1 36 43.2 V46.8 A1.2 1.2 0 0 1 34.8 48 H31.2 A1.2 1.2 0 0 1 30 46.8 V43.2 A1.2 1.2 0 0 1 31.2 42 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M41.2 42 H44.8 A1.2 1.2 0 0 1 46 43.2 V46.8 A1.2 1.2 0 0 1 44.8 48 H41.2 A1.2 1.2 0 0 1 40 46.8 V43.2 A1.2 1.2 0 0 1 41.2 42 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M51.2 42 H54.8 A1.2 1.2 0 0 1 56 43.2 V46.8 A1.2 1.2 0 0 1 54.8 48 H51.2 A1.2 1.2 0 0 1 50 46.8 V43.2 A1.2 1.2 0 0 1 51.2 42 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M61.2 42 H64.8 A1.2 1.2 0 0 1 66 43.2 V46.8 A1.2 1.2 0 0 1 64.8 48 H61.2 A1.2 1.2 0 0 1 60 46.8 V43.2 A1.2 1.2 0 0 1 61.2 42 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M31.2 52 H34.8 A1.2 1.2 0 0 1 36 53.2 V56.8 A1.2 1.2 0 0 1 34.8 58 H31.2 A1.2 1.2 0 0 1 30 56.8 V53.2 A1.2 1.2 0 0 1 31.2 52 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M41.2 52 H44.8 A1.2 1.2 0 0 1 46 53.2 V56.8 A1.2 1.2 0 0 1 44.8 58 H41.2 A1.2 1.2 0 0 1 40 56.8 V53.2 A1.2 1.2 0 0 1 41.2 52 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M51.2 52 H54.8 A1.2 1.2 0 0 1 56 53.2 V56.8 A1.2 1.2 0 0 1 54.8 58 H51.2 A1.2 1.2 0 0 1 50 56.8 V53.2 A1.2 1.2 0 0 1 51.2 52 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M61.2 52 H64.8 A1.2 1.2 0 0 1 66 53.2 V56.8 A1.2 1.2 0 0 1 64.8 58 H61.2 A1.2 1.2 0 0 1 60 56.8 V53.2 A1.2 1.2 0 0 1 61.2 52 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M31.2 62 H34.8 A1.2 1.2 0 0 1 36 63.2 V66.8 A1.2 1.2 0 0 1 34.8 68 H31.2 A1.2 1.2 0 0 1 30 66.8 V63.2 A1.2 1.2 0 0 1 31.2 62 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M41.2 62 H44.8 A1.2 1.2 0 0 1 46 63.2 V66.8 A1.2 1.2 0 0 1 44.8 68 H41.2 A1.2 1.2 0 0 1 40 66.8 V63.2 A1.2 1.2 0 0 1 41.2 62 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M51.2 62 H54.8 A1.2 1.2 0 0 1 56 63.2 V66.8 A1.2 1.2 0 0 1 54.8 68 H51.2 A1.2 1.2 0 0 1 50 66.8 V63.2 A1.2 1.2 0 0 1 51.2 62 Z", dash = "2 2", alpha = 0.7f)
    shape(k, "M61.2 62 H64.8 A1.2 1.2 0 0 1 66 63.2 V66.8 A1.2 1.2 0 0 1 64.8 68 H61.2 A1.2 1.2 0 0 1 60 66.8 V63.2 A1.2 1.2 0 0 1 61.2 62 Z", dash = "2 2", alpha = 0.7f)
    // ecal-mark
    layer(o17) {
        withTransform({ translate(53f, 55f); scale(s17, s17, Offset.Zero); translate(-53f, -55f) }) {
            shape(k, "M59.5 55 A6.5 6.5 0 0 1 53 61.5 A6.5 6.5 0 0 1 46.5 55 A6.5 6.5 0 0 1 53 48.5 A6.5 6.5 0 0 1 59.5 55 Z")
        }
    }
    // ecal-sheet
    withTransform({ translate(48f, 24f); scale(1f, sy19, Offset.Zero); translate(-48f, -24f) }) {
        shape(k, "M28 24 H68 A4 4 0 0 1 72 28 V31 A4 4 0 0 1 68 35 H28 A4 4 0 0 1 24 31 V28 A4 4 0 0 1 28 24 Z", paper = true)
    }
    shape(k, "M35 19 v9 M61 19 v9")
}

/** The one pill under empty art (same look as the FriendlyState action). */
@Composable
fun EmptyPill(label: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        modifier = modifier.height(40.dp)
            .clip(CircleShape).background(c.bgPanel)
            .border(BorderStroke(1.dp, c.borderSubtle), CircleShape)
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) { Text(label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
}

/**
 * The Gizzi mascot walking in (style E: agent and terminal screens), in a 96 dp box with the mascot at 72 dp.
 * Walk-in: slides in from the left over 0.9 s with two small hops, then rests. Reduced motion = the idle frame.
 * (Android has no pixel-pet sprite sheet yet; this reuses the shipped mascot art, the same image SetupGizzi shows.)
 */
@Composable
fun GizziWalk(modifier: Modifier = Modifier, size: Dp = 72.dp) = GizziWalkBox(modifier, size, 96.dp, still = false)

/** GizziWalk in a [box] square (EmptyArtKind.Gizzi: box = size, mascot = round(size * 0.75)); the walk scales with it. */
@Composable
private fun GizziWalkBox(modifier: Modifier, size: Dp, box: Dp, still: Boolean) {
    val reduce = rememberDotReducedMotion() || still
    val k = box.value / 96f
    val x = remember { androidx.compose.animation.core.Animatable(if (reduce) 0f else -1f) }
    val hop = remember { androidx.compose.animation.core.Animatable(0f) }
    androidx.compose.runtime.LaunchedEffect(reduce) {
        if (reduce) { x.snapTo(0f); return@LaunchedEffect }
        x.animateTo(0f, tween(900, easing = EaOut))
    }
    androidx.compose.runtime.LaunchedEffect(reduce) {
        if (reduce) return@LaunchedEffect
        repeat(2) {
            hop.animateTo(1f, tween(225, easing = EaOut)); hop.animateTo(0f, tween(225, easing = EaIn))
        }
    }
    Box(modifier.size(box).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        Image(
            painter = painterResource(R.drawable.gizzi_mascot), contentDescription = null,
            modifier = Modifier.size(size).offset(x = (x.value * 40f * k).dp, y = (-4f * hop.value * k).dp),
        )
    }
}
