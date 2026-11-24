package com.allternit.feature.shell.backdrop

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.ceil
import kotlin.math.cos
import kotlin.math.exp
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin

private const val TAU = Ascii.tau

private fun chr(s: String, i: Int): Char = s[max(0, min(s.length - 1, i))]

private fun rnd(x: Double): Double = Ascii.round(x)

private fun d(i: Int): Double = i.toDouble()

/**
 * The twenty ASCII looks (iOS AsciiScenes.swift, allternit-ai ascii-scenes.ts): one Allternit look
 * (the A-block mark), six moods and thirteen scenes. Each `make` sets up a look for a grid of
 * c x r cells and returns the per-frame draw. Random draws keep the iOS order so a look lays out
 * the same on both phones.
 */
internal object AsciiScenes {
    private val aBlock = AsciiScene(
        id = "ascii-a-block", name = "A-block",
        note = "The A-block mark breathes, sweeps, runs and writes; it leans away from your finger",
        bg = listOf("#111214", "#1b1c20"), tile = true, solid = true, own = true,
        make = ::makeABlock,
    )
    private val clay = AsciiScene(
        id = "ascii-clay", name = "Clay Contours",
        note = "Warm paper with terracotta contour lines; calm and bookish",
        bg = listOf("#f7f1e7", "#ede3d3"), make = ::makeClay,
    )
    private val inkBloom = AsciiScene(
        id = "ascii-ink-bloom", name = "Ink Bloom",
        note = "A soft black-and-white cloud breathes in the middle",
        bg = listOf("#fcfcfc", "#eeeeee"), make = ::makeInkBloom,
    )
    private val spectrum = AsciiScene(
        id = "ascii-spectrum", name = "Spectrum",
        note = "Blue to violet to rose ribbons, with sparks that twinkle",
        bg = listOf("#0a0f22", "#151a38"), make = ::makeSpectrum,
    )
    private val eventHorizon = AsciiScene(
        id = "ascii-event-horizon", name = "Event Horizon",
        note = "A black hole in black and white; its disk turns slowly",
        bg = listOf("#000000", "#0b0b0b"), make = ::makeEventHorizon,
    )
    private val lattice = AsciiScene(
        id = "ascii-lattice", name = "Lattice",
        note = "A teal lattice with pulses running along its lines",
        bg = listOf("#0b1416", "#10211f"), make = ::makeLattice,
    )
    private val ember = AsciiScene(
        id = "ascii-ember", name = "Ember Blocks",
        note = "Stacks of warm pixel blocks rise and fall, red through yellow",
        bg = listOf("#15100c", "#221812"), tile = true, make = ::makeEmber,
    )
    private val skyDrift = AsciiScene(
        id = "ascii-sky-drift", name = "Sky Drift",
        note = "Puffy clouds drift across a soft blue sky at two depths",
        bg = listOf("#9fd2ff", "#e8f4ff"), tile = true, make = ::makeSkyDrift,
    )
    private val glyphStars = AsciiScene(
        id = "ascii-glyph-stars", name = "Glyph Stars",
        note = "A:// glyphs drift and link up",
        bg = listOf("#0a0f24", "#1b2143"), make = ::makeGlyphStars,
    )
    private val rainGlass = AsciiScene(
        id = "ascii-rain-glass", name = "Rain on Glass",
        note = "Streaks fall and drops slide down the window, city lights behind",
        bg = listOf("#1c2836", "#4a5e72"), make = ::makeRainGlass,
    )
    private val codeRain = AsciiScene(
        id = "ascii-code-rain", name = "Code Rain",
        note = "Soft streams of code characters, made for Code mode",
        bg = listOf("#05110d", "#0a241d"), make = ::makeCodeRain,
    )
    private val northernLights = AsciiScene(
        id = "ascii-northern-lights", name = "Northern Lights",
        note = "Curtains of light over a still lake that mirrors them",
        bg = listOf("#030712", "#0b1a30"), make = ::makeNorthernLights,
    )
    private val tide = AsciiScene(
        id = "ascii-tide", name = "Tide",
        note = "Sunset over slow waves; birds and a small sailboat pass",
        bg = listOf("#ffb37a", "#ffd6a8", "#6fb9cf"), make = ::makeTide,
    )
    private val lanterns = AsciiScene(
        id = "ascii-lanterns", name = "Lanterns",
        note = "Lanterns rise from a dark lake and glow in its water",
        bg = listOf("#0f0a1e", "#2d1430", "#3a1a2c"), tile = true, make = ::makeLanterns,
    )
    private val snowfall = AsciiScene(
        id = "ascii-snowfall", name = "Snowfall",
        note = "Layers of snow drift past pines and far mountains",
        bg = listOf("#7f97b3", "#c9d6e4", "#eef3f8"), make = ::makeSnowfall,
    )
    private val orbit = AsciiScene(
        id = "ascii-orbit", name = "Orbit",
        note = "Planets circle a glowing sun with trails behind them",
        bg = listOf("#06060e", "#161628"), make = ::makeOrbit,
    )
    private val garden = AsciiScene(
        id = "ascii-garden", name = "Garden",
        note = "Grass sways, flowers open and two butterflies wander",
        bg = listOf("#f7fcf1", "#c9e6bd"), make = ::makeGarden,
    )
    private val ripple = AsciiScene(
        id = "ascii-ripple", name = "Ripple Grid",
        note = "A quiet dot grid; drops send rings across it",
        bg = listOf("#131417", "#202227"), make = ::makeRipple,
    )
    private val meteors = AsciiScene(
        id = "ascii-meteors", name = "Meteor Night",
        note = "Shooting stars cross the sky above the hills",
        bg = listOf("#0b0a1d", "#33224b"), make = ::makeMeteors,
    )
    private val kelp = AsciiScene(
        id = "ascii-kelp", name = "Kelp Forest",
        note = "Kelp sways while small fish swim through",
        bg = listOf("#0f5566", "#04202a"), make = ::makeKelp,
    )

    /** In picker order: Allternit, moods, scenes. */
    val all: List<AsciiScene> = listOf(
        aBlock, clay, inkBloom, spectrum, eventHorizon, lattice, ember, skyDrift, glyphStars, rainGlass,
        codeRain, northernLights, tide, lanterns, snowfall, orbit, garden, ripple, meteors, kelp,
    )

