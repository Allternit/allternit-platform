package com.allternit.feature.chat.voice

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import kotlin.math.PI
import kotlin.math.cbrt
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

/*
 * The Ring speaker style: a port of iOS VoiceRingKit (VoiceRing.swift → Aura.swift's frame, VoiceFollower.swift,
 * Math.swift's skinField, LightPainter.swift). Six glassy rings lean off the view axis and bundle into one ribbon,
 * a conic spectrum runs round it and the voice bends every ring together.
 *
 * Differences from iOS, on purpose: the halo/bloom layer is not blurred by 1.5 pt (Compose has no per-layer blur
 * below API 31; the stacked passes already fall off softly); no lean toward a voice's side (voice mode passes no
 * direction on iOS either); no mood glow (iOS passes none). Everything is computed in dp and drawn scaled by density.
 */

private const val TAU = PI * 2

private fun clamp01(v: Double) = if (v < 0) 0.0 else if (v > 1) 1.0 else v
private fun hashD(a: Double, b: Double): Double { val h = sin(a * 12.9898 + b * 78.233) * 43758.5453; return h - floor(h) }
private fun sinc(a: Double) = if (a < 1e-6) 1.0 else sin(a) / a
private fun radiusScale(size: Double, p: Double = 0.6) = (size / 200).pow(p)

/** iOS VoiceState as the ring reads it. */
internal enum class RingState { Idle, Listening, Speaking, Processing }

/** What the voice is doing at one instant, shaped and smoothed (iOS VoiceInput, the fields Aura reads). */
internal class RingVoice {
    var level = 0.0
    val bands = DoubleArray(3)
    var soft = 0.0
    val softBands = DoubleArray(3)
    var drive = 0.0
    var processing = 0.0
    var spin = 0.0
}

/**
 * iOS VoiceFollower on its plain-level path (readLevel / readSilence / step) with the Ring preset's tuning:
 * speed 1.6, driveGain 2 × voiceSpeed 0.9, attack 0.06 s; voice mode's sensitivity 0.9.
 */
internal class RingFollower {
    var clock = 0.0
        private set
    var t = 0.0
        private set
    private var drive = 0.0
    private var level = 0.0
    private val bands = DoubleArray(3)
    private var heard = 0.0
    private var proc = 0.0
    private var spin = 0.0
    private var rawLevel = 0.0
    private val rawBands = DoubleArray(3)
    val out = RingVoice()

    private val sensitivity = 0.9
    private val threshold = 0.04
    private val attack = 0.06
    private val release = 0.32
    private val idle = 0.1
    private val calm = 0.4
    private val rate = 1.6
    private val driveGain = 2.0 * 0.9

    private fun shape(raw: Double, threshold: Double): Double {
        if (raw <= threshold) return 0.0
        val t = (raw - threshold) / max(0.001, 1 - threshold)
        return clamp01((1 - exp(-3 * t)) / (1 - exp(-3.0)))
    }

    private fun follow(prev: Double, target: Double, dt: Double, attack: Double, release: Double): Double {
        val tau = if (target > prev) attack else release
        return prev + (target - prev) * (1 - exp(-dt / max(0.001, tau)))
    }

    fun readLevel(value: Double) {
        val raw = clamp01(value) * sensitivity
        rawLevel = raw
        rawBands[0] = raw
        rawBands[1] = raw * (0.72 + 0.28 * sin(clock * 2.3))
        rawBands[2] = raw * (0.6 + 0.4 * sin(clock * 3.7 + 2))
    }

    fun readSilence() {
        rawLevel = 0.0
        rawBands.fill(0.0)
    }

    fun step(dt: Double, processing: Boolean, still: Boolean): RingVoice {
        proc = follow(proc, if (processing) 1.0 else 0.0, dt, 0.35, 0.5)
        val p = proc
        val pace = calm + (1 - calm) * min(1.0, max(heard * 2.5, p))
        if (!still) {
            clock += dt
            t += dt * rate * pace * (1 + 0.5 * p)
        }
        level = follow(level, shape(rawLevel, threshold), dt, attack, release)
        heard = follow(heard, level, dt, 0.35, 0.9)
        for (b in 0 until 3) bands[b] = follow(bands[b], shape(rawBands[b], threshold * 0.6), dt, attack, release * 1.15)
        val breathe = if (still) 0.5 else 0.5 + 0.5 * sin(TAU * clock / 6)
        val lift = idle * breathe * (1 - p)
        out.level = level + (1 - level) * lift
        for (b in 0 until 3) out.bands[b] = bands[b] + (1 - bands[b]) * lift * 0.6
        out.soft = follow(out.soft, out.level, dt, 0.35, 0.9)
        for (b in 0 until 3) out.softBands[b] = follow(out.softBands[b], out.bands[b], dt, 0.35, 0.9)
        if (!still) drive += dt * rate * (pace + driveGain * level + 1.3 * p)
        out.drive = drive
        out.processing = p
        if (!still) {
            val home = ceil(spin / TAU - 1e-6) * TAU
            val settle = min(0.3, (home - spin) * 0.6)
            spin += dt * (2.1 * rate * p + settle * (1 - p))
        }
        out.spin = spin
        return out
    }
}

