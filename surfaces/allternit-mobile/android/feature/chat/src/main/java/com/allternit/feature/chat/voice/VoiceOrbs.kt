package com.allternit.feature.chat.voice

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.acos
import kotlin.math.atan2
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
 * The five Motion orbs: Orbit, Sash, Signal, Swell and Pixel. A port of iOS VoiceRingKit's OrbTypes.swift (each
 * orb's geometry and motion, with the src/presets.ts tunings), VoiceCore's Math.swift (hashes, noise, projection,
 * skin field, finalizeFrame), VoiceFollower.swift (the plain-level path with its 16-slot spectrum) and Painter.swift
 * (ink colours). Same numbers as iOS, so the phone shows the same looks.
 *
 * Built for the low-end phones: one engine per speaker keeps every buffer (dot arrays, the z-sort keys, the
 * Fibonacci directions, the stroke paths) and reuses them each frame, so a frame allocates nothing. Dots are drawn
 * one by one in z order (the GPU batches circles), not as one path per colour as iOS does; the colours are the same
 * quantised ink. Signal's lines are batched by quantised opacity into reused paths, as iOS does.
 *
 * As on iOS the speaker passes no mood and no accent, so the orbs paint in plain ink (dark ink on a light theme,
 * light ink on a dark one); a palette multiplies its first colour in (iOS `.saturation(0).colorMultiply`).
 */

private const val TAU = PI * 2
private const val SPECTRUM_SLOTS = 16

private fun clamp01(v: Double) = if (v < 0) 0.0 else if (v > 1) 1.0 else v
private fun lerp(a: Double, b: Double, f: Double) = a + (b - a) * f
private fun smoothstep(e0: Double, e1: Double, x: Double): Double { val t = clamp01((x - e0) / (e1 - e0)); return t * t * (3 - 2 * t) }
private fun hashD(a: Double, b: Double): Double { val h = sin(a * 12.9898 + b * 78.233) * 43758.5453; return h - floor(h) }
private fun hash3(x: Double, y: Double, z: Double): Double { val h = sin(x * 127.1 + y * 311.7 + z * 74.7) * 43758.5453; return h - floor(h) }
private fun radiusScale(size: Double, p: Double) = (size / 200).pow(p)
private fun sinc(a: Double) = if (a < 1e-6) 1.0 else sin(a) / a

/** Value noise on a 3D lattice, in [0, 1] (iOS vnoise3). */
private fun vnoise3(x: Double, y: Double, z: Double): Double {
    val xi = floor(x); val yi = floor(y); val zi = floor(z)
    var fx = x - xi; var fy = y - yi; var fz = z - zi
    fx = fx * fx * (3 - 2 * fx)
    fy = fy * fy * (3 - 2 * fy)
    fz = fz * fz * (3 - 2 * fz)
    val x00 = lerp(hash3(xi, yi, zi), hash3(xi + 1, yi, zi), fx)
    val x10 = lerp(hash3(xi, yi + 1, zi), hash3(xi + 1, yi + 1, zi), fx)
    val x01 = lerp(hash3(xi, yi, zi + 1), hash3(xi + 1, yi, zi + 1), fx)
    val x11 = lerp(hash3(xi, yi + 1, zi + 1), hash3(xi + 1, yi + 1, zi + 1), fx)
    return lerp(lerp(x00, x10, fy), lerp(x01, x11, fy), fz)
}

private val GOLDEN = PI * (3 - sqrt(5.0))

/** Stable directions on a unit sphere (iOS fibDir), into [out] at [at] as x, y, z. */
private fun fibDir(i: Int, n: Int, out: DoubleArray, at: Int) {
    val y = 1 - (2 * (i + 0.5)) / n
    val rad = sqrt(max(0.0, 1 - y * y))
    val a = i * GOLDEN
    out[at] = rad * cos(a); out[at + 1] = y; out[at + 2] = rad * sin(a)
}

/** The five orbs (iOS OrbType). */
internal enum class OrbKind { Orbit, Sash, Signal, Swell, Pixel }

/** The orb a speaker style draws, or null for Ring and our own looks (iOS SpeakerStyle.orbType). */
internal val SpeakerStyle.orbKind: OrbKind?
    get() = when (this) {
        SpeakerStyle.Orbit -> OrbKind.Orbit
        SpeakerStyle.Sash -> OrbKind.Sash
        SpeakerStyle.Signal -> OrbKind.Signal
        SpeakerStyle.Swell -> OrbKind.Swell
        SpeakerStyle.Pixel -> OrbKind.Pixel
        else -> null
    }