    private val byId: Map<String, AsciiScene> = all.associateBy { it.id }

    fun scene(id: String): AsciiScene? = byId[id]
}

// MARK: - Allternit

private class Blk(val x: Double, val y: Double, val ol: Int, val st: Double)
private class ABPart(val a: Double, val d: Double, val v: Double, val p: Double, val coral: Boolean)

private fun makeABlock(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    // x, y on the 5 x 5 grid, outline order, stroke order.
    val raw = listOf(
        listOf(2, 0, 4, 4), listOf(1, 1, 3, 3), listOf(3, 1, 5, 5), listOf(0, 2, 2, 2), listOf(1, 2, 10, 9), listOf(3, 2, 9, 10),
        listOf(4, 2, 6, 6), listOf(0, 3, 1, 1), listOf(4, 3, 7, 7), listOf(0, 4, 0, 0), listOf(4, 4, 8, 8),
    )
    val blocks = raw.map { Blk(d(it[0]), d(it[1]), it[2], d(it[3])) }
    val slots = blocks.sortedBy { it.ol }.map { doubleArrayOf(it.x, it.y) }
    val u0 = max(1.2, (rd * 0.54) / 5)
    val uc = u0 * 2
    val bh = max(1, rnd(u0 * 0.82).toInt())
    val bw = max(2, rnd(uc * 0.82).toInt())
    val ox = cd / 2 - 2.5 * uc
    val oy = rd * 0.48 - 2.5 * u0
    val scatter = blocks.map {
        val sx = (rand() - 0.5) * 2
        val sy = (rand() - 0.5) * 2
        doubleArrayOf(sx, sy)
    }
    val parts = (0 until Ascii.count(cd * rd / 80, dn, 6)).map {
        val a = rand() * TAU
        val dd = 0.35 + rand() * 0.65
        val v = 0.05 + rand() * 0.08
        val p = rand()
        ABPart(a, dd, v, p, rand() < 0.25)
    }
    val ink = "#d8d4cc"
    val core = "#D97757"

    fun block(g: AsciiGrid, gx: Double, gy: Double, col: String) {
        val x0 = rnd(ox + gx * uc + (uc - d(bw)) / 2)
        val y0 = rnd(oy + gy * u0 + (u0 - d(bh)) / 2)
        for (j in 0 until bh) for (i in 0 until bw) g.put(x0 + d(i), y0 + d(j), ' ', col, 1.0)
    }

    return { g, t, pt ->
        for (gy in -12 until 16) {
            for (gx in -24 until 28) {
                val x = rnd(ox + d(gx) * uc + uc / 2)
                val y = rnd(oy + d(gy) * u0 + u0 / 2)
                if (x < 0 || x >= cd || y < 0 || y >= rd) continue
                var al = 0.16
                if (pt.s > 0.01) al += max(0.0, 1 - hypot(x - pt.x, (y - pt.y) * 2) / 18) * 0.5 * pt.s
                g.put(x, y, '\u00B7', "#8a8f98", al)
            }
        }
        val ccx = ox + 2.5 * uc
        val ccy = oy + 2.5 * u0
        val reach = hypot(cd / 2, rd)
        for (q in parts) {
            val life = Ascii.mod(q.p + t * q.v, 1.0)
            val dist = (1 - life) * reach * q.d
            g.put(ccx + cos(q.a) * dist, ccy + sin(q.a) * dist * 0.5, '\u25AA', if (q.coral) core else ink, 0.2 + 0.45 * life)
        }
        // Breathe, sweep, conveyor, write: five seconds each.
        val ph = Ascii.mod(t, 20.0) / 5
        val st = ph.toInt()
        val u = ph - d(st)
        for ((i, b) in blocks.withIndex()) {
            var gx = b.x
            var gy = b.y
            var col = ink
            var show = true
            if (st == 0) {
                val k = sin(u * PI).pow(2) * 0.35
                gx += (b.x - 2) * k
                gy += (b.y - 2) * k
            } else if (st == 1) {
                val sw = Ascii.mod(atan2(b.x - 2, 2 - b.y) / TAU + 1, 1.0)
                val s2 = Ascii.mod(u * 2, 1.0)
                val dd = min(abs(sw - s2), 1 - abs(sw - s2))
                val k = max(0.0, 1 - dd * 7)
                gx += (b.x - 2) * k * 0.28
                gy += (b.y - 2) * k * 0.28
                if (k > 0.35) col = "#ffffff"
            } else if (st == 2) {
                val pp = u * 11
                val sI = pp.toInt()
                val f = Ascii.smooth(pp - d(sI))
                val fr = slots[(b.ol + sI) % 11]
                val to = slots[(b.ol + sI + 1) % 11]
                gx = fr[0] + (to[0] - fr[0]) * f
                gy = fr[1] + (to[1] - fr[1]) * f
            } else {
                val k = if (u < 0.2) 1 - Ascii.smooth(u / 0.2) else min(1.0, max(0.0, (u - 0.2) * 16 - b.st))
                show = k > 0.02
                gx = 2 + (b.x - 2) * Ascii.smooth(k)
                gy = 2 + (b.y - 2) * Ascii.smooth(k)
            }
            if (pt.s > 0.01) {
                val dx = gx - ((pt.x - ox) / uc - 0.5)
                val dy = gy - ((pt.y - oy) / u0 - 0.5)
                val dist = hypot(dx, dy) + 0.01
                val f = pt.s * max(0.0, 1 - dist / 3.4).pow(2) * 1.3
                gx += dx / dist * f
                gy += dy / dist * f
            }
            val age = t - pt.wt
            if (age >= 0 && age < 2.4) {
                val e = if (age < 0.3) Ascii.smooth(age / 0.3) else max(0.0, 1 - Ascii.smooth((age - 0.3) / 2.1))
                gx += scatter[i][0] * 5 * e
                gy += scatter[i][1] * 3.5 * e
            }
            if (show) block(g, gx, gy, col)
        }
        var cx2 = 2.0
        var cy2 = 2.0
        if (pt.s > 0.01) {
            cx2 += max(-0.7, min(0.7, ((pt.x - ox) / uc - 2.5) * 0.15)) * pt.s
            cy2 += max(-0.7, min(0.7, ((pt.y - oy) / u0 - 2.5) * 0.15)) * pt.s
        }
        val hx = ox + (cx2 + 0.5) * uc
        val hy = oy + (cy2 + 0.5) * u0
        val rr = u0 * 1.1 + sin(t * 2) * 0.3
        var a = 0.0
        while (a < TAU) {
            g.put(hx + cos(a) * rr * 2, hy + sin(a) * rr, '\u00B7', core, 0.35)
            a += 0.18
        }
        block(g, cx2, cy2, if (st == 2 && (u * 11).toInt() % 2 == 1) "#e8906f" else core)
    }
}