/** One ring of the ribbon: flat [x0, y0, x1, y1, …] in dp and its mean depth opacity. */
internal class RingStroke(val pts: FloatArray, val alpha: Float)

internal class RingSpec(val x: Float, val y: Float, val r: Float, val a: Float)

/** One rendered instant (iOS OrbFrame + FrameLight for the aura type). */
internal class RingFrame(
    val strokes: List<RingStroke>,
    val lineWidth: Float,
    val cx: Float, val cy: Float, val r: Float,
    /** Radians clockwise from +x where the spectrum starts. */
    val angle: Double,
    val intensity: Double,
    val halo: Double,
    val spec: RingSpec,
    val fringe: Double,
)

/** iOS skinField: the shared bulge field, with its average over the sphere taken out so it only reshapes. */
private fun skinField(aLobe: Double, aFine: Double, T: Double, radius: Double): (Double, Double, Double) -> Double {
    val k = 1 / max(0.05, radius)
    val p1 = 0.9 * T; val p2 = -0.7 * T + 1.3; val p3 = 2.1 * T; val p4 = -1.6 * T
    val kLobe1 = hypot(2.1, 1.3); val kLobe2 = hypot(1.9, 1.7)
    val kFineDiff = sqrt((4.3 - 2.9).pow(2) + 3.7 * 3.7 + 3.1 * 3.1)
    val kFineSum = sqrt((4.3 + 2.9).pow(2) + 3.7 * 3.7 + 3.1 * 3.1)
    val mean = aLobe * (0.55 * sin(p1) * sinc(kLobe1 * k) + 0.45 * sin(p2) * sinc(kLobe2 * k)) +
        aFine * 0.5 * (cos(p3 - p4) * sinc(kFineDiff * k) - cos(p3 + p4) * sinc(kFineSum * k))
    return { x0, y0, z0 ->
        val x = x0 * k; val y = y0 * k; val z = z0 * k
        aLobe * (0.55 * sin(2.1 * x + 1.3 * y + p1) + 0.45 * sin(1.7 * z - 1.9 * y + p2)) +
            aFine * sin(4.3 * x - 3.1 * z + p3) * sin(3.7 * y + 2.9 * x + p4) - mean
    }
}