/** iOS Projector at scale 1: spin, tilt, orthographic. +z faces the viewer. The result lands in [px], [py], [pz]. */
private class Projector {
    private var st = 0.0; private var ct = 1.0; private var sy = 0.0; private var cyw = 1.0
    private var cx = 0.0; private var cy = 0.0
    var px = 0.0; var py = 0.0; var pz = 0.0

    fun set(yaw: Double, tilt: Double, cx: Double, cy: Double) {
        st = sin(tilt); ct = cos(tilt); sy = sin(yaw); cyw = cos(yaw)
        this.cx = cx; this.cy = cy
    }

    fun project(x: Double, y: Double, z: Double) {
        val x1 = x * cyw + z * sy
        val z1 = -x * sy + z * cyw
        val y1 = y * ct - z1 * st
        val z2 = y * st + z1 * ct
        px = cx + x1; py = cy - y1; pz = z2
    }
}

/** iOS skinField at radius 1: the shared bulge field, its average over the sphere taken out so it only reshapes. */
private class SkinField {
    private var aLobe = 0.0; private var aFine = 0.0
    private var p1 = 0.0; private var p2 = 0.0; private var p3 = 0.0; private var p4 = 0.0
    private var mean = 0.0

    fun set(aLobe: Double, aFine: Double, T: Double) {
        this.aLobe = aLobe; this.aFine = aFine
        p1 = 0.9 * T; p2 = -0.7 * T + 1.3; p3 = 2.1 * T; p4 = -1.6 * T
        mean = aLobe * (0.55 * sin(p1) * sinc(K_LOBE1) + 0.45 * sin(p2) * sinc(K_LOBE2)) +
            aFine * 0.5 * (cos(p3 - p4) * sinc(K_FINE_DIFF) - cos(p3 + p4) * sinc(K_FINE_SUM))
    }

    fun at(x: Double, y: Double, z: Double): Double =
        aLobe * (0.55 * sin(2.1 * x + 1.3 * y + p1) + 0.45 * sin(1.7 * z - 1.9 * y + p2)) +
            aFine * sin(4.3 * x - 3.1 * z + p3) * sin(3.7 * y + 2.9 * x + p4) - mean

    private companion object {
        val K_LOBE1 = hypot(2.1, 1.3)
        val K_LOBE2 = hypot(1.9, 1.7)
        val K_FINE_DIFF = sqrt((4.3 - 2.9).pow(2) + 3.7 * 3.7 + 3.1 * 3.1)
        val K_FINE_SUM = sqrt((4.3 + 2.9).pow(2) + 3.7 * 3.7 + 3.1 * 3.1)
    }
}

/** What the voice is doing at one instant (iOS VoiceInput, the fields the orbs read). */
internal class OrbVoice {
    var level = 0.0
    val bands = DoubleArray(3)
    var soft = 0.0
    val softBands = DoubleArray(3)
    val spectrum = DoubleArray(SPECTRUM_SLOTS)
    var drive = 0.0
    var processing = 0.0
    var spin = 0.0
}

/**
 * iOS VoiceFollower on its plain-level path (readLevel / readSilence / step), the 16-slot spectrum included (Pixel
 * lights its rings from it). Voice mode's style: sensitivity 0.9, threshold 0.04, release 0.32, idle 0.1, calm 0.4;
 * each orb sets its own speed, drive gain, attack and gain ([configure]).
 */
private class OrbFollower {
    var clock = 0.0
        private set
    var t = 0.0
        private set
    private var drive = 0.0
    private var level = 0.0
    private val bands = DoubleArray(3)
    private val spectrum = DoubleArray(SPECTRUM_SLOTS)
    private var heard = 0.0
    private var proc = 0.0
    private var spin = 0.0
    private var rawLevel = 0.0
    private val rawBands = DoubleArray(3)
    private val rawSpectrum = DoubleArray(SPECTRUM_SLOTS)
    val out = OrbVoice()

    private val threshold = 0.04
    private val release = 0.32
    private val idle = 0.1
    private val calm = 0.4
    private var sensitivity = 0.9
    private var attack = 0.06
    private var rate = 1.0
    private var driveGain = 1.0