// MARK: - Moods

private fun makeClay(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val sd = rand() * 100
    val sc = 0.05 / max(0.6, min(1.4, d(c) / 90))
    val v = DoubleArray(c * r)
    return { g, t, _ ->
        val k = 7 + 3 * dn
        for (y in 0 until r) for (x in 0 until c) v[y * c + x] = Ascii.fbm(d(x) * sc + sd, d(y) * sc * 2 + t * 0.035, 7.0) * k
        for (y in 0 until r) {
            for (x in 0 until c) {
                val b = floor(v[y * c + x]).toInt()
                val bx = if (x + 1 < c) floor(v[y * c + x + 1]).toInt() else b
                val by = if (y + 1 < r) floor(v[(y + 1) * c + x]).toInt() else b
                if (bx != b || by != b) {
                    val hi = max(b, max(bx, by))
                    val glyph = if (bx != b && by != b) {
                        if ((bx > b) == (by > b)) '/' else '\\'
                    } else if (by != b) '-' else '|'
                    g.put(d(x), d(y), if (hi % 4 == 0) '~' else glyph, if (hi % 2 != 0) "#c2603e" else "#8a7a66", if (hi % 4 == 0) 0.95 else 0.6)
                } else if ((x * 7 + y * 13) % 23 == 0) {
                    g.put(d(x), d(y), '.', "#b8a68e", 0.3 * dn)
                }
            }
        }
    }
}

private fun makeInkBloom(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val ramp = " .\u00B7:-=+*#%@"
    val cd = d(c)
    val rd = d(r)
    return { g, t, _ ->
        val fx = cd / 2
        val fy = rd / 2
        val rad = min(cd / 2.1, rd) * 0.44 * (1 + 0.07 * sin(t * 0.8))
        for (y in 0 until r) {
            for (x in 0 until c) {
                val dx = (d(x) - fx) / 2
                val dy = d(y) - fy
                val dist = hypot(dx, dy) / rad
                if (dist > 1.3) continue
                val a = atan2(dy, dx)
                val n = Ascii.fbm(cos(a) * 1.6 + t * 0.18 + dx * 0.05, sin(a) * 1.6 - t * 0.14 + dy * 0.05, 3.0)
                val v = max(0.0, (1 - dist) * 1.25 + (n - 0.5) * (0.7 + 0.4 * dn))
                val i = min(ramp.length - 1, (v * ramp.length).toInt())
                if (i > 0) g.put(d(x), d(y), chr(ramp, i), "#161616", 0.22 + 0.68 * min(1.0, v))
            }
        }
    }
}

private class Spark(val x: Double, val y: Double, val p: Double, val s: Double)

private fun makeSpectrum(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val gcList = listOf("#4f8df5", "#7b6cf0", "#a86ad8", "#d9668a")
    fun gc(x: Double): String = gcList[max(0, min(3, ((x / cd) * 4).toInt()))]
    val sparks = (0 until Ascii.count(cd * rd / 40, dn, 8)).map {
        val x = floor(rand() * cd)
        val y = floor(rand() * rd)
        val p = rand() * TAU
        Spark(x, y, p, 0.6 + rand() * 1.6)
    }
    return { g, t, _ ->
        for (k in 0 until 4) {
            val kd = d(k)
            for (xi in 0 until c) {
                val x = d(xi)
                val y = rd * (0.28 + kd * 0.14) + sin(x * 0.045 + t * (0.35 + kd * 0.07) + kd * 1.3) * rd * 0.12 + sin(x * 0.013 - t * 0.2) * rd * 0.05
                val a = 0.5 + 0.5 * (0.5 + 0.5 * sin(x * 0.08 - t + kd))
                g.put(x, y, '\u2248', gc(x), a)
                g.put(x, y + 1, '~', gc(x), a * 0.6)
                g.put(x, y - 1, '-', gc(x), a * 0.4)
                g.put(x, y + 2, '.', gc(x), a * 0.3)
            }
        }
        for (s in sparks) {
            val v = sin(t * s.s + s.p)
            if (v <= 0.2) continue
            val col = gc(s.x)
            g.put(s.x, s.y, if (v > 0.85) '*' else '+', col, 0.4 + 0.6 * v)
            if (v > 0.92) {
                g.put(s.x - 1, s.y, '-', col, 0.55)
                g.put(s.x + 1, s.y, '-', col, 0.55)
                g.put(s.x, s.y - 1, '|', col, 0.45)
                g.put(s.x, s.y + 1, '|', col, 0.45)
            }
        }
    }
}

private fun makeEventHorizon(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val stars = Ascii.stars(rand, c, rd, (cd * rd / 35) * dn)
    val ramp = ".:-=+*#"
    return { g, t, _ ->
        Ascii.drawStars(g, stars, t, "#ffffff")
        val cx = cd / 2
        val cy = rd / 2
        val kk = min(cd / 2.3, rd * 0.95) / 12
        for (y in 0 until r) {
            for (x in 0 until c) {
                val dx = (d(x) - cx) / 2 / kk
                val dy = (d(y) - cy) / kk
                val dist = hypot(dx, dy)
                if (dist < 2.5) {
                    g.erase(d(x), d(y))
                    continue
                }
                val ey = dy / 0.3
                val de = hypot(dx, ey)
                var v = 0.0
                if (de > 3 && de < 9.5) v = (1 - abs(de - 5) / 4.5) * (0.5 + 0.5 * sin(atan2(ey, dx) * 3 - t * 1.1 + de * 0.9))
                if (dy < 0 && de < 9.5 && de > 3) v *= 0.55
                if (dist < 3.3) v = max(v, 1 - abs(dist - 2.85) * 2.4)
                if (v > 0.08) g.put(d(x), d(y), chr(ramp, min(6, (v * 7).toInt())), "#f4f4f4", 0.2 + 0.8 * v)
            }
        }
    }
}

