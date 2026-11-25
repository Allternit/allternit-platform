package com.allternit.feature.shell.firstrun

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.snap
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.widthIn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathMeasure
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp

// PLAN-v2 step 2.16. Port of handoffs/ui-polish-2026-11-24/reference/ConnectScene.reference.tsx
// (viewBox 0 0 120 72, stroke 1.5, round caps). Driven ONLY by the real connection state; no timer of its own.

internal enum class ConnectSceneState { Asleep, Connecting, Connected }

/** off / needs -> asleep, connecting -> connecting, on -> connected. A failure lands on Asleep (never an unearned check). */
internal fun ConnState.sceneState(): ConnectSceneState = when (this) {
    ConnState.Connecting -> ConnectSceneState.Connecting
    ConnState.On -> ConnectSceneState.Connected
    ConnState.Off, ConnState.Needs -> ConnectSceneState.Asleep
}

private val EaseOut = CubicBezierEasing(.16f, 1f, .3f, 1f)
private val EaseInOut = CubicBezierEasing(.65f, 0f, .35f, 1f)

@Composable
internal fun ConnectScene(state: ConnectSceneState, modifier: Modifier = Modifier) {
    val c = setupColors
    val reduced = reducedMotion()
    fun <T> spec(ms: Int, delay: Int = 0, easing: androidx.compose.animation.core.Easing = EaseOut) =
        if (reduced) snap<T>() else tween<T>(ms, delay, easing)

    val awake = state != ConnectSceneState.Asleep
    val gizziAlpha by animateFloatAsState(if (awake) 1f else .55f, spec(400), label = "csGizzi")
    val cableAlpha by animateFloatAsState(if (awake) 1f else .35f, spec(400), label = "csCable")
    val zAlpha by animateFloatAsState(if (state == ConnectSceneState.Asleep) 1f else 0f, spec(160, easing = EaseInOut), label = "csZ")
    val connected = state == ConnectSceneState.Connected
    // Badge: in 0.30 s after 0.25 s; out in 0.25 s ease-in-out.
    val badge by animateFloatAsState(
        if (connected) 1f else 0f,
        if (connected) spec(300, 250) else spec(250, easing = EaseInOut),
        label = "csBadge",
    )
    // Device screen: one 0.4 s fade .5 -> 1 each time it becomes connected.
    val screen = remember { Animatable(1f) }
    LaunchedEffect(connected, reduced) {
        if (connected && !reduced) {
            screen.snapTo(.5f)
            screen.animateTo(1f, tween(400, easing = EaseOut))
        } else screen.snapTo(1f)
    }
    val zClock = if (state == ConnectSceneState.Asleep) loopClock(2.4f, "csZLoop") else 0f
    val runClock = if (state == ConnectSceneState.Connecting) loopClock(1.2f, "csRun") else 0f

    Box(modifier.fillMaxWidth().widthIn(max = 240.dp).aspectRatio(120f / 72f).clearAndSetSemantics { }) {
        Canvas(Modifier.fillMaxWidth().aspectRatio(120f / 72f)) {
            val s = size.width / 120f
            val ink = c.ink
            val sw = Stroke(width = 1.5f * s, cap = StrokeCap.Round, join = StrokeJoin.Round)
            fun p(x: Float, y: Float) = Offset(x * s, y * s)

            // z marks (asleep): 2.4 s loop, second one delayed 1.2 s. Reduced = still at opacity .6.
            fun zMark(path: Path, delay: Float) {
                val ph = if (reduced) 0f else ((zClock - delay) / 2.4f).let { it - kotlin.math.floor(it) }
                val tx: Float; val ty: Float; val a: Float
                if (reduced) { tx = 0f; ty = 0f; a = .6f } else {
                    val e = EaseInOut.transform(ph)
                    tx = 5f * e; ty = -8f * e
                    a = if (ph < .3f) EaseInOut.transform(ph / .3f) else 1f - EaseInOut.transform((ph - .3f) / .7f)
                }
                val alpha = a * zAlpha
                if (alpha > 0f) {
                    translateStroke(path, tx * s, ty * s, ink.copy(alpha = alpha), sw)
                }
            }
            zMark(Path().apply { moveTo(12 * s, 10 * s); relativeLineTo(5 * s, 0f); relativeLineTo(-5 * s, 6 * s); relativeLineTo(5 * s, 0f) }, 0f)
            zMark(Path().apply { moveTo(20 * s, 4 * s); relativeLineTo(3.5f * s, 0f); relativeLineTo(-3.5f * s, 4 * s); relativeLineTo(3.5f * s, 0f) }, 1.2f)

            // Cables (pathLength 1): stubs when asleep, travelling dashes when connecting, solid when connected.
            fun cable(y0: Float, y1: Float, delay: Float) {
                val path = Path().apply { moveTo(42 * s, y0 * s); cubicTo(56 * s, y0 * s, 60 * s, y1 * s, 75.4f * s, 33 * s) }
                val col = ink.copy(alpha = cableAlpha)
                when (state) {
                    ConnectSceneState.Connected -> drawPath(path, col, style = sw)
                    ConnectSceneState.Connecting -> {
                        val t = if (reduced) 0f else (((runClock - delay) / 1.2f).let { it - kotlin.math.floor(it) })
                        dashes(path, col, sw, listOf(t to minOf(t + .22f, 1f)) + if (t > .78f) listOf(0f to (t - .78f)) else emptyList())
                    }
                    ConnectSceneState.Asleep -> dashes(path, col, sw, listOf(0f to .06f, 1f to 1f))
                }
            }
            cable(30f, 26f, 0f)
            cable(44f, 40f, .6f)

            // Device
            val paper = c.bg
            drawRoundRect(paper, p(78f, 18f), Size(36 * s, 30 * s), CornerRadius(4 * s))
            drawRoundRect(ink, p(78f, 18f), Size(36 * s, 30 * s), CornerRadius(4 * s), style = sw)
            drawRoundRect(ink.copy(alpha = screen.value), p(82f, 22f), Size(28 * s, 18 * s), CornerRadius(2 * s), style = sw)
            drawLine(ink, p(96f, 48f), p(96f, 54f), 1.5f * s, StrokeCap.Round)
            drawLine(ink, p(88f, 54f), p(104f, 54f), 1.5f * s, StrokeCap.Round)
            drawCircle(paper, 2.6f * s, p(78f, 33f))
            drawCircle(ink, 2.6f * s, p(78f, 33f), style = sw)
            if (state == ConnectSceneState.Connecting && !reduced) {
                val f = runClock / 1.2f
                val e = EaseOut.transform(f - kotlin.math.floor(f))
                drawCircle(ink.copy(alpha = .6f * (1f - e)), 2.6f * s * (1f + .8f * e), p(78f, 33f), style = sw)
            }

            // Check badge: scale .6 -> 1, opacity 0 -> 1 (over the screen)
            if (badge > 0f) {
                val k = .6f + .4f * badge
                scale(k, k, pivot = p(96f, 33f)) {
                    drawCircle(paper.copy(alpha = badge), 9 * s, p(96f, 33f))
                    drawCircle(ink.copy(alpha = badge), 9 * s, p(96f, 33f), style = sw)
                    val tick = Path().apply { moveTo(91.5f * s, 33 * s); relativeLineTo(3 * s, 3.2f * s); relativeLineTo(6.5f * s, -6.8f * s) }
                    drawPath(tick, c.ok.copy(alpha = badge), style = sw)
                }
            }
        }
        // Gizzi slot: box x 6..42, y 14..62 (36 x 48), sprite 44 centred. Centre (24, 38) of 120 x 72.
        BoxGizzi(gizziAlpha)
    }
}

