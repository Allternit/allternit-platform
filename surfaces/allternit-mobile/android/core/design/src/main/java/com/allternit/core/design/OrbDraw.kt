package com.allternit.core.design

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin

/** Draws one frame of [state] at time [t] seconds. Port of iOS `ThinkingOrb.draw`; math is in dp. */
internal fun DrawScope.drawOrb(state: ThinkingOrbState, size: ThinkingOrbSize, t: Double, ink: Color) {
    val d = density.toDouble()
    val cx = this.size.width / 2.0
    val cy = this.size.height / 2.0
    val dotRadius = size.dotRadius.toDouble()
    val radius = min(this.size.width, this.size.height) / d / 2 - dotRadius * 2
    val dotCount = size.dotCount

    fun dot(x: Double, y: Double, opacity: Double = 1.0) {
        drawCircle(ink.copy(alpha = (ink.alpha * opacity).toFloat().coerceIn(0f, 1f)), (dotRadius * d).toFloat(), Offset((cx + x * d).toFloat(), (cy + y * d).toFloat()))
    }
    val tau = 2 * PI

    when (state) {
        ThinkingOrbState.Working -> for (ring in 0 until 3) {
            val tilt = ring * 0.9
            val ringRadius = radius * (0.45 + 0.28 * ring)
            val ringSpeed = 0.5 + ring * 0.22
            val perRing = max(4, dotCount / 3)
            for (i in 0 until perRing) {
                val angle = i.toDouble() / perRing * tau + t * ringSpeed
                dot(cos(angle) * ringRadius, sin(angle) * ringRadius * cos(tilt), 0.5 + 0.5 * cos(angle))
            }
        }
        ThinkingOrbState.Searching -> {
            val lats = 5
            for (lat in 0 until lats) {
                val f = lat.toDouble() / (lats - 1) - 0.5
                val latRadius = radius * cos(f * PI * 0.9)
                val y = sin(f * PI * 0.9) * radius
                val perLat = max(6, (dotCount * cos(f * PI * 0.4)).toInt())
                for (i in 0 until perLat) dot(cos(i.toDouble() / perLat * tau) * latRadius, y, 0.35)
            }
            val sweep = (t % 2) / 2 * tau
            for (lat in 0 until lats) {
                val f = lat.toDouble() / (lats - 1) - 0.5
                dot(cos(sweep) * radius * cos(f * PI * 0.9), sin(f * PI * 0.9) * radius, 1.0)
            }
        }
        ThinkingOrbState.Solving -> {
            val bands = 4
            val quarterTurns = floor(t / 0.6)
            val settle = min(1.0, (t / 0.6 - quarterTurns) / 0.4)
            for (band in 0 until bands) {
                val bandY = (band.toDouble() / (bands - 1) - 0.5) * radius * 1.6
                val scramble = sin(quarterTurns * 2.1 + band * 1.7) * PI / 2
                val settled = (scramble / (PI / 2)).roundHalfAway() * (PI / 2)
                val offset = scramble + (settled - scramble) * settle
                val perBand = max(4, dotCount / bands)
                for (i in 0 until perBand) dot(cos(i.toDouble() / perBand * tau + offset) * radius * 0.85, bandY, 0.7)
            }
        }
        ThinkingOrbState.Listening -> for (ring in 0 until 4) {
            val ringRadius = radius * (0.3 + 0.23 * ring)
            val perRing = max(8, dotCount)
            for (i in 0 until perRing) {
                val angle = i.toDouble() / perRing * tau
                val r = ringRadius + sin(angle * 3 + t * 3 - ring * 0.6) * radius * 0.08
                dot(cos(angle) * r, sin(angle) * r, 0.6)
            }
        }
        ThinkingOrbState.Connecting -> {
            val n = max(6, dotCount / 2)
            val nodes = List(n) { i ->
                val angle = i.toDouble() / n * tau + 0.3
                val r = radius * (0.55 + 0.35 * sin(i * 2.3))
                doubleArrayOf(cos(angle) * r, sin(angle) * r)
            }
            for (i in nodes.indices) {
                val a = nodes[i]
                val b = nodes[(i + 2) % n]
                drawLine(ink.copy(alpha = ink.alpha * 0.18f), Offset((cx + a[0] * d).toFloat(), (cy + a[1] * d).toFloat()),
                    Offset((cx + b[0] * d).toFloat(), (cy + b[1] * d).toFloat()), strokeWidth = (0.6 * d).toFloat())
                dot(a[0], a[1], 0.6)
            }
            val p = (t % 1.4) / 1.4
            val from = nodes[(t / 1.4).toInt() % n]
            val to = nodes[((t / 1.4).toInt() % n + 2) % n]
            dot(from[0] + (to[0] - from[0]) * p, from[1] + (to[1] - from[1]) * p, 1.0)
        }
        ThinkingOrbState.Weaving -> {
            val strands = 3
            val points = max(10, dotCount / strands)
            for (s in 0 until strands) {
                val phase = s.toDouble() / strands * tau
                for (i in 0 until points) {
                    val progress = i.toDouble() / points * tau
                    dot(sin(progress + t * 0.8 + phase) * radius * 0.85,
                        (progress / tau - 0.5) * radius * 1.7 * cos(t * 0.2),
                        0.4 + 0.5 * cos(progress + t * 0.8 + phase))
                }
            }
        }
        ThinkingOrbState.Composing -> for (band in 0 until 4) {
            val y = (band.toDouble() / 3 - 0.5) * radius * 1.5
            val perBand = max(8, dotCount)
            for (i in 0 until perBand) {
                val x = (i.toDouble() / (perBand - 1) - 0.5) * radius * 2
                dot(x, y + sin(x * 0.15 + t * 1.4 + band * 0.8) * radius * 0.12, 0.55)
            }
        }
        ThinkingOrbState.Breathing -> {
            val breath = 0.75 + 0.25 * sin(t * 0.9)
            val perRing = max(10, dotCount)
            for (i in 0 until perRing) {
                val angle = i.toDouble() / perRing * tau + t * 0.15
                dot(cos(angle) * radius * breath, sin(angle) * radius * breath, 0.5 + 0.3 * sin(t * 0.9))
            }
        }
        ThinkingOrbState.Shaping -> {
            val cycle = 3.0
            val phase = (t % cycle) / cycle
            val shape = (t / cycle).toInt() % 3
            val next = (shape + 1) % 3
            val eased = phase * phase * (3 - 2 * phase)
            val per = max(16, dotCount)
            for (i in 0 until per) {
                val progress = i.toDouble() / per
                val a = outlinePoint(shape, progress, radius)
                val b = outlinePoint(next, progress, radius)
                dot(a.first + (b.first - a.first) * eased, a.second + (b.second - a.second) * eased, 0.7)
            }
        }
    }
}

/** Swift `round` (half away from zero); Kotlin's `roundToInt` rounds half up. */
private fun Double.roundHalfAway(): Double = if (this < 0) -((-this).roundToInt().toDouble()) else roundToInt().toDouble()

private fun outlinePoint(shape: Int, progress: Double, radius: Double): Pair<Double, Double> {
    val angle = progress * 2 * PI
    return when (shape) {
        0 -> cos(angle) * radius to sin(angle) * radius
        1 -> polygonPoint(3, angle, radius)
        else -> polygonPoint(4, angle, radius)
    }
}

private fun polygonPoint(sides: Int, angle: Double, radius: Double): Pair<Double, Double> {
    val sector = 2 * PI / sides
    val sectorAngle = angle % sector
    val corner = sector / 2
    val edge = radius / cos(corner) * cos(sectorAngle - corner)
    return cos(angle) * edge to sin(angle) * edge
}