private class Pulse(val k: Int, val o: Double, val v: Double, val dir: Int)

private fun makeLattice(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val pp = 8
    val span = ceil((d(c) / 2 + d(r)) / d(pp)).toInt()
    val pulses = (0 until Ascii.count(10.0, dn, 3)).map {
        val k = (rand() * d(span)).toInt()
        val o = rand() * 50
        val v = 5 + rand() * 8
        Pulse(k, o, v, if (rand() < 0.5) 1 else -1)
    }
    return { g, t, _ ->
        for (y in 0 until r) {
            var x = 0
            while (x < c) {
                val u = ((x / 2 + y) % pp + pp) % pp
                val w = ((x / 2 - y) % pp + pp) % pp
                if (u == 0 && w == 0) g.put(d(x), d(y), 'o', "#5fd3d0", 0.75)
                else if (u == 0) g.put(d(x), d(y), '/', "#2c7a80", 0.4)
                else if (w == 0) g.put(d(x), d(y), '\\', "#2c7a80", 0.4)
                x += 2
            }
        }
        val sp = d(r + 8)
        for (p in pulses) {
            for (i in 0 until 6) {
                val yy = rnd(Ascii.wrap(p.o + t * p.v - d(i) * 0.9, sp) - 4)
                val x = if (p.dir > 0) 2 * (d(p.k * pp) - yy) else 2 * (d((p.k - r / pp) * pp) + yy)
                g.put(x, yy, if (p.dir > 0) '/' else '\\', "#9ff5ef", if (i > 0) 0.75 * (1 - d(i) / 6) else 1.0)
            }
        }
    }
}

private fun makeEmber(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val gcList = listOf("#ffd800", "#ffaf00", "#ff8205", "#fa500f", "#e10500")
    val w = 3
    val rd = d(r)
    val ph = (0 until ceil(d(c) / d(w)).toInt()).map { rand() * TAU }
    return { g, t, _ ->
        for (i in ph.indices) {
            val id = d(i)
            val h = rnd(rd * (0.12 + 0.4 * dn * 0.6 * (0.5 + 0.5 * sin(t * 0.9 + ph[i])) * (0.6 + 0.4 * sin(id * 0.35 + t * 0.3)))).toInt()
            if (h > 0) {
                for (k in 0 until h) {
                    val col = gcList[max(0, min(4, ((1 - d(k) / max(1.0, rd * 0.55)) * 5).toInt()))]
                    for (j in 0 until (w - 1)) g.put(d(i * w + j), rd - 1 - d(k), ' ', col, 0.95)
                }
                g.put(d(i * w), rd - 2 - d(h) - rnd(1 + abs(sin(t * 2 + ph[i])) * 2), ' ', "#ffd800", 0.85)
            }
        }
    }
}

// MARK: - Scenes

private class FarCloud(val x: Double, val y: Double, val s: Double, val w: Int)
private class NearCloud(val x: Double, val y: Double, val s: Double, val w: Int, val p: Double)

private fun makeSkyDrift(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val far = (0 until Ascii.count(cd / 12, dn)).map {
        val x = rand() * cd
        val y = 1 + rand() * rd * 0.55
        val s = 0.25 + rand() * 0.3
        FarCloud(x, y, s, (6 + rand() * 6).toInt())
    }
    val near = (0 until Ascii.count(cd / 14, dn, 3)).map {
        val x = rand() * cd
        val y = rd * 0.18 + rand() * rd * 0.7
        val s = 0.8 + rand() * 0.6
        val w = (10 + rand() * 12).toInt()
        NearCloud(x, y, s, w, rand() * 9)
    }
    return { g, t, _ ->
        for (k in far) {
            val x = Ascii.mod(k.x + t * k.s, cd + d(k.w)) - d(k.w) / 2
            for (i in 0 until k.w) g.put(x + d(i), k.y, if (i % 2 == 1) '.' else '-', "#ffffff", 0.55)
        }
        for (k in near) {
            val x0 = Ascii.mod(k.x + t * k.s, cd + d(k.w * 2)) - d(k.w)
            val h = max(2, k.w / 5)
            for (y in -h..1) {
                for (i in 0 until k.w) {
                    val u = (d(i) / d(k.w - 1)) * 2 - 1
                    if (d(y) >= -d(h) * (1 - u * u) * (0.75 + 0.25 * sin(d(i) * 1.3 + k.p))) {
                        g.put(x0 + d(i), k.y + d(y), if (y == 1) '_' else ':', if (y == 1) "#d3e5f6" else "#ffffff", 0.95)
                    }
                }
            }
        }
    }
}

private class GStar(val x: Double, val y: Double, val vx: Double, val vy: Double, val ch: Char, val p: Double, val big: Boolean)