/** iOS auraFrame with VoiceRing.preset's knobs (rings 6, segs 140 scaled by size^0.5). */
internal fun ringFrame(size: Double, t: Double, v: RingVoice): RingFrame {
    val half = size / 2
    val L = v.level
    val lo = v.softBands[0]; val mid = v.softBands[1]; val hi = v.bands[2]
    val T = v.drive
    val P = v.processing
    val voice = 0.5 * L + 0.5 * v.soft
    val R = half * 0.56 * (1 + 0.17 * v.soft) * (1 - 0.06 * P)
    val deform = 1.45
    val field = skinField(0.02 + (0.14 * lo + 0.06 * L) * deform, 0.01 + (0.07 * mid + 0.03 * hi) * deform, T, 1.75)
    val lw = max(0.6, 1.85 * radiusScale(size, 0.5))
    val rings = 6
    val scale = min(1.4, max(0.2, size / 200))
    val segs = max(24, (140 * scale.pow(0.5)).roundToInt())
    val spread = 0.91
    val gap = 0.02

    val dirs = Array(rings) { DoubleArray(segs * 2) }
    val fs = Array(rings) { DoubleArray(segs) }
    val alphas = DoubleArray(rings)
    val rks = DoubleArray(rings)
    var sum = 0.0
    var count = 0
    for (k in 0 until rings) {
        val kd = k.toDouble()
        val odd = k % 2 == 1
        val spinK = 0.16 * (0.5 + hashD(kd, 2.1)) * (if (odd) 1 else -1)
        val lean = max(0.02, spread * (0.2 + 0.8 * hashD(kd, 3.1)) + 0.03 * sin(t * 0.27 + kd * 1.3) + 0.08 * sin(T * 0.35 + kd * 1.7))
        val dir = hashD(kd, 7.7) * TAU + t * 0.05 * (if (odd) 1 else -1) + T * spinK
        val nx = sin(lean) * cos(dir); val ny = sin(lean) * sin(dir); val nz = cos(lean)
        val ul = hypot(nx, nz)
        val ux = nz / ul; val uz = -nx / ul
        val vx = ny * uz; val vy = nz * ux - nx * uz; val vz = -ny * ux
        rks[k] = R * (1 + gap * (kd - (rings - 1) / 2.0))
        val sl = sin(lean)
        var aSum = 0.0
        for (i in 0 until segs) {
            val th = i.toDouble() / segs * TAU
            val c = cos(th); val s = sin(th)
            val x = ux * c + vx * s; val y = vy * s; val z = uz * c + vz * s
            val fi = field(x, y, z)
            dirs[k][2 * i] = x
            dirs[k][2 * i + 1] = y
            fs[k][i] = fi
            sum += fi
            count++
            aSum += 0.62 + 0.38 * clamp01((z / sl + 1) / 2)
        }
        alphas[k] = aSum / segs
    }
    // take out the average (size) and the first harmonic (a slide): bends only
    val mean = if (count > 0) sum / count else 0.0
    var c1 = 0.0; var s1 = 0.0
    for (k in 0 until rings) for (i in 0 until segs) {
        val x = dirs[k][2 * i]; val y = dirs[k][2 * i + 1]
        val l = max(1e-9, hypot(x, y))
        c1 += (fs[k][i] - mean) * (x / l)
        s1 += (fs[k][i] - mean) * (y / l)
    }
    if (count > 0) { c1 = 2 * c1 / count; s1 = 2 * s1 / count }
    val strokes = (0 until rings).map { k ->
        val pts = FloatArray(segs * 2)
        for (i in 0 until segs) {
            val x = dirs[k][2 * i]; val y = dirs[k][2 * i + 1]
            val l = max(1e-9, hypot(x, y))
            val r = rks[k] * (1 + fs[k][i] - mean - c1 * (x / l) - s1 * (y / l))
            pts[2 * i] = (half + x * r).toFloat()
            pts[2 * i + 1] = (half - y * r).toFloat()
        }
        RingStroke(pts, alphas[k].toFloat())
    }
    val specAt = 2.3 + 0.25 * sin(t * 0.21) + 2 * v.spin
    return RingFrame(
        strokes = strokes,
        lineWidth = lw.toFloat(),
        cx = half.toFloat(), cy = half.toFloat(), r = R.toFloat(),
        angle = -PI / 2 - 0.35 + 0.5 * sin(t * 0.07) + 0.4 * sin(T * 0.2) + v.spin,
        intensity = 0.62 + 0.55 * voice + 0.5 * P,
        halo = 0.5 * (1 + 0.6 * P),
        spec = RingSpec((half + cos(specAt) * R).toFloat(), (half + sin(specAt) * R).toFloat(), (R * (0.5 + 0.25 * P)).toFloat(), (0.5 + 0.35 * voice + 0.4 * P).toFloat()),
        fringe = 0.016 + 0.012 * voice,
    )
}

// ---- colour: the ring's spectrum, smoothed in OKLab (iOS LightPainter.palette + smoothSpectrum) ----

private class Rgb(val r: Double, val g: Double, val b: Double)
private class Stop(val offset: Double, val c: Rgb)

private val SPECTRUM = listOf(
    doubleArrayOf(0.0, 255.0, 150.0, 90.0), doubleArrayOf(0.12, 255.0, 110.0, 120.0), doubleArrayOf(0.24, 255.0, 80.0, 170.0),
    doubleArrayOf(0.4, 190.0, 80.0, 255.0), doubleArrayOf(0.58, 90.0, 92.0, 255.0), doubleArrayOf(0.74, 60.0, 132.0, 255.0),
    doubleArrayOf(0.84, 170.0, 90.0, 255.0), doubleArrayOf(0.93, 240.0, 104.0, 165.0), doubleArrayOf(1.0, 255.0, 150.0, 90.0),
).map { Stop(it[0], Rgb(it[1], it[2], it[3])) }

