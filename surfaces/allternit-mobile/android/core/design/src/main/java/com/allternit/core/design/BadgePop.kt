package com.allternit.core.design

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Fill
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipRect
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.withTransform
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

/**
 * Grey badge that pops to one solid colour when earned. A port of reference/BadgePop.reference.tsx (PLAN-v2 3.9).
 * Plays once on the state change (0.8 s), never on first composition: mounting earned shows the end frame.
 * Animator scale 0 (reduce motion) switches straight to the earned or grey frame with no wipe, ring or draw.
 */
enum class BadgeShape { Seal, Hex, Flower }
enum class BadgeTone { Success, Accent }

private val BpOut = CubicBezierEasing(.16f, 1f, .3f, 1f)
private val BpIo = CubicBezierEasing(.65f, 0f, .35f, 1f)
private const val EARN_MS = 800

private val sealPath: Path by lazy {
    // 12 lobes: radius 46 with amplitude 6, 96 steps (reference SEAL_D).
    val n = 12; val r = 46f; val a = 6f
    Path().apply {
        for (i in 0..96) {
            val t = i / 96f * (2f * PI.toFloat())
            val rr = r + a * cos(n * t)
            val x = 60f + rr * cos(t); val y = 60f + rr * sin(t)
            if (i == 0) moveTo(x, y) else lineTo(x, y)
        }
        close()
    }
}
private val hexPath: Path by lazy { PathParser().parsePathString("M60 8 L105 34 L105 86 L60 112 L15 86 L15 34 Z").toPath() }
private val checkPath: Path by lazy { PathParser().parsePathString("M42 62 L55 75 L79 48").toPath() }
private val boltPath: Path by lazy { PathParser().parsePathString("M66 34 L46 64 H62 L54 88 L76 56 H60 Z").toPath() }
private val starPath: Path by lazy {
    PathParser().parsePathString("M60 38 L66.5 53 L82 54.6 L70.3 65 L74 80.5 L60 72.4 L46 80.5 L49.7 65 L38 54.6 L53.5 53 Z").toPath()
}

private fun DrawScope.shapeFill(shape: BadgeShape, color: Color) {
    when (shape) {
        BadgeShape.Seal -> drawPath(sealPath, color, style = Fill)
        BadgeShape.Hex -> {
            drawPath(hexPath, color, style = Fill)
            drawPath(hexPath, color, style = Stroke(width = 8f, join = StrokeJoin.Round))
        }
        BadgeShape.Flower -> {
            for (deg in 0 until 360 step 60) {
                val rad = deg * PI.toFloat() / 180f
                drawCircle(color, 24f, Offset(60f + 30f * cos(rad), 60f + 30f * sin(rad)))
            }
            drawCircle(color, 26f, Offset(60f, 60f))
        }
    }
}

private fun seg(t: Float, from: Float, len: Float) = ((t - from) / len).coerceIn(0f, 1f)

/**
 * [earned] flips the badge. [size] is the render size (reference default 40). [label] is the accessible name;
 * colour is never the only signal, since the mark is always drawn when earned.
 */