private fun makeGlyphStars(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val n = Ascii.count(cd * rd / 60, dn, 10)
    val glyphs = "A:/*+"
    val s = (0 until n).map {
        val x = rand() * cd
        val y = rand() * rd
        val vx = (rand() - 0.5) * 0.5
        val vy = (rand() - 0.5) * 0.25
        val ch = chr(glyphs, (rand() * 5).toInt())
        val p = rand() * TAU
        GStar(x, y, vx, vy, ch, p, rand() < 0.2)
    }
    fun slope(dx: Double, dy: Double): Char {
        val q = Ascii.wrap(atan2(dy * 2, dx), PI)
        return if (q < 0.4 || q > 2.75) '-' else if (q < 1.2) '\\' else if (q < 1.95) '|' else '/'
    }
    val offs = listOf(doubleArrayOf(-2.0, 0.0), doubleArrayOf(2.0, 0.0), doubleArrayOf(0.0, -1.0), doubleArrayOf(0.0, 1.0))
    return { g, t, _ ->
        val px = DoubleArray(n) { Ascii.wrap(s[it].x + s[it].vx * t, cd) }
        val py = DoubleArray(n) { Ascii.wrap(s[it].y + s[it].vy * t, rd) }
        for (i in 0 until n) {
            for (j in (i + 1) until max(i + 1, n)) {
                val dx = px[j] - px[i]
                val dy = py[j] - py[i]
                val dist = hypot(dx / 2, dy)
                if (dist >= 7) continue
                val al = 0.6 * (1 - dist / 7)
                Ascii.line(g, px[i], py[i], px[j], py[j], slope(dx, dy), "#7f8fff", al)
                val f = Ascii.mod(t * 0.5 + d(i) * 0.37 + d(j) * 0.11, 1.0)
                g.put(px[i] + dx * f, py[i] + dy * f, '\u2022', "#e3e8ff", min(1.0, al * 1.8))
            }
        }
        for ((i, p) in s.withIndex()) {
            val tw = 0.6 + 0.4 * sin(t * 2 + p.p)
            if (p.big) {
                for (o in offs) g.put(px[i] + o[0], py[i] + o[1], '\u00B7', "#aab6ff", 0.4 * tw)
            }
            g.put(px[i], py[i], p.ch, if (p.big) "#ffffff" else "#d6dcff", tw)
        }
    }
}

private class RainDrop(val x: Double, val y: Double, val v: Double, val l: Int)
private class Bead(val x: Double, val y: Double, val v: Double)
private class CityLight(val x: Double, val y: Double, val col: String, val p: Double)

private fun makeRainGlass(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val drops = (0 until Ascii.count(cd * 0.7, dn, 8)).map {
        val x = floor(rand() * cd)
        val y = rand() * rd
        val v = 10 + rand() * 12
        RainDrop(x, y, v, (3 + rand() * 5).toInt())
    }
    val beads = (0 until Ascii.count(cd * rd / 70, dn, 6)).map {
        val x = floor(rand() * cd)
        val y = rand() * rd
        val v = if (rand() < 0.35) 0.4 + rand() * 1.2 else 0.0
        Bead(x, y, v)
    }
    val palette = listOf("#fbbf24", "#f472b6", "#60a5fa")
    val lights = (0 until Ascii.count(cd / 8, dn, 3)).map {
        val x = rand() * cd
        val y = rd * (0.3 + rand() * 0.6)
        val col = palette[(rand() * 3).toInt()]
        CityLight(x, y, col, rand() * TAU)
    }
    return { g, t, _ ->
        for (l in lights) {
            for (i in -1..1) for (j in -1..1) g.put(l.x + d(i * 2), l.y + d(j), 'o', l.col, 0.18 + 0.08 * sin(t + l.p))
        }
        for (k in drops) {
            val y = Ascii.mod(k.y + t * k.v, rd + d(k.l))
            for (i in 0 until k.l) g.put(k.x, y - d(i), if (i == 0) '|' else if (i < 3) ':' else '.', "#d4e4f4", (1 - d(i) / d(k.l)) * 0.6)
        }
        for (k in beads) {
            val y = Ascii.mod(k.y + t * k.v, rd)
            g.put(k.x, y, 'o', "#eef6ff", 0.85)
            if (k.v > 0) {
                g.put(k.x, y - 1, ':', "#eef6ff", 0.35)
                g.put(k.x, y - 2, '.', "#eef6ff", 0.2)
            }
        }
    }
}

private class Stream(val x: Double, val y: Double, val v: Double, val l: Int)

private fun makeCodeRain(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val chars = "const=>{}();01aAfn<>/*[]\$#"
    val rd = d(r)
    val s = ArrayList<Stream>()
    for (x in 0 until c) {
        if (rand() < 0.45 * dn) {
            val y = rand() * rd * 2
            val v = 4 + rand() * 9
            s.add(Stream(d(x), y, v, (6 + rand() * 14).toInt()))
        }
    }
    return { g, t, _ ->
        for (k in s) {
            val y = Ascii.mod(k.y + t * k.v, rd + d(k.l))
            for (i in 0 until k.l) {
                val yy = floor(y) - d(i)
                val glyph = chr(chars, (Ascii.hash(k.x * 131 + yy * 17 + floor(t * 3 + d(i))) * chars.length).toInt())
                g.put(k.x, yy, glyph, if (i > 0) "#4fd1a5" else "#d8fff2", if (i > 0) 0.75 * (1 - d(i) / d(k.l)) else 1.0)
            }
        }
    }
}

private fun makeNorthernLights(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val stars = Ascii.stars(rand, c, rd, (cd * rd / 40) * dn, 0.5)
    val lake = rnd(rd * 0.72).toInt()
    val p1 = rand() * TAU
    val hh = IntArray(c) { x -> rnd(1 + abs(sin(d(x) * 0.06 + p1)) * rd * 0.08 + Ascii.fbm(d(x) * 0.15, 1.0, 4.0) * rd * 0.06).toInt() }
    val cols = listOf("#34d399", "#5eead4", "#818cf8", "#c084fc")
    return { g, t, _ ->
        Ascii.drawStars(g, stars, t, "#cbd5ff")
        for (x in 0 until c) {
            val xd = d(x)
            val top = rd * 0.06 + Ascii.fbm(xd * 0.04 - t * 0.08, t * 0.05, 5.0) * rd * 0.25
            val len = rd * (0.15 + 0.35 * Ascii.fbm(xd * 0.09 + t * 0.12, 3.0, 6.0)) * (0.7 + 0.3 * dn)
            val sh = 0.35 + 0.65 * Ascii.fbm(xd * 0.12 - t * 0.3, 4.0, 8.0)
            val cc = cols[min(3, (Ascii.fbm(xd * 0.03 + t * 0.05, 2.0, 9.0) * 4.4).toInt())]
            var i = 0.0
            while (i < len) {
                val y = top + i
                if (y >= d(lake - hh[x])) break
                val f = i / len
                g.put(xd, y, if (i < 1) '\u2248' else if (f < 0.35) '|' else if (f < 0.7) ':' else '.', cc, sh * (if (i < 1) 0.9 else (1 - f) * 0.95))
                i += 1
            }
            for (y in (lake - hh[x]) until max(lake - hh[x], lake)) {
                g.put(xd, d(y), if (y == lake - hh[x]) '^' else '#', "#1c2740", 1.0)
            }
        }
        for (y in lake until r) {
            val my = 2 * lake - 1 - y
            if (my < 0) continue
            for (x in 0 until c) {
                val sx = rnd(d(x) + sin(d(y) * 0.9 + t * 1.5) * 0.8).toInt()
                if (sx < 0 || sx >= c) continue
                val i = my * c + sx
                val ch = g.chAt(i)
                if (ch != AsciiGrid.NONE) g.put(d(x), d(y), if (ch == '|') ':' else ch, g.col[i], g.a[i] * 0.32)
            }
        }
    }
}

