package com.allternit.core.design

import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Paint
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Empty-state art. A port of reference/EmptyArt.reference.tsx (PLAN-v2 1.1): same paths, same 3.4 s loop,
 * same per-segment easings and still frames. Never redraw; change the reference first.
 * One colour (text primary), 1.5 unit stroke, round caps and joins, paper fill = the screen background.
 * Animator scale 0 (reduce motion) shows the still frame and nothing moves.
 */
enum class EmptyArtKind { Sheets, Bell, Envelope, Coins }

private const val LOOP_MS = 3400
private const val VIEWBOX = 96f

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
 * The four empty-state drawings. [size] is the square render size (96 dp default). Decorative, so it is hidden
 * from accessibility; the headline and line under it carry the meaning. [still] forces the still frame.
 */
@Composable
fun EmptyArt(kind: EmptyArtKind, modifier: Modifier = Modifier, size: Dp = 96.dp, still: Boolean = false) {
    val reduce = rememberDotReducedMotion() || still
    val c = AllternitTheme.colors
    val ink = remember(c.textPrimary, c.bgPrimary) { Ink(c.textPrimary, c.bgPrimary) }
    if (!reduce) {
        val inf = rememberInfiniteTransition(label = "emptyArt")
        val phase by inf.animateFloat(0f, 1f, infiniteRepeatable(tween(LOOP_MS, easing = LinearEasing), RepeatMode.Restart), label = "loop")
        val dotPhase by inf.animateFloat(0f, 1f, infiniteRepeatable(tween(500, easing = LinearEasing), RepeatMode.Restart), label = "dots")
        Canvas(modifier.size(size).clearAndSetSemantics {}) { drawArt(kind, ink, phase, dotPhase) }
    } else {
        Canvas(modifier.size(size).clearAndSetSemantics {}) { drawArt(kind, ink, null, 0f) }
    }
}

private fun DrawScope.drawArt(kind: EmptyArtKind, ink: Ink, phase: Float?, dotPhase: Float) {
    val s = this.size.width / VIEWBOX
    withTransform({ scale(s, s, Offset.Zero) }) {
        when (kind) {
            EmptyArtKind.Sheets -> sheets(ink, phase)
            EmptyArtKind.Bell -> bell(ink, phase)
            EmptyArtKind.Coins -> coins(ink, phase)
            EmptyArtKind.Envelope -> {
                // ea-dot: 0.5 s bob, 0 -> -2.5 -> 0, staggered 0.12 s; still frame has no bob.
                val dots = FloatArray(3) { i ->
                    if (phase == null) 0f else {
                        val q = ((dotPhase - 0.24f * i) % 1f + 1f) % 1f
                        kf(q, K(0f, 0f), K(50f, -2.5f), K(100f, 0f))
                    }
                }
                envelope(ink, phase, dots)
            }
        }
    }
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
fun GizziWalk(modifier: Modifier = Modifier, size: Dp = 72.dp) {
    val reduce = rememberDotReducedMotion()
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
    Box(modifier.size(96.dp).clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        Image(
            painter = painterResource(R.drawable.gizzi_mascot), contentDescription = null,
            modifier = Modifier.size(size).offset(x = (x.value * 40f).dp, y = (-4f * hop.value).dp),
        )
    }
}