private fun lin(c: Double): Double { val v = c / 255; return if (v <= 0.04045) v / 12.92 else ((v + 0.055) / 1.055).pow(2.4) }
private fun gam(c: Double): Double = 255 * (if (c <= 0.0031308) 12.92 * c else 1.055 * max(0.0, c).pow(1 / 2.4) - 0.055)

private fun toLab(c: Rgb): DoubleArray {
    val r = lin(c.r); val g = lin(c.g); val b = lin(c.b)
    val l = cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
    val m = cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
    val s = cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
    return doubleArrayOf(
        0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
    )
}

private fun fromLab(v: DoubleArray): Rgb {
    val l = (v[0] + 0.3963377774 * v[1] + 0.2158037573 * v[2]).pow(3)
    val m = (v[0] - 0.1055613458 * v[1] - 0.0638541728 * v[2]).pow(3)
    val s = (v[0] - 0.0894841775 * v[1] - 1.291485548 * v[2]).pow(3)
    fun c(x: Double) = max(0.0, min(255.0, gam(x)))
    return Rgb(
        c(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        c(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        c(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
    )
}

/** The ring's colours resampled every 10° along a smooth curve through the stops (iOS smoothSpectrum). */
private fun smoothSpectrum(stops: List<Stop>, samples: Int = 36): List<Stop> {
    val ring = stops.filter { it.offset < 1 }
    val m = ring.size
    if (m < 3) return stops
    val lab = ring.map { toLab(it.c) }
    fun at(i: Int) = ((i % m) + m) % m
    fun pos(i: Int) = ring[at(i)].offset + floor(i.toDouble() / m)
    fun slope(i: Int): DoubleArray {
        val a = lab[at(i + 1)]; val b = lab[at(i - 1)]; val d = pos(i + 1) - pos(i - 1)
        return DoubleArray(3) { (a[it] - b[it]) / d }
    }
    return (0..samples).map { k ->
        val u = k.toDouble() / samples
        var i = -1
        while (i < m - 1 && pos(i + 1) <= u) i++
        val h = pos(i + 1) - pos(i)
        val t = (u - pos(i)) / h; val t2 = t * t; val t3 = t2 * t
        val a = lab[at(i)]; val b = lab[at(i + 1)]; val sa = slope(i); val sb = slope(i + 1)
        val v = DoubleArray(3) { j ->
            (2 * t3 - 3 * t2 + 1) * a[j] + (t3 - 2 * t2 + t) * h * sa[j] + (-2 * t3 + 3 * t2) * b[j] + (t3 - t2) * h * sb[j]
        }
        Stop(u, fromLab(v))
    }
}

/** The smoothed spectrum, per theme (dark keeps full strength, light dims to 0.82 as iOS). */
private val smoothDark: List<Stop> by lazy { smoothSpectrum(SPECTRUM) }
private val smoothLight: List<Stop> by lazy { smoothSpectrum(SPECTRUM.map { Stop(it.offset, Rgb(it.c.r * 0.82, it.c.g * 0.82, it.c.b * 0.82)) }) }

private fun sampleAt(stops: List<Stop>, u: Double): Rgb {
    val n = stops.size - 1
    val x = u * n
    val i = floor(x).toInt().coerceIn(0, n - 1)
    val f = x - i
    val a = stops[i].c; val b = stops[i + 1].c
    return Rgb(a.r + (b.r - a.r) * f, a.g + (b.g - a.g) * f, a.b + (b.b - a.b) * f)
}

/**
 * The sweep gradient's stops, turned so the spectrum starts at [angle] (Compose's sweep starts at +x and runs
 * clockwise, as SwiftUI's conic does). A [tint] reproduces iOS's `.saturation(0).colorMultiply(tint)`.
 */
private fun spectrumStops(angle: Double, dark: Boolean, tint: Color?): Array<Pair<Float, Color>> {
    val src = if (dark) smoothDark else smoothLight
    val n = 36
    val shift = angle / TAU
    return Array(n + 1) { k ->
        val loc = k.toDouble() / n
        var u = (loc - shift) % 1.0
        if (u < 0) u += 1.0
        val c = sampleAt(src, u)
        val color = if (tint != null) {
            val lum = (0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b) / 255
            Color((tint.red * lum).toFloat(), (tint.green * lum).toFloat(), (tint.blue * lum).toFloat())
        } else {
            Color((c.r / 255).toFloat(), (c.g / 255).toFloat(), (c.b / 255).toFloat())
        }
        loc.toFloat() to color
    }
}

private val FRINGE_OUT = Color(120 / 255f, 232 / 255f, 214 / 255f)
private val FRINGE_IN = Color(64 / 255f, 72 / 255f, 1f)

/**
 * iOS paintLight: halo, bloom, chromatic fringes (dark), the crisp core and the specular spot (dark). The caller
 * draws into an offscreen layer so [BlendMode.Plus] adds within the ring's own pixels, as SwiftUI's layer does.
 * The frame is in dp; drawing is scaled to px here.
 */
internal fun DrawScope.paintRing(frame: RingFrame, dark: Boolean, tint: Color?) {
    val paths = frame.strokes.map { s ->
        Path().apply {
            val n = s.pts.size / 2
            for (i in 0 until n) if (i == 0) moveTo(s.pts[0], s.pts[1]) else lineTo(s.pts[2 * i], s.pts[2 * i + 1])
            close()
        }
    }
    val alphas = frame.strokes.map { it.alpha }
    val center = Offset(frame.cx, frame.cy)
    val spectrum = Brush.sweepGradient(*spectrumStops(frame.angle, dark, tint), center = center)
    val blend = if (dark) BlendMode.Plus else BlendMode.SrcOver
    val n = max(1, paths.size).toFloat()
    val I = frame.intensity.toFloat()
    val lw = frame.lineWidth

    fun pass(brush: Brush, width: (Float) -> Float, alpha: (Float) -> Float) {
        for (i in paths.indices) {
            val a = alpha(alphas[i]).coerceIn(0f, 1f)
            if (a <= 0.002f) continue
            drawPath(paths[i], brush, alpha = a, style = Stroke(width(lw), cap = StrokeCap.Round, join = StrokeJoin.Round), blendMode = blend)
        }
    }

    scale(density, density, pivot = Offset.Zero) {
        // 1 · halo: very wide, very faint, shared out between the rings
        val haloK = (frame.halo * I * (if (dark) 1.0 else 0.2) / n).toFloat()
        for ((wk, ak) in listOf(0.7f to 0.045f, 0.42f to 0.075f, 0.24f to 0.11f, 0.12f to 0.16f)) {
            pass(spectrum, { frame.r * wk }, { ak * haloK })
        }
        // 2 · bloom: three soft passes that sum into one luminous band
        val bloomK = I * (if (dark) 1f else 0.6f)
        for ((wk, ak) in listOf(16f to 0.05f, 8f to 0.11f, 4f to 0.2f)) {
            pass(spectrum, { it * wk }, { ak * bloomK * it })
        }
        // 3 · chromatic fringes, scaled about the centre (dark substrates only)
        if (dark && frame.fringe > 0) {
            for ((sc, color, ak) in listOf(Triple((1 + frame.fringe).toFloat(), FRINGE_OUT, 0.16f), Triple((1 - frame.fringe).toFloat(), FRINGE_IN, 0.22f))) {
                scale(sc, sc, pivot = center) {
                    val brush = androidx.compose.ui.graphics.SolidColor(color)
                    for (i in paths.indices) {
                        val a = (ak * I * alphas[i]).coerceIn(0f, 1f)
                        drawPath(paths[i], brush, alpha = a, style = Stroke(lw * 1.4f / sc, cap = StrokeCap.Round, join = StrokeJoin.Round), blendMode = blend)
                    }
                }
            }
        }
        // 4 · the crisp core of every ring
        pass(spectrum, { it }, { 0.7f * it * (0.75f + 0.25f * I) })
        // 5 · specular hot spot
        if (dark) {
            val s = frame.spec
            val shine = Brush.radialGradient(
                0f to Color.White, 0.35f to Color.White.copy(alpha = 0.45f), 1f to Color.White.copy(alpha = 0f),
                center = Offset(s.x, s.y), radius = max(0.5f, s.r),
            )
            pass(shine, { it * 2.2f }, { s.a * it })
        }
    }
}

/** Keeps the frame-to-frame state one speaker needs: the follower and the last frame time. */
internal class RingEngine {
    val follower = RingFollower()
    private var lastNanos = 0L

    fun frame(nanos: Long, sizeDp: Double, state: RingState, level: Double, still: Boolean): RingFrame {
        val dt = if (lastNanos == 0L) 1.0 / 60 else min(0.05, max(0.0, (nanos - lastNanos) / 1e9))
        lastNanos = nanos
        val hears = state == RingState.Listening || state == RingState.Speaking
        if (hears) follower.readLevel(level) else follower.readSilence()
        val v = follower.step(dt, state == RingState.Processing, still)
        return ringFrame(sizeDp, follower.t, v)
    }
}