@Composable
fun BadgePop(
    earned: Boolean,
    modifier: Modifier = Modifier,
    shape: BadgeShape = BadgeShape.Seal,
    tone: BadgeTone = BadgeTone.Success,
    size: Dp = 40.dp,
    label: String = if (earned) "Earned" else "Not yet",
) {
    val c = AllternitTheme.colors
    val reduce = rememberDotReducedMotion()
    val on = if (tone == BadgeTone.Accent) c.accentPrimary else AllternitStatic.StatusSuccess
    val markOn = if (tone == BadgeTone.Accent) c.bgPrimary else Color.White
    val grey = c.textPrimary.copy(alpha = 0.16f)

    // wipe 0..1 = how much of the colour layer shows. t = seconds into the earn timeline (0.8 = settled).
    val wipe = remember { Animatable(if (earned) 1f else 0f) }
    val t = remember { Animatable(if (earned) 0.8f else 0f) }
    val first = remember { booleanArrayOf(true) }
    LaunchedEffect(earned, reduce) {
        if (first[0]) { first[0] = false; wipe.snapTo(if (earned) 1f else 0f); t.snapTo(if (earned) 0.8f else 0f); return@LaunchedEffect }
        if (reduce) { wipe.snapTo(if (earned) 1f else 0f); t.snapTo(if (earned) 0.8f else 0f); return@LaunchedEffect }
        if (earned) {
            wipe.snapTo(0f); t.snapTo(0f)
            t.animateTo(0.8f, tween(EARN_MS, easing = LinearEasing))
        } else {
            // Un-earn: reverse in 0.25 s, no ring.
            t.snapTo(0.8f)
            wipe.animateTo(0f, tween(250, easing = BpIo))
            t.snapTo(0f)
        }
    }

    Canvas(
        modifier.size(size).semantics { contentDescription = label; role = Role.Image },
    ) {
        val s = this.size.width / 120f
        withTransform({ scale(s, s, Offset.Zero) }) {
            val tt = t.value
            val earning = earned && tt < 0.8f
            // Wipe: while earning it follows the timeline (0.40 s, EASE_OUT); otherwise the animatable.
            val w = if (earning) BpOut.transform(seg(tt, 0f, 0.4f)) else wipe.value
            val pop = if (earning) {
                // .92 -> 1.04 at .28 -> 1 at .50, EASE_OUT per segment.
                if (tt < 0.28f) 0.92f + (1.04f - 0.92f) * BpOut.transform(tt / 0.28f)
                else 1.04f + (1f - 1.04f) * BpOut.transform(seg(tt, 0.28f, 0.22f))
            } else 1f
            // Glow ring behind the shape.
            if (earning && tt >= 0.1f) {
                val e = BpOut.transform(seg(tt, 0.1f, 0.7f))
                val rs = 0.85f + 0.4f * e
                withTransform({ scale(rs, rs, Offset(60f, 60f)) }) {
                    drawCircle(on.copy(alpha = 0.5f * (1f - e)), 50f, Offset(60f, 60f), style = Stroke(width = 3f - 2f * e))
                }
            }
            withTransform({ scale(pop, pop, Offset(60f, 60f)) }) {
                shapeFill(shape, grey)
                if (w > 0f) clipRect(left = 0f, top = 0f, right = 120f * w, bottom = 120f) { shapeFill(shape, on) }
                // Mark: visible while earned; fades with the wipe on un-earn.
                val markAlpha = if (earned) 1f else wipe.value
                if (markAlpha > 0f) {
                    val mk = markOn.copy(alpha = markAlpha)
                    when (shape) {
                        BadgeShape.Seal -> {
                            val d = if (earning) BpOut.transform(seg(tt, 0.15f, 0.4f)) else 1f
                            if (d > 0f) {
                                val pm = PathMeasure().apply { setPath(checkPath, false) }
                                val part = Path()
                                pm.getSegment(0f, pm.length * d, part, true)
                                drawPath(part, mk, style = Stroke(width = 7f, cap = StrokeCap.Round, join = StrokeJoin.Round))
                            }
                        }
                        BadgeShape.Hex, BadgeShape.Flower -> {
                            val m = if (earning) BpOut.transform(seg(tt, 0.2f, 0.3f)) else 1f
                            if (m > 0f) {
                                val p = if (shape == BadgeShape.Hex) boltPath else starPath
                                val centre = if (shape == BadgeShape.Hex) Offset(61f, 61f) else Offset(60f, 59f)
                                withTransform({ scale(m, m, centre) }) {
                                    drawPath(p, mk, style = Fill)
                                    drawPath(p, mk, style = Stroke(width = 3f, join = StrokeJoin.Round))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