private class Bird(val o: Double, val y: Double, val v: Double)

private fun makeTide(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val hz = rnd(rd * 0.55).toInt()
    val hzd = d(hz)
    val sx = cd * 0.68
    val sr = max(3.0, rd * 0.13)
    val birds = (0 until Ascii.count(4.0, dn, 2)).map {
        val o = rand() * cd
        val y = rd * (0.1 + rand() * 0.25)
        Bird(o, y, 2 + rand() * 2)
    }
    return { g, t, _ ->
        for (y in (hz - ceil(sr).toInt()) until hz) {
            var x = floor(sx - sr * 2.2)
            while (x <= sx + sr * 2.2) {
                val dx = (x - sx) / 2.1
                val dy = d(y) - (hzd - 1)
                if (dx * dx + dy * dy < sr * sr) g.put(x, d(y), '@', if (d(y) < hzd - sr * 0.5) "#ff9f5a" else "#ff6f3c", 0.92)
                x += 1
            }
        }
        for (i in 0 until 3) {
            val id = d(i)
            val y = hzd - sr - 3 - id * 2.5
            val w = 10 + i * 4
            val x = Ascii.mod(id * 37 + t * (1 + id * 0.4), cd + d(w)) - d(w)
            for (j in 0 until w) g.put(x + d(j), y, '=', "#fff3e6", 0.7)
        }
        for (b in birds) {
            g.put(Ascii.mod(b.o + t * b.v, cd + 4) - 2, b.y + sin(t * 3 + b.o) * 0.4, if (sin(t * 8 + b.o) > 0) 'v' else '-', "#5a3a3a", 0.8)
        }
        for (y in hz until r) {
            val f = (d(y) - hzd) / (rd - hzd)
            for (x in 0 until c) {
                val xd = d(x)
                val yd = d(y)
                val v = sin(xd * 0.22 - t * 1.4 + yd * 0.9) + sin(xd * 0.09 + t * 0.6 - yd * 0.3)
                val glyph: Char? = if (v > 1.1) '~' else if (v > 0.2) '-' else if (v > -0.7) '_' else null
                if (glyph != null) g.put(xd, yd, glyph, "#1f6f8c", 0.35 + f * 0.55)
            }
            val rw = sr * 2.2 * (1 - f * 0.7)
            var x = floor(sx - rw)
            while (x < sx + rw) {
                if (Ascii.hash(x * 7 + d(y) * 13 + floor(t * 4)) > 0.45) g.put(x, d(y), '=', "#ffb07a", 0.85 * (1 - f))
                x += 1
            }
        }
        val bx = Ascii.mod(t * 1.3, cd + 14) - 10
        val by = hzd - 3 + sin(t * 1.8) * 0.5
        g.sprite(bx, by, listOf("   |\\", "   | \\", "   |__\\"), "#fff7ed", 0.95)
        g.sprite(bx, by + 3, listOf("\\_______/"), "#3a2a2a", 0.95)
    }
}

private class Lantern(val x: Double, val y: Double, val v: Double, val p: Double, val z: Double)

private fun makeLanterns(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val stars = Ascii.stars(rand, c, rd * 0.6, (cd * rd / 70) * dn)
    val lake = rnd(rd * 0.78).toInt()
    val span = d(lake + 6)
    val lamps = (0 until Ascii.count(cd * rd / 80, dn, 6)).map {
        val x = rand() * cd
        val y = rand() * span
        val v = 0.4 + rand()
        val p = rand() * TAU
        Lantern(x, y, v, p, rand())
    }
    val sparks = listOf(
        doubleArrayOf(-1.0, 1.0), doubleArrayOf(3.0, 1.0), doubleArrayOf(-1.0, 2.0),
        doubleArrayOf(3.0, 2.0), doubleArrayOf(1.0, 3.0), doubleArrayOf(1.0, -1.0),
    )
    return { g, t, _ ->
        Ascii.drawStars(g, stars, t, "#f5d0fe")
        for (l in lamps) {
            val y = Ascii.wrap(l.y - t * l.v * (0.5 + l.z), span) - 4
            val x = l.x + sin(t * 0.5 + l.p) * 2
            val fl = 0.85 + 0.15 * sin(t * 7 + l.p * 3)
            if (l.z > 0.65) {
                g.text(x, y, ".-.", "#ffd08a", 0.6)
                for (i in 0 until 3) {
                    g.put(x + d(i), y + 1, ' ', if (i == 1) "#ffcf6b" else "#ffb347", fl)
                    g.put(x + d(i), y + 2, ' ', if (i == 1) "#ffb347" else "#f59e0b", fl)
                }
                for (o in sparks) g.put(x + o[0], y + o[1], '\u00B7', "#ff9f43", 0.4)
            } else if (l.z > 0.3) {
                for (i in 0 until 2) {
                    g.put(x + d(i), y, ' ', "#ffb347", fl)
                    g.put(x + d(i), y + 1, ' ', "#f59e0b", fl)
                }
            } else {
                g.put(x, y, 'o', "#ffcf80", 0.6 * fl)
            }
        }
        for (y in lake until r) {
            for (x in 0 until c) {
                val my = 2 * lake - 1 - y
                if (my < 0) continue
                val xd = d(x)
                val yd = d(y)
                val i = my * c + x
                if (g.chAt(i) != AsciiGrid.NONE && g.a[i] > 0.5 && Ascii.hash(xd * 13 + yd * 7 + floor(t * 6)) > 0.35) {
                    g.put(xd + rnd(sin(yd + t * 2)), yd, '~', g.col[i], 0.45)
                } else if ((x + y * 3 + floor(t * 3).toInt()) % 9 == 0) {
                    g.put(xd, yd, '-', "#6b3a5a", 0.3)
                }
            }
        }
    }
}

