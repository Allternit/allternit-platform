package com.allternit.feature.shell.backdrop

import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Shader
import android.graphics.Typeface
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The ASCII backgrounds' engine (iOS AsciiEngine.swift, allternit-ai ascii-engine.ts): each look
 * draws characters into a grid every frame, and one canvas paints the grid.
 */
internal object Ascii {
    const val tau = Math.PI * 2

    /** Swift `.rounded()`: half away from zero. */
    fun round(x: Double): Double = if (x < 0) -floor(-x + 0.5) else floor(x + 0.5)

    fun hash(n: Double): Double {
        var x = floor(n).toLong().toInt()
        x = (x xor 0x9e3779b9L.toInt()) * 0x85ebca6bL.toInt()
        x = x xor (x ushr 13)
        x *= 0xc2b2ae35L.toInt()
        x = x xor (x ushr 16)
        return (x.toLong() and 0xFFFFFFFFL).toDouble() / 4294967296.0
    }

    /** Seeded so a look draws the same layout every time it mounts. */
    fun seeded(seed: Int): () -> Double {
        var a = seed
        return {
            a += 0x6d2b79f5
            var t = (a xor (a ushr 15)) * (1 or a)
            t = (t + ((t xor (t ushr 7)) * (61 or t))) xor t
            ((t xor (t ushr 14)).toLong() and 0xFFFFFFFFL).toDouble() / 4294967296.0
        }
    }

    fun smooth(t: Double): Double = t * t * (3 - 2 * t)

    private fun valueNoise(x: Double, y: Double, s: Double): Double {
        val xi = floor(x)
        val yi = floor(y)
        val u = smooth(x - xi)
        val v = smooth(y - yi)
        fun h(a: Double, b: Double): Double = hash(a * 7919 + b * 104729 + s * 15485863)
        return (h(xi, yi) * (1 - u) + h(xi + 1, yi) * u) * (1 - v) +
            (h(xi, yi + 1) * (1 - u) + h(xi + 1, yi + 1) * u) * v
    }

    /** Three octaves of value noise, 0..1. */
    fun fbm(x: Double, y: Double, s: Double = 0.0): Double =
        valueNoise(x, y, s) * 0.55 + valueNoise(x * 2.1, y * 2.1, s + 1) * 0.3 + valueNoise(x * 4.3, y * 4.3, s + 2) * 0.15

    /** How many of something to draw at this density, never fewer than [minimum]. */
    fun count(n: Double, density: Double, minimum: Int = 2): Int = max(minimum, round(n * density).toInt())

    /** JS `%`: the remainder keeps the dividend's sign. */
    fun mod(a: Double, b: Double): Double = a % b

    /** Always positive. */
    fun wrap(a: Double, b: Double): Double = ((a % b) + b) % b

    fun line(g: AsciiGrid, x0: Double, y0: Double, x1: Double, y1: Double, ch: Char, col: String, a: Double) {
        val n = max(abs(x1 - x0), abs(y1 - y0)).toInt()
        if (n <= 1) return
        for (i in 1 until n) {
            val f = i.toDouble() / n.toDouble()
            g.put(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f, ch, col, a)
        }
    }

    class Star(val x: Double, val y: Double, val p: Double)

    fun stars(rand: () -> Double, c: Int, r: Double, n: Double, top: Double = 1.0): List<Star> {
        val count = max(4, round(n).toInt())
        return (0 until count).map {
            val x = floor(rand() * c.toDouble())
            val y = floor(rand() * r * top)
            val p = rand() * tau
            Star(x, y, p)
        }
    }

    fun drawStars(g: AsciiGrid, list: List<Star>, t: Double, col: String) {
        for (s in list) g.put(s.x, s.y, '.', col, 0.35 + 0.35 * sin(t * 1.5 + s.p))
    }
}

/** The character grid a look draws into. A space with tiles on renders as a solid block. */
internal class AsciiGrid(val c: Int, val r: Int) {
    val ch = CharArray(c * r)
    val col = Array(c * r) { "" }
    val a = DoubleArray(c * r)

    fun clear() {
        ch.fill(NONE)
    }