@Composable
private fun BoxGizzi(alpha: Float) {
    // 24/120 and 38/72 of the scene, sprite 44 dp; centred on the slot centre.
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth().aspectRatio(120f / 72f)) {
        val w = maxWidth
        val sprite = 44.dp * (w / 240.dp).coerceAtMost(1f)
        Box(Modifier.offset(x = w * (24f / 120f) - sprite / 2, y = w * (38f / 120f) - sprite / 2).alpha(alpha)) { SetupGizzi(sprite) }
    }
}

private fun DrawScope.translateStroke(path: Path, dx: Float, dy: Float, color: Color, style: Stroke) {
    androidx.compose.ui.graphics.drawscope.translate(dx, dy) { drawPath(path, color, style = style) }
}

/** Draws [spans] (fractions 0..1 of the path length) as round-capped dashes; zero-length spans draw a dot. */
private fun DrawScope.dashes(path: Path, color: Color, style: Stroke, spans: List<Pair<Float, Float>>) {
    val m = PathMeasure().apply { setPath(path, false) }
    val len = m.length
    spans.forEach { (a, b) ->
        val seg = Path()
        if (b - a <= 0.0005f) {
            val pos = m.getPosition(len * a.coerceIn(0f, 1f))
            drawCircle(color, style.width / 2f, pos)
        } else {
            m.getSegment(len * a.coerceIn(0f, 1f), len * b.coerceIn(0f, 1f), seg, true)
            drawPath(seg, color, style = style)
        }
    }
}