private class Flake(val x: Double, val y: Double, val v: Double, val p: Double)
private class Pine(val x: Double, val h: Int)

private fun makeSnowfall(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    fun flakes(n: Double, z: Double): List<Flake> = (0 until Ascii.count(n, dn, 6)).map {
        val x = rand() * cd
        val y = rand() * rd
        val v = (0.6 + rand()) * (1 + z * 2)
        Flake(x, y, v, rand() * TAU)
    }
    val far = flakes(cd * rd / 40, 0.0)
    val near = flakes(cd * rd / 80, 1.0)
    val p1 = rand() * 9
    val mtn = IntArray(c) { x -> rnd(rd * 0.3 + sin(d(x) * 0.06 + p1) * rd * 0.08 + Ascii.fbm(d(x) * 0.2, 0.0, 3.0) * rd * 0.05).toInt() }
    fun ground(x: Double): Int = rnd(rd * 0.12 + sin(x * 0.09) * rd * 0.03).toInt()
    val pines = (0 until max(3, rnd(cd / 11).toInt())).map {
        val x = floor(rand() * cd)
        Pine(x, (3 + rand() * max(2.0, rd * 0.16)).toInt())
    }
    return { g, t, _ ->
        for (x in 0 until c) {
            for (y in max(0, r - mtn[x]) until r) {
                g.put(d(x), d(y), if (y == r - mtn[x]) '^' else ':', if (y < r - mtn[x] + 2) "#ffffff" else "#9fb2c8", 0.7)
            }
        }
        for (f in far) g.put(f.x + sin(t * 0.6 + f.p), Ascii.mod(f.y + t * f.v, rd), '.', "#ffffff", 0.6)
        for (p in pines) {
            val base = d(r - ground(p.x))
            for (k in 0 until p.h) {
                val w = rnd(d(p.h - k) * 0.7).toInt()
                for (i in -w..w) g.put(p.x + d(i), base - 1 - d(k), if (i < 0) '/' else if (i > 0) '\\' else '|', if (k % 2 == 1) "#2c4a3e" else "#35574a", 1.0)
            }
            g.put(p.x, base - 1 - d(p.h), '^', "#2c4a3e", 1.0)
        }
        for (x in 0 until c) {
            val h = ground(d(x))
            for (y in max(0, r - h) until r) g.put(d(x), d(y), if (y == r - h) '_' else '\u00B7', "#ffffff", 0.95)
        }
        for (f in near) g.put(f.x + sin(t * 0.9 + f.p) * 2, Ascii.mod(f.y + t * f.v, rd), '*', "#ffffff", 0.95)
    }
}

private class Planet(val d: Double, val s: Double, val ch: Char, val col: String)

private fun makeOrbit(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val stars = Ascii.stars(rand, c, rd, (cd * rd / 50) * dn)
    val cx = cd / 2
    val cy = rd / 2
    val ky = (rd / 2 - 1) / 18
    val kx = min(ky * 2.1, (cd / 2 - 1) / 18)
    val planets = listOf(Planet(7.0, 0.7, 'o', "#60a5fa"), Planet(12.0, 0.42, 'O', "#f472b6"), Planet(17.0, 0.25, '0', "#a3e635"))
    val spokes = "-\\|/"
    fun posX(dist: Double, a: Double): Double = cx + cos(a) * dist * kx
    fun posY(dist: Double, a: Double): Double = cy + sin(a) * dist * ky
    return { g, t, _ ->
        Ascii.drawStars(g, stars, t, "#c7d2fe")
        for (p in planets) {
            var a = 0.0
            while (a < TAU) {
                g.put(posX(p.d, a), posY(p.d, a), '.', "#8b8bb5", 0.25)
                a += 0.04
            }
        }
        val sr = max(1.2, ky * 3.5)
        var y = -ceil(sr * 1.5)
        while (y <= sr * 1.5) {
            var x = -ceil(sr * 3)
            while (x <= sr * 3) {
                val dist = hypot(x / 2, y)
                if (dist < sr) {
                    g.put(cx + x, cy + y, '@', "#ffd166", 1.0)
                } else if (dist < sr + 1) {
                    val k = ((floor((atan2(y, x / 2) / TAU) * 8 + t * 3).toInt() % 4) + 4) % 4
                    g.put(cx + x, cy + y, chr(spokes, k), "#ffb347", 0.6)
                }
                x += 1
            }
            y += 1
        }
        for ((i, p) in planets.withIndex()) {
            for (j in 8 downTo 0) {
                val a = t * p.s + d(i) * 2 - d(j) * 0.06
                g.put(posX(p.d, a), posY(p.d, a), if (j > 0) '.' else p.ch, p.col, if (j > 0) 0.5 * (1 - d(j) / 9) else 1.0)
            }
        }
        val ma = t * planets[1].s + 2
        val mx = posX(planets[1].d, ma)
        val my = posY(planets[1].d, ma)
        g.put(mx + cos(t * 2.5) * 3, my + sin(t * 2.5) * 1.5, '\u00B7', "#e5e7eb", 0.9)
    }
}

private class Blade(val h: Int, val p: Double, val f: Char?, val fc: String)
private class Butterfly(val a: Double, val cx: Double, val col: String)