    /** iOS OrbView.preset per type: speed, driveGain (× voiceSpeed 1), attack and gain (× voice mode's 0.9). */
    fun configure(kind: OrbKind) {
        when (kind) {
            OrbKind.Orbit -> { rate = 1.05; driveGain = 0.12; attack = 0.06; sensitivity = 0.9 }
            OrbKind.Sash -> { rate = 1.5; driveGain = 0.8; attack = 0.06; sensitivity = 0.9 * 1.1 }
            OrbKind.Signal -> { rate = 1.6; driveGain = 2.0; attack = 0.06; sensitivity = 0.9 }
            OrbKind.Swell -> { rate = 1.4; driveGain = 2.4; attack = 0.06; sensitivity = 0.9 }
            OrbKind.Pixel -> { rate = 1.2; driveGain = 1.6; attack = 0.16; sensitivity = 0.9 }
        }
    }

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
        val t = clock
        rawLevel = raw
        rawBands[0] = raw
        rawBands[1] = raw * (0.72 + 0.28 * sin(t * 2.3))
        rawBands[2] = raw * (0.6 + 0.4 * sin(t * 3.7 + 2))
        val c1 = 0.28 + 0.1 * sin(t * 2.3) + 0.05 * sin(t * 5.1)
        val c2 = 0.64 + 0.1 * sin(t * 1.7 + 1)
        for (k in 0 until SPECTRUM_SLOTS) {
            val f = k.toDouble() / (SPECTRUM_SLOTS - 1)
            val d1 = f - c1; val d2 = f - c2
            val hump = exp(-(d1 * d1) / 0.012) + 0.6 * exp(-(d2 * d2) / 0.008)
            rawSpectrum[k] = raw * (0.25 + 0.75 * hump) * (1 - 0.4 * f)
        }
    }

    fun readSilence() {
        rawLevel = 0.0
        rawBands.fill(0.0)
        rawSpectrum.fill(0.0)
    }

    fun step(dt: Double, processing: Boolean, still: Boolean): OrbVoice {
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
        for (k in 0 until SPECTRUM_SLOTS) spectrum[k] = follow(spectrum[k], shape(rawSpectrum[k], threshold * 0.6), dt, attack, release)
        val breathe = if (still) 0.5 else 0.5 + 0.5 * sin(TAU * clock / 6)
        val lift = idle * breathe * (1 - p)
        out.level = level + (1 - level) * lift
        for (b in 0 until 3) out.bands[b] = bands[b] + (1 - bands[b]) * lift * 0.6
        out.soft = follow(out.soft, out.level, dt, 0.35, 0.9)
        for (b in 0 until 3) out.softBands[b] = follow(out.softBands[b], out.bands[b], dt, 0.35, 0.9)
        spectrum.copyInto(out.spectrum)
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

/**
 * One rendered instant, reused frame to frame (iOS OrbFrame): dots in dp as parallel arrays, drawn in [order]
 * (far → near); Signal's lines as flat point and opacity arrays.
 */
internal class OrbFrame {
    var count = 0
        private set
    var x = FloatArray(0)
        private set
    var y = FloatArray(0)
        private set
    var z = FloatArray(0)
        private set
    var r = FloatArray(0)
        private set
    var white = FloatArray(0)
        private set
    var a = FloatArray(0)
        private set
    private var keys = LongArray(0)
    /** Dot indices, z-sorted, [visible] long, after [finish]. */
    var order = IntArray(0)
        private set
    /** Paint the dots as squares, `r` their half side (Pixel). */
    var square = false

    var strokeCount = 0
        private set
    /** Points per stroke (each stroke holds 2 × that many coordinates). */
    var strokeLen = 0
        private set
    var strokeWidth = 0f
    var strokePts: Array<FloatArray> = emptyArray()
        private set
    var strokeAlpha: Array<FloatArray> = emptyArray()
        private set

    fun begin(dotCapacity: Int) {
        count = 0
        strokeCount = 0
        square = false
        if (x.size < dotCapacity) grow(dotCapacity)
    }

    /** Bigger buffers, keeping the dots already written (only when an orb grows between frames). */
    private fun grow(capacity: Int) {
        val n = max(capacity, x.size * 3 / 2)
        x = x.copyOf(n); y = y.copyOf(n); z = z.copyOf(n); r = r.copyOf(n)
        white = white.copyOf(n); a = a.copyOf(n); keys = LongArray(n); order = IntArray(n)
    }

    fun dot(dx: Double, dy: Double, dz: Double, dr: Double, dw: Double, da: Double = 1.0) {
        if (count == x.size) grow(count + 64)
        val i = count++
        x[i] = dx.toFloat(); y[i] = dy.toFloat(); z[i] = dz.toFloat(); r[i] = dr.toFloat()
        white[i] = dw.toFloat(); a[i] = da.toFloat()
    }

    /** Room for [strokes] closed strokes of [len] points each, filled through [strokePts] and [strokeAlpha]. */
    fun beginStrokes(strokes: Int, len: Int, width: Float) {
        if (strokePts.size < strokes || strokeLen != len) {
            strokePts = Array(max(strokes, strokePts.size)) { FloatArray(len * 2) }
            strokeAlpha = Array(max(strokes, strokeAlpha.size)) { FloatArray(len) }
        }
        strokeLen = len
        strokeCount = strokes
        strokeWidth = width
    }

    /** iOS finalizeFrame: drop invisible dots (a < 0.02), clamp radii to [rMin], z-sort far → near. */
    fun finish(rMin: Double) {
        val rm = rMin.toFloat()
        var n = 0
        for (i in 0 until count) {
            if (a[i] < 0.02f) continue
            if (r[i] < rm) r[i] = rm
            // a float's bits as an int that orders like the float, then the index: one primitive sort, no boxing
            val b = z[i].toRawBits()
            val k = if (b >= 0) b else b xor 0x7fffffff
            keys[n++] = (k.toLong() shl 32) or (i.toLong() and 0xffffffffL)
        }
        java.util.Arrays.sort(keys, 0, n)
        for (j in 0 until n) order[j] = (keys[j] and 0xffffffffL).toInt()
        visible = n
    }

    /** Dots left after [finish]. */
    var visible = 0
        private set
}

private const val LX = -0.45
private const val LY = 0.55
private const val LZ = 0.7
private val LN = sqrt(LX * LX + LY * LY + LZ * LZ)

/** Keeps the frame-to-frame state one speaker needs: the follower, the buffers and the last frame time. */
internal class OrbEngine {
    private val follower = OrbFollower()
    private val pt = Projector()
    private val skin = SkinField()
    val frame = OrbFrame()
    private var lastNanos = 0L
    private var fib = DoubleArray(0)
    private var fibN = -1
    private var disps = DoubleArray(0)
    private val n0 = DoubleArray(3)

    fun frame(nanos: Long, sizeDp: Double, kind: OrbKind, state: RingState, level: Double, still: Boolean): OrbFrame {
        val dt = if (lastNanos == 0L) 1.0 / 60 else min(0.05, max(0.0, (nanos - lastNanos) / 1e9))
        lastNanos = nanos
        follower.configure(kind)
        val hears = state == RingState.Listening || state == RingState.Speaking
        if (hears) follower.readLevel(level) else follower.readSilence()
        val v = follower.step(dt, state == RingState.Processing, still)
        val t = follower.t
        when (kind) {
            OrbKind.Orbit -> orbit(sizeDp, t, v)
            OrbKind.Sash -> sash(sizeDp, t, v)
            OrbKind.Signal -> signal(sizeDp, t, v)
            OrbKind.Swell -> swell(sizeDp, t, v)
            OrbKind.Pixel -> pixel(sizeDp, t, v)
        }
        return frame
    }

    /** iOS ResolvedPreset.resolve: a count knob scaled by size (dots 1), rounded, at least 1. */
    private fun count(size: Double, value: Double, exp: Double): Double {
        val scale = min(1.4, max(0.2, size / 200))
        return max(1.0, (value * scale.pow(exp)).roundToInt().toDouble())
    }

    /** The Fibonacci directions for [n] points, cached until n changes. */
    private fun fibFor(n: Int): DoubleArray {
        if (fibN != n) {
            if (fib.size < n * 3) fib = DoubleArray(n * 3)
            for (i in 0 until n) fibDir(i, n, fib, i * 3)
            fibN = n
        }
        return fib
    }

    // ---- orbit.ts: particles on tilted orbits; a voice pulls them in ----
    private fun orbit(size: Double, t: Double, v: OrbVoice) {
        val cx = size / 2; val cy = size / 2
        val voice = 0.5 * v.level + 0.5 * v.soft
        val zoom = (1 + 0.49 * voice) * (1 - 0.14 * v.processing)
        val R = (size / 2) * 0.56
        val persp = 0.25
        pt.set(t * 0.12 + v.drive * 0.03, 0.3, 0.0, 0.0)
        val rs = radiusScale(size, 0.9)
        val orbitN = 12; val particles = 6
        frame.begin(orbitN * particles)
        for (orb in 0 until orbitN) {
            val od = orb.toDouble()
            val h1 = hashD(od, 1.7); val h2 = hashD(od, 5.2); val h3 = hashD(od, 8.9)
            val ro = R * (0.45 + 0.52 * h1)
            val th = h1 * TAU
            val phi = acos(2 * h2 - 1)
            val nx = sin(phi) * cos(th); val ny = cos(phi); val nz = sin(phi) * sin(th)
            var ux = -ny; var uy = nx
            val ul = max(1e-6, sqrt(ux * ux + uy * uy))
            ux /= ul; uy /= ul
            val vx = -nz * uy; val vy = nz * ux; val vz = nx * uy - ny * ux
            val speed = (0.25 + 0.55 * h3) * (if (h3 > 0.5) 1 else -1)
            for (m in 0 until particles) {
                val a = v.drive * speed + m.toDouble() / particles * TAU + h2 * 6
                val ca = cos(a); val sa = sin(a)
                pt.project((ux * ca + vx * sa) * ro, (uy * ca + vy * sa) * ro, vz * sa * ro)
                val x = pt.px; val y = pt.py; val z = pt.pz
                val depth = clamp01((z / R + 1) / 2)
                val k = zoom / (1 - (persp * z) / R)
                frame.dot(cx + x * k, cy + y * k, z, (0.47 + 0.65 * depth) * rs * k, 0.32 - 0.24 * depth)
            }
        }
        frame.finish(0.3)
    }

    // ---- sash.ts: a band of dotted lanes round a ghost sphere; a voice deepens the wave ----
    private fun sash(size: Double, t: Double, v: OrbVoice) {
        val cx = size / 2; val cy = size / 2
        val L = v.soft
        val lo = v.softBands[0]; val mid = v.softBands[1]
        val R = (size / 2) * 0.66 * (1 + 0.21 * v.level)
        skin.set(0.015 + (0.1 * lo + 0.05 * v.level), 0.04 * mid, v.drive)
        val ta = 0.55 + 0.05 * sin(t * 0.37)
        val vy = cos(ta); val vz = sin(ta)
        var dc = 0.0
        for (i in 0 until 48) {
            val a = i.toDouble() / 48 * TAU
            dc += skin.at(cos(a), sin(a) * vy, sin(a) * vz)
        }
        dc /= 48
        pt.set(0.0, 0.3, cx, cy)
        val rs = radiusScale(size, 0.6)
        val T = v.drive
        val ghostN = count(size, 60.0, 0.8).toInt()
        val lanes = max(1, 10)
        val segs = max(8, count(size, 56.0, 0.45).roundToInt())
        frame.begin(ghostN + lanes * segs)
        val dirs = fibFor(ghostN)
        for (i in 0 until ghostN) {
            val dx = dirs[i * 3]; val dy = dirs[i * 3 + 1]; val dz = dirs[i * 3 + 2]
            val rr = R * (1 + skin.at(dx, dy, dz) - dc)
            pt.project(dx * rr, dy * rr, dz * rr)
            val depth = (pt.pz / R + 1) / 2
            frame.dot(pt.px, pt.py, pt.pz, 0.65 * rs, 0.78, 0.1 + 0.22 * depth)
        }
        val ny = -vz; val nz = vy
        val P = v.processing
        val laneGap = 0.062 * (1 + 0.25 * L) * (1 - 0.4 * P)
        val pulseAt = T * 1.6
        val aLong = 0.15 * (0.55 + 0.9 * lo) * (1 - 0.6 * P)
        val aShort = 0.06 * (0.5 + 1.2 * mid)
        val aSwell = 0.05 + 0.05 * L
        val half = (lanes - 1) / 2.0
        val swellPhase = t * 0.31 + sin(t * 0.17) * 1.5
        for (w in 0 until lanes) {
            val wd = w.toDouble()
            val laneOff = (wd - half) * laneGap
            val edge = abs(wd - half) / max(1.0, half)
            for (k in 0 until segs) {
                val a = k.toDouble() / segs * TAU
                val wob = aLong * sin(a * 3 - T * 0.95 + wd * 0.18) + aShort * sin(a * 5 + T * 0.6) + aSwell * sin(a * 2 + swellPhase)
                val off = laneOff + wob
                val x = cos(a)
                val y = sin(a) * vy + ny * off
                val z = sin(a) * vz + nz * off
                val l = sqrt(x * x + y * y + z * z)
                val rr = (R * (1 + skin.at(x / l, y / l, z / l) - dc)) / l
                pt.project(x * rr, y * rr, z * rr)
                val zr = pt.pz
                val depth = (zr / R + 1) / 2
                val crest = max(0.0, wob) * L
                val da = atan2(sin(a - pulseAt), cos(a - pulseAt))
                val pulse = exp(-(da * da) / 0.18) * P
                frame.dot(
                    pt.px, pt.py, zr,
                    (0.49 + 0.78 * depth) * (1 - 0.25 * edge) * (1 + 1.2 * crest + 0.5 * pulse) * rs,
                    0.52 - 0.44 * depth + 0.18 * edge - 0.3 * pulse,
                    0.4 + 0.6 * depth,
                )
            }
        }
        frame.finish(0.3)
    }

    // ---- signal.ts: a wireframe sphere of great circles that a voice bends like a skin ----
    private fun signal(size: Double, t: Double, v: OrbVoice) {
        val half = size / 2
        val L = v.level
        val lo = v.softBands[0]; val mid = v.softBands[1]
        val hi = v.bands[2]
        val T = v.drive
        val R = half * 0.6 * (1 + 0.06 * v.soft) * (1 - 0.08 * v.processing)
        pt.set(0.42 + 0.08 * sin(t * 0.23), 0.32, half, half)
        val lw = max(0.4, 0.6 * radiusScale(size, 0.5))
        skin.set(0.02 + (0.14 * lo + 0.06 * L), 0.01 + (0.07 * mid + 0.03 * hi), T)
        val segs = max(24, count(size, 96.0, 0.5).roundToInt())
        val limbs = 2
        val circles = max(2, count(size, 8.0, 0.5).roundToInt())
        frame.begin(0)
        frame.beginStrokes(limbs + circles, segs, lw.toFloat())
        for (k in 0 until limbs) {
            val kd = k.toDouble()
            val lean = 0.1 + 0.05 * sin(t * 0.31 + kd * 2.1)
            val dir = t * 0.17 * (if (k % 2 == 1) 1 else -1) + kd * 2.4
            val nx = sin(lean) * cos(dir); val ny = sin(lean) * sin(dir); val nz = cos(lean)
            val ul = hypot(nx, nz)
            val ux = nz / ul; val uz = -nx / ul
            val vx = ny * uz; val vy = nz * ux - nx * uz; val vz = -ny * ux
            val sl = sin(lean)
            val pts = frame.strokePts[k]; val al = frame.strokeAlpha[k]
            for (i in 0 until segs) {
                val th = i.toDouble() / segs * TAU
                val c = cos(th); val s = sin(th)
                val x = ux * c + vx * s; val y = vy * s; val z = uz * c + vz * s
                val r = R * (1 + 0.006 * kd + skin.at(x, y, z))
                pts[2 * i] = (half + x * r).toFloat(); pts[2 * i + 1] = (half - y * r).toFloat()
                al[i] = (0.55 + 0.35 * clamp01((z / sl + 1) / 2)).toFloat()
            }
        }
        for (k in 0 until circles) {
            val kd = k.toDouble()
            fibDir(k, circles, n0, 0)
            val a = T * 0.16 * (0.5 + hashD(kd, 2.1)) * (if (k % 2 == 1) 1 else -1) + hashD(kd, 6.6) * TAU
            val nx = n0[0]
            val ny = n0[1] * cos(a) - n0[2] * sin(a)
            val nz = n0[1] * sin(a) + n0[2] * cos(a)
            val ul = max(1e-6, hypot(nz, nx))
            val ux = nz / ul; val uz = -nx / ul
            val vx = ny * uz; val vy = nz * ux - nx * uz; val vz = -ny * ux
            val pts = frame.strokePts[limbs + k]; val al = frame.strokeAlpha[limbs + k]
            for (i in 0 until segs) {
                val th = i.toDouble() / segs * TAU
                val c = cos(th); val s = sin(th)
                val x = ux * c + vx * s; val y = vy * s; val z = uz * c + vz * s
                val r = R * (1 + skin.at(x, y, z))
                pt.project(x * r, y * r, z * r)
                pts[2 * i] = pt.px.toFloat(); pts[2 * i + 1] = pt.py.toFloat()
                val d = clamp01((pt.pz / R + 1) / 2)
                al[i] = (0.06 + 0.94 * d * d).toFloat()
            }
        }
        frame.finish(0.3)
    }

    // ---- swell.ts: a halftone sphere of drifting clouds ----
    private fun swell(size: Double, t: Double, v: OrbVoice) {
        val half = size / 2
        val L = v.level
        val lo = v.bands[0]; val mid = v.bands[1]; val hi = v.bands[2]
        val T = v.drive
        val P = v.processing
        val R = half * 0.6 * (1 + 0.1 * L) * (1 - 0.08 * P)
        val scanY = 0.95 * sin(t * 1.3)
        pt.set(t * 0.14, 0.32, half, half)
        val rs = radiusScale(size, 0.6)
        val n = max(40, count(size, 900.0, 1.0).roundToInt())
        val freq = 2.3
        val aLobe = 0.018 + (0.12 * lo + 0.04 * L)
        val aFine = 0.045 * mid + 0.03 * hi
        frame.begin(n)
        val dirs = fibFor(n)
        if (disps.size < n) disps = DoubleArray(n)
        var mean = 0.0
        for (i in 0 until n) {
            val dx = dirs[i * 3]; val dy = dirs[i * 3 + 1]; val dz = dirs[i * 3 + 2]
            val lobes = 0.55 * sin(2.1 * dx + 1.3 * dy + 0.9 * T) + 0.45 * sin(1.7 * dz - 1.9 * dy - 0.7 * T + 1.3)
            val fine = sin(5.3 * dx - 4.1 * dz + 2.3 * T) * sin(4.7 * dy + 3.1 * dx - 1.7 * T)
            val disp = aLobe * lobes + aFine * fine
            disps[i] = disp
            mean += disp / n
        }
        val f2 = freq * 2.1
        for (i in 0 until n) {
            val dx = dirs[i * 3]; val dy = dirs[i * 3 + 1]; val dz = dirs[i * 3 + 2]
            val disp = disps[i] - mean
            val rr = R * (1 + disp)
            pt.project(dx * rr, dy * rr, dz * rr)
            val z = pt.pz
            val depth = clamp01((z / R + 1) / 2)
            val c = 0.65 * vnoise3(dx * freq + T * 0.21, dy * freq - T * 0.15, dz * freq + T * 0.1) +
                0.35 * vnoise3(dx * f2 - T * 0.29, dy * f2 + 7.3, dz * f2 + T * 0.24)
            val cloud = smoothstep(0.34, 0.68, c)
            val sd = dy - scanY
            val scan = exp(-(sd * sd) / 0.012) * P
            val bulge = max(0.0, disp) / 0.16
            frame.dot(
                pt.px, pt.py, z,
                (0.27 + 0.84 * depth) * (0.22 + 0.95 * cloud + 0.6 * scan) * (1 + 0.5 * bulge) * rs,
                0.62 - 0.52 * depth - 0.16 * cloud - 0.3 * scan,
                0.3 + 0.7 * depth,
            )
        }
        frame.finish(0.25)
    }

    // ---- pixel.ts: a ball of square pixels, lit, textured, and lit in rings by the voice's spectrum ----
    private fun pixel(size: Double, t: Double, v: OrbVoice) {
        val half = size / 2
        val L = v.level
        val lo = v.bands[0]; val mid = v.bands[1]; val hi = v.bands[2]
        val T = v.drive
        val P = v.processing
        val base = half * 0.72
        val R = base * (1 + 0.3 * L) * (1 - 0.06 * P)
        val rMul = 0.55 // the pixel preset's dotSize
        val n = max(4, count(size, 17.0, 0.5).roundToInt())
        val cell = (2 * base) / n
        val most = cell / 2 - max(0.5, (cell * 0.24) / 2)
        val reach = ceil((R * 1.25) / cell).toInt()
        val yaw = 0.12 * t + 0.1 * T + 0.5 * v.spin
        val sy = sin(yaw); val cyw = cos(yaw)
        val st = sin(0.35); val ct = cos(0.35)
        val shade = 0.7; val tex = 0.35; val fade = 1.3; val strength = 1.38; val deform = 1.7
        val aLobe = 0.012 + (0.09 * lo + 0.03 * L) * deform
        val aFine = (0.035 * mid + 0.025 * hi) * deform
        val scanY = 0.95 * sin(t * 1.3)
        val spectrum = v.spectrum
        val slots = spectrum.size
        val side = 2 * reach + 1
        frame.begin(side * side)
        for (j in -reach..reach) {
            for (i in -reach..reach) {
                val x = half + i * cell
                val y = half + j * cell
                val nx = (x - half) / R
                val ny = -(y - half) / R
                val ang = atan2(ny, nx)
                val bulge = aLobe * (0.55 * sin(2 * ang + 0.9 * T) + 0.45 * sin(3 * ang - 0.7 * T + 1.3)) +
                    aFine * sin(7 * ang + 2.3 * T) * sin(5 * ang - 1.7 * T)
                val rho = hypot(nx, ny) / (1 + bulge)
                if (rho >= 1) continue
                val k = 1 / (1 + bulge)
                val vx = nx * k; val vy = ny * k
                val vz = sqrt(max(0.0, 1 - vx * vx - vy * vy))
                val lambert = clamp01((vx * LX + vy * LY + vz * LZ) / LN)
                val lit = 1 - shade + shade * lambert
                val gy = vy * ct + vz * st
                val z1 = -vy * st + vz * ct
                val gx = vx * cyw - z1 * sy
                val gz = vx * sy + z1 * cyw
                val c = 0.65 * vnoise3(gx * 2.2 + T * 0.15, gy * 2.2, gz * 2.2 - T * 0.1) +
                    0.35 * vnoise3(gx * 4.6 - T * 0.2, gy * 4.6 + 7.3, gz * 4.6)
                val surface = 1 - tex + tex * smoothstep(0.32, 0.68, c)
                val ring = if (slots > 0) spectrum[min(slots - 1, floor(rho * slots).toInt())] else L
                val voice = 0.35 * L + 0.85 * ring
                val sd = vy - scanY
                val scan = exp(-(sd * sd) / 0.01) * P
                val edge = 1 - 0.4 * fade * (1 - vz)
                val fill = (0.35 + 0.75 * lit * surface + 0.3 * voice + 0.2 * scan) * edge * rMul
                val ink = 0.22 + 0.62 * lit * surface + 0.22 * voice + 0.3 * scan
                frame.dot(
                    x, y, vz * R, most * min(1.0, fill),
                    clamp01(1 - ink * strength),
                    clamp01((1 - 0.75 * fade * (1 - sqrt(vz))) * strength),
                )
            }
        }
        frame.finish(0.4)
        frame.square = true
    }
}

// ---- painting (iOS Painter.swift, ink frames) ----

/** One ink value to a packed colour: grey on the depth ramp, times the palette's [tint] when there is one. */
private fun inkArgb(w: Double, alpha: Double, dark: Boolean, tint: Color?): Int {
    val g = (if (dark) 1 - w else w)
    var r = g; var gr = g; var b = g; var al = alpha
    if (tint != null) {
        r *= tint.red; gr *= tint.green; b *= tint.blue; al *= tint.alpha
    }
    fun q(c: Double) = (clamp01(c) * 255).roundToInt()
    return (q(al) shl 24) or (q(r) shl 16) or (q(gr) shl 8) or q(b)
}

/** Line opacity steps (iOS paintStrokes' alphaSteps). */
private const val ALPHA_STEPS = 24

/** Paints orb frames; keeps one reused path per quantised line opacity (iOS paintStrokes batches by the same key). */
internal class OrbPainter {
    private val paths = Array(ALPHA_STEPS + 1) { Path() }
    private val used = BooleanArray(ALPHA_STEPS + 1)
    private var stroke = Stroke(1f, cap = StrokeCap.Round, join = StrokeJoin.Round)

    /** Paint [frame] (in dp) scaled to px: Signal's lines, then the dots far → near (iOS paintMarks, monochrome). */
    fun paint(scope: DrawScope, frame: OrbFrame, dark: Boolean, tint: Color?) = with(scope) {
        scale(density, density, pivot = Offset.Zero) {
            if (frame.strokeCount > 0) paintStrokes(frame, dark, tint)
            paintDots(frame, dark, tint)
        }
    }

    private fun DrawScope.paintStrokes(frame: OrbFrame, dark: Boolean, tint: Color?) {
        used.fill(false)
        val n = frame.strokeLen
        if (n < 2) return
        for (s in 0 until frame.strokeCount) {
            val pts = frame.strokePts[s]; val al = frame.strokeAlpha[s]
            var key = -1
            // closed: n segments, the last back to the first point
            for (i in 0 until n) {
                val j = if (i + 1 == n) 0 else i + 1
                val q = (clamp01((al[i] + al[j]) / 2.0) * ALPHA_STEPS).roundToInt()
                if (q == 0) { key = -1; continue }
                val path = paths[q]
                if (!used[q]) { path.reset(); used[q] = true }
                if (q != key) { path.moveTo(pts[2 * i], pts[2 * i + 1]); key = q }
                path.lineTo(pts[2 * j], pts[2 * j + 1])
            }
        }
        if (stroke.width != frame.strokeWidth) stroke = Stroke(frame.strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round)
        for (q in 1..ALPHA_STEPS) {
            if (!used[q]) continue
            drawPath(paths[q], Color(inkArgb(0.0, q.toDouble() / ALPHA_STEPS, dark, tint)), style = stroke)
        }
    }

    private fun DrawScope.paintDots(frame: OrbFrame, dark: Boolean, tint: Color?) {
        val order = frame.order
        val square = frame.square
        for (j in 0 until frame.visible) {
            val i = order[j]
            // iOS quantises ink to 48 steps and opacity to 32 so dots batch; the same steps keep the same colours
            val qa = (clamp01(frame.a[i].toDouble()) * 32).roundToInt()
            if (qa == 0) continue
            val qw = (clamp01(frame.white[i].toDouble()) * 48).roundToInt()
            val color = Color(inkArgb(qw / 48.0, qa / 32.0, dark, tint))
            val r = frame.r[i]
            if (square) drawRect(color, topLeft = Offset(frame.x[i] - r, frame.y[i] - r), size = Size(r * 2, r * 2))
            else drawCircle(color, radius = r, center = Offset(frame.x[i], frame.y[i]))
        }
    }
}