    fun put(x: Double, y: Double, char: Char, colour: String, alpha: Double = 1.0) {
        if (!x.isFinite() || !y.isFinite()) return
        val xi = floor(x + 0.5).toInt()
        val yi = floor(y + 0.5).toInt()
        if (xi < 0 || yi < 0 || xi >= c || yi >= r || alpha <= 0.02) return
        val i = yi * c + xi
        ch[i] = char
        col[i] = colour
        a[i] = min(1.0, alpha)
    }

    fun erase(x: Double, y: Double) {
        val xi = floor(x + 0.5).toInt()
        val yi = floor(y + 0.5).toInt()
        if (xi >= 0 && yi >= 0 && xi < c && yi < r) ch[yi * c + xi] = NONE
    }

    fun text(x: Double, y: Double, s: String, colour: String, alpha: Double = 1.0) {
        for ((i, char) in s.withIndex()) if (char != ' ') put(x + i.toDouble(), y, char, colour, alpha)
    }

    fun sprite(x: Double, y: Double, lines: List<String>, colour: String, alpha: Double = 1.0) {
        for ((j, l) in lines.withIndex()) text(x, y + j.toDouble(), l, colour, alpha)
    }

    /** The character at cell [i], or [NONE] (the lake looks mirror what's above). */
    fun chAt(i: Int): Char = if (i >= 0 && i < ch.size) ch[i] else NONE

    companion object {
        const val NONE = '\u0000'
    }
}

/** The finger in grid cells: s fades 0..1 as it lands and lifts; wt/wx/wy are the last tap. */
internal class AsciiPointer {
    var x = -99.0
    var y = -99.0
    var s = 0.0
    var wt = -99.0
    var wx = -99.0
    var wy = -99.0
}

internal typealias AsciiDraw = (AsciiGrid, Double, AsciiPointer) -> Unit

internal class AsciiScene(
    val id: String,
    val name: String,
    val note: String,
    /** Background gradient, top to bottom. */
    val bg: List<String>,
    /** Characters on their own tiles, as designed. */
    val tile: Boolean = false,
    /** Spaces draw as seamless blocks (the A-block mark). */
    val solid: Boolean = false,
    /** Handles the pointer itself; skip the shared push and pulse. */
    val own: Boolean = false,
    val make: (c: Int, r: Int, rand: () -> Double, density: Double) -> AsciiDraw,
)

// MARK: - Colour

/** Parses "#rgb" / "#rrggbb" to an opaque ARGB int (0 when it can't). */
internal fun parseHexInt(hex: String): Int {
    var value = hex.trim()
    if (value.startsWith("#")) value = value.substring(1)
    if (value.length == 3) value = value.map { "$it$it" }.joinToString("")
    val n = value.take(6).toIntOrNull(16) ?: 0
    return (0xFF shl 24) or n
}

/** Hex colours as HSL, so a look can be recoloured (Bot's colour, Mono) or have its lightness flipped. */
internal class AsciiHSL(var h: Double, var s: Double, var l: Double) {
    fun copy(): AsciiHSL = AsciiHSL(h, s, l)

    /** HSL to an opaque ARGB int. */
    fun toColorInt(): Int {
        val q = if (l < 0.5) l * (1 + s) else l + s - l * s
        val p = 2 * l - q
        fun hue(tIn: Double): Double {
            var t = tIn
            if (t < 0) t += 1
            if (t > 1) t -= 1
            return when {
                t < 1.0 / 6 -> p + (q - p) * 6 * t
                t < 0.5 -> q
                t < 2.0 / 3 -> p + (q - p) * (2.0 / 3 - t) * 6
                else -> p
            }
        }
        val r = if (s == 0.0) l else hue(h + 1.0 / 3)
        val g = if (s == 0.0) l else hue(h)
        val b = if (s == 0.0) l else hue(h - 1.0 / 3)
        fun ch(v: Double): Int = (v * 255).roundToInt().coerceIn(0, 255)
        return (0xFF shl 24) or (ch(r) shl 16) or (ch(g) shl 8) or ch(b)
    }

    companion object {
        fun fromHex(hex: String): AsciiHSL {
            val n = parseHexInt(hex)
            val r = ((n shr 16) and 255) / 255.0
            val g = ((n shr 8) and 255) / 255.0
            val b = (n and 255) / 255.0
            val mx = max(r, max(g, b))
            val mn = min(r, min(g, b))
            val l = (mx + mn) / 2
            var h = 0.0
            var s = 0.0
            if (mx != mn) {
                val d = mx - mn
                s = if (l > 0.5) d / (2 - mx - mn) else d / (mx + mn)
                h = when (mx) {
                    r -> (g - b) / d + (if (g < b) 6 else 0)
                    g -> (b - r) / d + 2
                    else -> (r - g) / d + 4
                }
                h /= 6
            }
            return AsciiHSL(h, s, l)
        }
    }
}