private fun makeGarden(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val flowers = listOf('*', '@', 'o')
    val fcols = listOf("#e0559a", "#f2a93b", "#8b5cf6")
    val blades = (0 until c).map {
        val h = (2 + rand() * max(3.0, rd * 0.3 * dn)).toInt()
        val p = rand() * TAU
        val f: Char? = if (rand() < 0.18 * dn) flowers[(rand() * 3).toInt()] else null
        Blade(h, p, f, fcols[(rand() * 3).toInt()])
    }
    val flies = listOf(0, 1).map { i -> Butterfly(rand() * TAU, cd * (0.3 + 0.4 * d(i)), if (i == 1) "#f2a93b" else "#e0559a") }
    return { g, t, _ ->
        for ((x, b) in blades.withIndex()) {
            val xd = d(x)
            val sw0 = sin(t * 1.3 + xd * 0.25 + b.p * 0.3)
            for (k in 0 until b.h) {
                val sw = sw0 * (d(k) / d(b.h))
                g.put(xd + rnd(sw * 1.2), rd - 1 - d(k), if (sw > 0.3) '/' else if (sw < -0.3) '\\' else '|', "#3f8f4a", 0.55 + (0.45 * d(k)) / d(b.h))
            }
            val f = b.f
            if (f != null) g.put(xd + rnd(sw0 * 1.2), rd - 1 - d(b.h), if (0.5 + 0.5 * sin(t * 0.4 + b.p) > 0.3) f else '.', b.fc, 1.0)
        }
        for ((i, b) in flies.withIndex()) {
            g.text(
                b.cx + sin(t * 0.5 + b.a) * cd * 0.2, rd * 0.35 + sin(t * 0.9 + b.a * 2) * rd * 0.15,
                if (sin(t * 12 + d(i)) > 0) "}{" else "><", b.col, 1.0,
            )
        }
    }
}

private class Drop(val x: Double, val y: Double, val t0: Double)

private fun makeRipple(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val ramp = " .\u00B7:-=+*#"
    val per = 1.3 / dn
    return { g, t, _ ->
        val now = floor(t / per).toInt()
        val src = ((now - 4)..now).map { k -> Drop(Ascii.hash(d(k * 3 + 1)) * cd, Ascii.hash(d(k * 3 + 2)) * rd, d(k) * per) }
        for (y in 0 until r) {
            var x = 0
            while (x < c) {
                var h = 0.0
                for (s in src) {
                    val age = t - s.t0
                    if (age < 0) continue
                    val dist = hypot((d(x) - s.x) / 2, d(y) - s.y)
                    h += exp(-(dist - age * 7).pow(2) / 2.5) * exp(-age * 0.5)
                }
                if (h > 0.08) g.put(d(x), d(y), chr(ramp, min(8, 1 + (h * 8).toInt())), "#F2A93B", 0.35 + h)
                else g.put(d(x), d(y), '.', "#9ca3af", 0.18)
                x += 2
            }
        }
    }
}

private fun makeMeteors(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val stars = Ascii.stars(rand, c, rd, (cd * rd / 30) * dn, 0.75)
    val p1 = rand() * 9
    val p2 = rand() * 9
    val h1 = IntArray(c) { x -> rnd(rd * 0.18 + sin(d(x) * 0.05 + p1) * rd * 0.08 + sin(d(x) * 0.13 + p2) * rd * 0.03).toInt() }
    val h2 = IntArray(c) { x -> rnd(rd * 0.1 + sin(d(x) * 0.08 + p2) * rd * 0.06).toInt() }
    return { g, t, _ ->
        Ascii.drawStars(g, stars, t, "#fef3c7")
        for (m in 0 until 3) {
            val md = d(m)
            val per = 3.5 + md * 1.3
            val q = t + md * 1.1
            val cyc = floor(q / per)
            val ph = Ascii.mod(q, per)
            if (ph >= 1.4) continue
            val x = Ascii.hash(cyc * 7 + md) * cd * 0.7 + ph * cd * 0.35
            val y = Ascii.hash(cyc * 11 + md) * rd * 0.3 + ph * rd * 0.22
            for (j in 0 until 9) {
                g.put(x - d(j) * 1.6, y - d(j) * 0.6, if (j > 0) '\\' else '*', if (j > 0) "#c4b5fd" else "#fff7d6", if (j > 0) (1 - d(j) / 9) * 0.8 else 1.0)
            }
        }
        for (x in 0 until c) {
            for (y in max(0, r - h1[x]) until r) g.put(d(x), d(y), ':', "#4b3a6e", 0.9)
            for (y in max(0, r - h2[x]) until r) g.put(d(x), d(y), '#', "#1d1530", 1.0)
        }
    }
}

private class Kelp(val x: Double, val h: Double, val p: Double)
private class Fish(val y: Double, val v: Double, val o: Double, val d: Double, val col: String)
private class Bubble(val x: Double, val y: Double, val v: Double)

private fun makeKelp(c: Int, r: Int, rand: () -> Double, dn: Double): AsciiDraw {
    val cd = d(c)
    val rd = d(r)
    val kelp = (0 until Ascii.count(cd / 6, dn, 3)).map {
        val x = rand() * cd
        val h = rd * (0.35 + rand() * 0.5)
        Kelp(x, h, rand() * TAU)
    }
    val fishCols = listOf("#fbbf24", "#fb7185", "#e0f2fe")
    val fish = (0 until Ascii.count(rd / 5, dn, 3)).map {
        val y = 2 + rand() * (rd - 6)
        val v = 2 + rand() * 4
        val o = rand() * 100
        val dir = if (rand() < 0.5) 1.0 else -1.0
        Fish(y, v, o, dir, fishCols[(rand() * 3).toInt()])
    }
    val bubbles = (0 until Ascii.count(cd / 8, dn, 4)).map {
        val x = rand() * cd
        val y = rand() * rd
        Bubble(x, y, 1.5 + rand() * 2)
    }
    return { g, t, _ ->
        for (k in kelp) {
            var i = 0.0
            while (i < k.h) {
                g.put(k.x + sin(i * 0.18 + t * 1.1 + k.p) * (i / k.h) * 3, rd - 1 - i, if (i.toInt() % 2 == 1) '(' else ')', "#3fae6a", 0.85)
                i += 1
            }
        }
        for (f in fish) {
            val p = Ascii.mod(f.o + t * f.v, cd + 8)
            g.text(if (f.d > 0) p - 4 else cd - p, f.y + sin(t + f.o) * 0.5, if (f.d > 0) "><>" else "<><", f.col, 1.0)
        }
        for (b in bubbles) g.put(b.x + sin(t * 2 + b.x), Ascii.wrap(b.y - t * b.v, rd), 'o', "#bdf2ff", 0.5)
    }
}