internal enum class AsciiColourMode(val raw: String) {
    Scene("scene"), Accent("accent"), Mono("mono");

    companion object {
        fun from(raw: String?): AsciiColourMode? = entries.firstOrNull { it.raw == raw }
    }
}

internal enum class AsciiCharMode(val raw: String) {
    Design("design"), Text("text"), Tiles("tiles");

    companion object {
        fun from(raw: String?): AsciiCharMode? = entries.firstOrNull { it.raw == raw }
    }
}

internal data class AsciiRenderOptions(
    val fontPx: Double,
    val density: Double,
    val colour: AsciiColourMode,
    val chars: AsciiCharMode,
    /** The screen's colour for Bot's colour mode (hex). */
    val accent: String,
    /** The app is dark. A look whose sky is the other tone is drawn with its lightness flipped. */
    val dark: Boolean,
)

/** One look on one canvas: builds the grid for a size and paints frames. */
internal class AsciiRenderer(
    val scene: AsciiScene,
    val options: AsciiRenderOptions,
    /** Pixels per dp, so the font is options.fontPx dp. */
    private val dp: Float,
) {
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = Typeface.MONOSPACE
        textSize = (options.fontPx * dp).toFloat()
    }
    private val fillPaint = Paint()
    private val bgPaint = Paint()
    private val one = CharArray(1)
    private val colours = HashMap<String, Int>()
    private val accentHSL = AsciiHSL.fromHex(options.accent)
    private val flip: Boolean
    private var grid: AsciiGrid? = null
    private var draw: AsciiDraw? = null
    private var lastW = 0f
    private var lastH = 0f

    val cw: Double
    val chh: Double
    private val baseline: Float

    var pointer = AsciiPointer()
    private var targetX = -99.0
    private var targetY = -99.0
    private var targetInside = false

    init {
        val ls = scene.bg.map { AsciiHSL.fromHex(it).l }
        val sceneDark = ls.sum() / max(1, ls.size).toDouble() < 0.5
        flip = sceneDark != options.dark
        var w = textPaint.measureText("M").toDouble()
        if (w <= 0) w = options.fontPx * dp * 0.6
        cw = w
        chh = options.fontPx * dp * 1.2
        baseline = -textPaint.fontMetrics.ascent
    }

    private fun fit(w: Float, h: Float) {
        if ((w == lastW && h == lastH) || w <= 0f || h <= 0f) return
        lastW = w
        lastH = h
        val c = ceil(w.toDouble() / cw).toInt()
        val r = ceil(h.toDouble() / chh).toInt()
        grid = AsciiGrid(c, r)
        var seed = 7
        for (ch in scene.id) seed = seed * 31 + ch.code
        draw = scene.make(c, r, Ascii.seeded(seed), options.density)
        val stops = scene.bg.map { tc(it) }.let { if (it.size > 1) it else listOf(it.firstOrNull() ?: 0xFF000000.toInt(), it.firstOrNull() ?: 0xFF000000.toInt()) }
        bgPaint.shader = LinearGradient(0f, 0f, 0f, h, stops.toIntArray(), null, Shader.TileMode.CLAMP)
    }

    /** The look's colour (opaque ARGB) for a hex, recoloured and flipped as the options ask. */
    fun tc(col: String): Int {
        colours[col]?.let { return it }
        val hsl = AsciiHSL.fromHex(col)
        when (options.colour) {
            AsciiColourMode.Scene -> Unit
            AsciiColourMode.Mono -> hsl.s = 0.0
            AsciiColourMode.Accent -> {
                if (hsl.s >= 0.06) hsl.s = min(1.0, accentHSL.s * 0.85)
                hsl.h = accentHSL.h
            }
        }
        if (flip) hsl.l = 1 - hsl.l
        val value = if (options.colour == AsciiColourMode.Scene && !flip) parseHexInt(col) else hsl.toColorInt()
        colours[col] = value
        return value
    }

    /** The finger in px relative to the canvas, or null when it lifts. */
    fun pointerAt(px: Double?, py: Double?) {
        if (px == null || py == null) {
            targetInside = false
            return
        }
        targetX = px / cw
        targetY = py / chh
        if (!targetInside && pointer.s < 0.01) {
            pointer.x = targetX
            pointer.y = targetY
        }
        targetInside = true
    }

    fun pulse(px: Double, py: Double, t: Double) {
        pointerAt(px, py)
        pointer.wt = t
        pointer.wx = px / cw
        pointer.wy = py / chh
    }

    /** Eases the pointer toward the finger. */
    fun stepPointer() {
        pointer.x += (targetX - pointer.x) * 0.3
        pointer.y += (targetY - pointer.y) * 0.3
        pointer.s += ((if (targetInside) 1.0 else 0.0) - pointer.s) * 0.18
    }

    private fun alpha255(a: Double): Int = (a * 255).roundToInt().coerceIn(0, 255)

    fun paint(canvas: Canvas, w: Float, h: Float, t: Double) {
        fit(w, h)
        val g = grid ?: return
        val drawFrame = draw ?: return
        canvas.drawRect(0f, 0f, w, h, bgPaint)
        g.clear()
        drawFrame(g, t, pointer)
        val tiles = options.chars == AsciiCharMode.Tiles || (options.chars == AsciiCharMode.Design && scene.tile)
        val ink = if (flip) 0xFFFFFFFF.toInt() else 0xFF000000.toInt()
        val pt = pointer
        val push = pt.s > 0.01 && !scene.own
        val age = t - pt.wt
        val wave = age >= 0 && age < PULSE_SECONDS && !scene.own
        val asp = chh / cw
        for (y in 0 until g.r) {
            for (x in 0 until g.c) {
                val i = y * g.c + x
                val char = g.ch[i]
                if (char == AsciiGrid.NONE) continue
                var a = g.a[i]
                val colKey = g.col[i]
                var px = x * cw
                var py = y * chh
                if (push) {
                    val dx = x - pt.x
                    val dy = (y - pt.y) * asp
                    val d = sqrt(dx * dx + dy * dy) + 0.001
                    if (d < PUSH_RADIUS) {
                        val f = (1 - d / PUSH_RADIUS).pow(2) * pt.s * 2.4
                        px += dx / d * f * cw
                        py += dy / d * f * cw
                        a = min(1.0, a + 0.35 * f)
                    }
                }
                if (wave) {
                    val dx = x - pt.wx
                    val dy = (y - pt.wy) * asp
                    val d = sqrt(dx * dx + dy * dy) + 0.001
                    val k = exp(-(d - age * 45).pow(2) / 30) * (1 - age / PULSE_SECONDS)
                    if (k > 0.01) {
                        px += dx / d * k * cw * 2.2
                        py += dy / d * k * cw * 2.2
                        a = min(1.0, a + 0.5 * k)
                    }
                }
                if (tiles && a >= 0.8) {
                    fillPaint.color = tc(colKey)
                    if (scene.solid && char == ' ') {
                        fillPaint.alpha = alpha255(a)
                        val left = floor(px).toFloat()
                        val top = floor(py).toFloat()
                        canvas.drawRect(left, top, left + (ceil(cw) + 1).toFloat(), top + (ceil(chh) + 1).toFloat(), fillPaint)
                        continue
                    }
                    fillPaint.alpha = alpha255(a * 0.92)
                    canvas.drawRect((px + 0.5).toFloat(), (py + 0.5).toFloat(), (px + cw - 0.5).toFloat(), (py + chh - 0.5).toFloat(), fillPaint)
                    if (char != ' ') {
                        textPaint.color = ink
                        textPaint.alpha = alpha255(0.42)
                        one[0] = char
                        canvas.drawText(one, 0, 1, px.toFloat(), py.toFloat() + baseline, textPaint)
                    }
                } else {
                    textPaint.color = tc(colKey)
                    textPaint.alpha = alpha255(a)
                    one[0] = if (char == ' ') '█' else char
                    canvas.drawText(one, 0, 1, px.toFloat(), py.toFloat() + baseline, textPaint)
                }
            }
        }
    }

    private companion object {
        const val PULSE_SECONDS = 1.6
        const val PUSH_RADIUS = 12.0
    }
}
