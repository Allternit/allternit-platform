package com.allternit.feature.chat.games

import android.content.SharedPreferences
import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.Typeface
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject
import java.util.Locale
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.sqrt
import kotlin.random.Random

/*
 * The waiting games (iOS WaitingGamesEngine.swift, web waiting-games-engine.ts): eight small games drawn in
 * ASCII on one canvas, played while a bot works (WaitingGamesLayer). Each game keeps its own state and has
 * init/step/draw plus input handlers; the engine owns the character grid and the clock, and reports the
 * score line and the banner back to the window. The game bodies follow the iOS port line for line, so rules,
 * speeds and scoring match. Coordinates are in dp (iOS points); the canvas is scaled by the density once.
 */

/** The game list (iOS WaitingGameID). [raw] is the saved id, shared with iOS and the web. */
enum class WaitingGameId(val raw: String, val title: String) {
    Fish("fish", "Big Fish"),
    Race("race", "Bot Race"),
    Catch("catch", "Token Catch"),
    Dodge("dodge", "Meteor Dodge"),
    Bash("bash", "Bug Bash"),
    Stack("stack", "Stack Deploy"),
    Snake("snake", "Context Snake"),
    Memory("memory", "Bot Memory"),
    Rpg("rpg", "Gizzi RPG"),
    Brawl("brawl", "Gizzi Brawl");

    /** The two full Gizzi games are their own live sites (web SITE_GAMES): shown in a web view, not on the canvas. */
    val site: WaitingSiteGame?
        get() = when (this) {
            Rpg -> WaitingSiteGame(
                url = "https://gizzirpg.allternit.com/",
                hint = "Gizzi: The Assistants, all six chapters. Full screen gives it the most room. Your save stays in the game.",
            )
            Brawl -> WaitingSiteGame(
                url = "https://brawl.allternit.com/",
                hint = "1v1 mascot brawler: pick a fighter and beat the AI. The game turns sideways, so hold your phone sideways.",
                landscape = true,
            )
            else -> null
        }

    companion object {
        fun from(raw: String?): WaitingGameId? = entries.firstOrNull { it.raw == raw }
    }
}

/** A site game. [landscape]: it only plays wider than tall, so a tall window turns it on its side. */
data class WaitingSiteGame(val url: String, val hint: String, val landscape: Boolean = false)

/** A Bot Race lane: one of the person's bots (name, accent hex), or a stand-in. */
data class WaitingGameRacer(val name: String, val color: String? = null)

data class WaitingGameBanner(val title: String, val sub: String)

// MARK: - Drawing

private const val TAU = PI * 2
private fun rnd(a: Double, b: Double): Double = a + Random.nextDouble() * (b - a)
private fun r01(): Double = Random.nextDouble()

/** JS `%` on numbers (keeps the sign of the dividend); Kotlin's Double `%` is the same truncated remainder. */
private fun jmod(a: Double, b: Double): Double = a % b

/** Rounds like Swift's `.rounded()` without throwing on NaN. */
private fun Double.toIntRounded(): Int = if (isNaN()) 0 else roundToInt()

/** Colours by hex, cached: the games draw hundreds of glyphs a frame. Main thread only. */
private object Ink {
    private val cache = HashMap<String, Int>()
    fun color(hex: String): Int = cache.getOrPut(hex) { android.graphics.Color.parseColor(hex) }
    fun alpha(color: Int, a: Double): Int {
        val na = (android.graphics.Color.alpha(color) * a.coerceIn(0.0, 1.0)).roundToInt()
        return (color and 0x00FFFFFF) or (na shl 24)
    }
}

/** Shared paints and faces (the canvas runs on the main thread only). */
private object Paints {
    val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val text = Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG)
    val mono: Typeface = Typeface.MONOSPACE
    val monoBold: Typeface = Typeface.create(Typeface.MONOSPACE, Typeface.BOLD)
    val sansBold: Typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)

    fun text(size: Double, face: Typeface, align: Paint.Align = Paint.Align.LEFT): Paint {
        text.typeface = face
        text.textSize = size.toFloat()
        text.textAlign = align
        return text
    }
}

/** The canvas calls the games make (txt, ptxt, bg, rects), on an Android canvas already scaled to dp. */
internal class WaitingGamesPainter(
    val canvas: Canvas,
    val W: Double, val H: Double, val cw: Double, val chh: Double, val FS: Double,
) {
    /** A multiplier on every colour's alpha (Bot Memory's matched cards). */
    var opacity = 1.0
    private val box = RectF()

    private fun ink(col: String, a: Double): Int = Ink.alpha(Ink.color(col), min(1.0, a) * opacity)

    fun bg(stops: List<String>) {
        val p = Paints.fill
        p.color = android.graphics.Color.BLACK
        p.shader = LinearGradient(0f, 0f, 0f, H.toFloat(), IntArray(stops.size) { Ink.color(stops[it]) }, null, Shader.TileMode.CLAMP)
        canvas.drawRect(0f, 0f, W.toFloat(), H.toFloat(), p)
        p.shader = null
    }

    /** One string on the grid, a glyph per cell (spaces skipped), top-left at column x, row y. */
    fun txt(x: Double, y: Double, s: String, col: String, a: Double = 1.0) {
        if (a <= 0.004) return
        val p = Paints.text(FS, Paints.mono)
        p.color = ink(col, a)
        val base = (y * chh + FS * 0.1).toFloat() - p.ascent()
        var i = 0
        var k = 0
        while (k < s.length) {
            val cp = s.codePointAt(k)
            val n = Character.charCount(cp)
            if (cp != ' '.code) canvas.drawText(s, k, k + n, ((x + i) * cw).toFloat(), base, p)
            i += 1
            k += n
        }
    }

    /** Centred text at a pixel point (the newer games' ptxt). */
    fun ptxt(px: Double, py: Double, s: String, col: String, a: Double = 1.0, size: Double? = null) {
        if (a <= 0.004) return
        val p = Paints.text(size ?: FS, if (size != null) Paints.monoBold else Paints.mono, Paint.Align.CENTER)
        p.color = ink(col, a)
        canvas.drawText(s, px.toFloat(), (py - (p.ascent() + p.descent()) / 2).toFloat(), p)
    }

    /** A centred line in the system face, top at py (Big Fish's flash). */
    fun sans(px: Double, py: Double, s: String, col: String, a: Double, size: Double) {
        if (a <= 0.004) return
        val p = Paints.text(size, Paints.sansBold, Paint.Align.CENTER)
        p.color = ink(col, a)
        canvas.drawText(s, px.toFloat(), py.toFloat() - p.ascent(), p)
    }

    /** A glyph at a pixel point, top-leading (Bot Race's big word). */
    fun glyph(px: Double, py: Double, s: String, col: String, size: Double, bold: Boolean, a: Double = 1.0) {
        val p = Paints.text(size, if (bold) Paints.monoBold else Paints.mono)
        p.color = ink(col, a)
        canvas.drawText(s, px.toFloat(), py.toFloat() - p.ascent(), p)
    }

    /** A line vertically centred on y, starting at x (Stack Deploy's labels). */
    fun label(x: Double, y: Double, s: String, size: Double, color: Int) {
        val p = Paints.text(size, Paints.monoBold)
        p.color = color
        canvas.drawText(s, x.toFloat(), (y - (p.ascent() + p.descent()) / 2).toFloat(), p)
    }

    fun rect(x: Double, y: Double, w: Double, h: Double, color: Int) {
        Paints.fill.color = color
        canvas.drawRect(x.toFloat(), y.toFloat(), (x + w).toFloat(), (y + h).toFloat(), Paints.fill)
    }

    fun roundRect(x: Double, y: Double, w: Double, h: Double, r: Double, color: Int) {
        Paints.fill.color = color
        box.set(x.toFloat(), y.toFloat(), (x + w).toFloat(), (y + h).toFloat())
        canvas.drawRoundRect(box, r.toFloat(), r.toFloat(), Paints.fill)
    }

    /** Draws [block] clipped to a rectangle. */
    inline fun clipped(x: Double, y: Double, w: Double, h: Double, block: () -> Unit) {
        val save = canvas.save()
        canvas.clipRect(x.toFloat(), y.toFloat(), (x + w).toFloat(), (y + h).toFloat())
        block()
        canvas.restoreToCount(save)
    }

    fun withAlpha(col: String, a: Double): Int = ink(col, a)

    companion object {
        fun monoWidth(size: Double, bold: Boolean = false): Double =
            Paints.text(size, if (bold) Paints.monoBold else Paints.mono).measureText("M").toDouble()
    }
}

// MARK: - Engine

/**
 * Owns the grid, the clock and the current game; the window draws it every frame and forwards touches and
 * keys. Score line and banner changes are posted after the frame (iOS DispatchQueue.main.async) so a draw
 * never writes the state it is drawn from.
 */
internal class WaitingGamesEngine(racers: List<WaitingGameRacer>, private val prefs: SharedPreferences) {
    var hud by mutableStateOf<List<String>>(emptyList()); private set
    var banner by mutableStateOf<WaitingGameBanner?>(null); private set
    var playing by mutableStateOf(true); private set

    /** The game on the canvas. State, so the window follows a pick (its hint, whether it wants the keyboard). */
    private var game by mutableStateOf<WaitingGame?>(null)

    var W = 0.0; private set
    var H = 0.0; private set
    var cw = 8.0; private set
    var chh = 14.0; private set
    var C = 0.0; private set
    var R = 0.0; private set
    var FS = 13.0; private set
    private var clock = 0.0
    private var last: Double? = null
    private var needsInit = false
    private val main = Handler(Looper.getMainLooper())
    private val games: Map<WaitingGameId, WaitingGame>
    private var hudSent: List<String> = emptyList()

    init {
        games = mapOf(
            WaitingGameId.Fish to FishGame(this), WaitingGameId.Race to RaceGame(this, racers), WaitingGameId.Catch to CatchGame(this),
            WaitingGameId.Dodge to DodgeGame(this), WaitingGameId.Bash to BugBashGame(this), WaitingGameId.Stack to StackGame(this),
            WaitingGameId.Snake to SnakeGame(this), WaitingGameId.Memory to MemoryGame(this),
        )
        // Best scores are a nicety: a JSON object of game id → best.
        prefs.getString(WaitingGamesStore.BEST_KEY, null)?.let { raw ->
            runCatching {
                val saved = JSONObject(raw)
                for ((id, g) in games) if (saved.has(id.raw)) g.best = saved.getDouble(id.raw)
            }
        }
    }

    fun saveBest() {
        val out = JSONObject()
        for ((id, g) in games) g.best?.let { out.put(id.raw, it) }
        prefs.edit().putString(WaitingGamesStore.BEST_KEY, out.toString()).apply()
    }

    fun setHud(items: List<String>) {
        if (items == hudSent) return
        hudSent = items
        main.post { hud = items }
    }

    fun showBanner(title: String, sub: String = "") {
        val next = WaitingGameBanner(title, sub)
        main.post { banner = next }
    }

    fun hideBanner() {
        main.post { banner = null }
    }

    fun pick(id: WaitingGameId) {
        game = games[id]
        hideBanner()
        setHud(emptyList())
        if (W > 0) game?.initGame() else needsInit = true
    }

    val hint: String get() = game?.hint ?: ""
    val wantsKeyboard: Boolean get() = game?.keyboard ?: false

    fun pointer(x: Double, y: Double) { if (playing) game?.pointer(x, y) }
    fun click(x: Double, y: Double) { if (playing && !needsInit) game?.click(x, y) }

    /** A key the game took ("ArrowUp", "Enter", "Backspace", " " or a letter). */
    fun key(key: String): Boolean = if (playing && !needsInit) game?.key(key) ?: false else false

    fun setPlaying(on: Boolean) {
        if (on == playing) return
        playing = on
        last = null
        if (!on) saveBest()
    }

    /** Fits the grid to the canvas (web `fit`). */
    private fun fit(w: Double, h: Double) {
        W = w; H = h
        FS = if (W < 520) 11.0 else 13.0
        cw = WaitingGamesPainter.monoWidth(FS)
        chh = FS * 1.2
        C = ceil(W / cw); R = ceil(H / chh)
        game?.resize()
    }

    /** One frame: step the game by the time since the last frame, then draw. [w]/[h] in dp, [date] in seconds. */
    fun render(canvas: Canvas, w: Double, h: Double, density: Float, date: Double) {
        if (w < 1 || h < 1) return
        if (w != W || h != H) fit(w, h)
        val g = game
        if (needsInit && g != null) { needsInit = false; g.initGame() }
        var dt = 0.0
        if (playing) {
            last?.let { dt = min(0.05, max(0.0, date - it)) }
            last = date
            clock += dt
        }
        if (g == null) return
        if (playing) g.step(dt, clock)
        val save = canvas.save()
        canvas.scale(density, density)
        g.draw(WaitingGamesPainter(canvas, W, H, cw, chh, FS), clock)
        canvas.restoreToCount(save)
    }

    /** Seconds on a monotonic clock (performance.now() / 1000). */
    val now: Double get() = System.nanoTime() / 1e9
}

// MARK: - Games

private class Flash(val t: Double, val txt: String, val c: String)

private abstract class WaitingGame(val e: WaitingGamesEngine) {
    var best: Double? = null
    open val hint: String get() = ""
    open val keyboard: Boolean get() = false
    var flash: Flash? = null

    val W get() = e.W
    val H get() = e.H
    val C get() = e.C
    val R get() = e.R
    val cw get() = e.cw
    val chh get() = e.chh
    val playRows get() = e.R - 2

    open fun initGame() {}
    open fun resize() {}
    open fun pointer(px: Double, py: Double) {}
    open fun click(px: Double, py: Double) {}
    open fun key(k: String): Boolean = false
    open fun step(dt: Double, t: Double) {}
    open fun draw(p: WaitingGamesPainter, t: Double) {}

    fun flashDraw(p: WaitingGamesPainter, t: Double) {
        val f = flash ?: return
        if (t - f.t < 1.4) p.ptxt(W / 2, H * 0.28, f.txt, f.c, 1 - (t - f.t) / 1.4, 20.0)
    }
}

// ---------- Big Fish: your finger is a fish. Eat smaller fish, avoid bigger ones; eaten = respawn small.
private val MIRROR = mapOf('>' to '<', '<' to '>', '(' to ')', ')' to '(', '/' to '\\', '\\' to '/')
private fun flip(s: String): String = buildString { for (ch in s.reversed()) append(MIRROR[ch] ?: ch) }
private fun fishShape(n: Int): List<String> {
    val mid = "><" + "(".repeat(n) + "°>"
    if (n < 4) return listOf(mid)
    return listOf("  " + "_".repeat(n) + "  ", mid, "  " + "‾".repeat(n) + "  ")
}
private val FISH_COL = listOf("#fde68a", "#fbbf24", "#fb923c", "#f87171", "#e879f9", "#94a3b8", "#cbd5e1")

/**
 * Big Fish sizes (iOS FishRules, web fishSpawnSize). You start at 2 because you only eat strictly smaller
 * fish and the smallest fish is 1: at size 1 nothing was edible, so you could never grow.
 */
internal object FishRules {
    const val START_SIZE = 2
    const val MAX_SIZE = 7
    /** Bigger fish in the tank at once: small ones get eaten and big ones don't, so without a cap the tank fills with threats. */
    const val MAX_THREATS = 4

    /** A new fish's size for a player at [size]: 22% bigger than you (none at the top size), the rest smaller. */
    fun spawnSize(size: Int, r1: Double, r2: Double): Int {
        if (r1 < 0.22 && size < MAX_SIZE) return min(MAX_SIZE, size + 1 + floor(r2 * 3).toInt())
        return 1 + floor(r2 * max(1, size - 1)).toInt()
    }

    /** Eats at the fish's full outline plus a little; a one-row fish is 13 pt tall and small fish swim away. */
    fun bites(gapX: Double, gapY: Double, widths: Double, heights: Double): Boolean = gapX < widths / 2 && gapY < heights / 2 + 0.4

    /** Gets eaten only on a clear overlap. */
    fun caught(gapX: Double, gapY: Double, widths: Double, heights: Double): Boolean = gapX < widths / 2 - 1 && gapY < heights / 2 - 0.3
}

/** Space the top bar and score line take over a game, in dp. */
private const val HUD_BOTTOM = 76.0

private class FishGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Me(var x: Double, var y: Double, var tx: Double, var ty: Double, var size: Int, var xp: Int, var dir: Double, var inv: Double)
    class Npc(var x: Double, var y: Double, val v: Double, val dir: Double, val s: Int, val p: Double, var dead: Boolean = false)
    class Kelp(val x: Double, val h: Double, val p: Double)
    class Bub(val x: Double, val y: Double, val v: Double)
    class Pop(val x: Double, val y: Double, val t: Double, val txt: String)

    var me = Me(0.0, 0.0, 0.0, 0.0, FishRules.START_SIZE, 0, 1.0, 0.0)
    var pops = mutableListOf<Pop>()
    var npc = mutableListOf<Npc>()
    var kelp = listOf<Kelp>()
    var bub = listOf<Bub>()
    var score = 0

    override val hint get() = "Move your finger to swim. Eat smaller fish to grow; stay away from bigger ones, marked !. Get eaten and you start small again."

    override fun initGame() {
        reset(true)
        best = best ?: 0.0
        kelp = List(14) { Kelp(r01(), rnd(0.3, 0.75), rnd(0.0, TAU)) }
        bub = List(40) { Bub(r01(), r01(), rnd(1.5, 3.5)) }
    }

    fun reset(first: Boolean) {
        me = Me(C / 2, R / 2, C / 2, R / 2, FishRules.START_SIZE, 0, 1.0, 2.0)
        if (first) score = 0
        npc = mutableListOf()
        pops = mutableListOf()
        repeat(14) { spawn(true) }
    }

    fun spawn(any: Boolean) {
        val threats = npc.count { it.s > me.size }
        val s = FishRules.spawnSize(me.size, if (threats >= FishRules.MAX_THREATS) 1.0 else r01(), r01())
        val dir = if (r01() < 0.5) 1.0 else -1.0
        npc.add(
            Npc(
                x = if (any) rnd(0.0, C) else (if (dir > 0) -12.0 else C + 4), y = rnd(3.0, R - 4),
                v = rnd(3.0, 8.0) * (if (s > 5) 0.7 else 1.0) / sqrt(s.toDouble()), dir = dir, s = s, p = rnd(0.0, TAU),
            ),
        )
    }

    override fun pointer(px: Double, py: Double) { me.tx = px / cw; me.ty = py / chh }

    private fun sign(v: Double): Double = if (v > 0) 1.0 else if (v < 0) -1.0 else 0.0

    override fun step(dt: Double, t: Double) {
        val dx = me.tx - me.x
        val dy = me.ty - me.y
        me.x += dx * min(1.0, dt * 6); me.y += dy * min(1.0, dt * 6)
        if (abs(dx) > 0.3) me.dir = if (dx > 0) 1.0 else -1.0
        me.inv = max(0.0, me.inv - dt)
        val ms = fishShape(me.size)
        val mw = ms[min(1, ms.size - 1)].length.toDouble()
        val mh = ms.size.toDouble()
        for (f in npc) {
            f.x += f.v * f.dir * dt
            f.y += sin(t * 1.3 + f.p) * dt * 0.6
            val fx = f.x
            val fy = f.y
            val sh = fishShape(f.s)
            val fw = sh[min(1, sh.size - 1)].length.toDouble()
            val fh = sh.size.toDouble()
            // flee or chase a little
            val ddx = me.x - fx
            val ddy = me.y - fy
            val dist = hypot(ddx / 2, ddy)
            if (f.s < me.size) {
                if (dist < 5) f.y -= sign(ddy) * dt * 1.5
            } else if (f.s > me.size && dist < 10) {
                f.y += sign(ddy) * dt * 1.5
            }
            f.y = max(1.0, min(R - 3, f.y))
            val gapX = abs(me.x - (fx + fw / 2))
            val gapY = abs(me.y - (fy + fh / 2))
            if (!f.dead) {
                if (f.s < me.size && FishRules.bites(gapX, gapY, mw + fw, mh + fh)) {
                    f.dead = true
                    score += f.s * 10
                    me.xp += f.s
                    pops.add(Pop(fx, fy, t, "+${f.s * 10}"))
                    if (me.xp >= me.size * 2 && me.size < FishRules.MAX_SIZE) { me.size += 1; me.xp = 0; flash = Flash(t, "Bigger!", "#ffffff") }
                } else if (f.s > me.size && me.inv <= 0 && FishRules.caught(gapX, gapY, mw + fw, mh + fh)) {
                    best = max(best ?: 0.0, score.toDouble())
                    flash = Flash(t, "Eaten! Back to the start", "#ffffff")
                    score = 0
                    reset(false)
                    return
                }
            }
        }
        pops.removeAll { t - it.t >= 0.7 }
        npc.removeAll { it.dead || it.x <= -16 || it.x >= C + 16 }
        while (npc.size < 14) spawn(false)
        e.setHud(listOf("Score $score", "Size ${me.size} / 7", "Best ${max(best ?: 0.0, score.toDouble()).toInt()}"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#1d8aa0", "#0f5a70", "#062a38"))
        for (i in 0 until 5) {
            val fi = i.toDouble()
            val x = (fi * 0.23 + 0.05 + sin(t * 0.1 + fi) * 0.03) * C
            var y = 0.0
            while (y < R * 0.75) {
                if ((y.toInt() + i) % 2 == 0) p.txt(x + y * 0.35, y, "/", "#bff3ff", 0.06 * (1 - y / (R * 0.75)))
                y += 1
            }
        }
        for (b in bub) {
            val y = jmod(jmod(b.y * R - t * b.v, R) + R, R)
            p.txt(b.x * C + sin(t * 2 + b.x * 9), y, "o", "#bdf2ff", 0.35)
        }
        for (k in kelp) {
            val h = k.h * R * 0.5
            var i = 0.0
            while (i < h) {
                p.txt(k.x * C + sin(i * 0.2 + t * 1.1 + k.p) * (i / h) * 2.5, R - 1 - i, if (i.toInt() % 2 == 1) "(" else ")", "#3fae6a", 0.75)
                i += 1
            }
        }
        var x = 0.0
        while (x < C) { p.txt(x, R - 1, if (x.toInt() % 3 != 0) "_" else ".", "#c9a96e", 0.6); x += 1 }
        for (f in npc) {
            val sh = fishShape(f.s).map { if (f.dir > 0) it else flip(it) }
            val big = f.s > me.size
            sh.forEachIndexed { j, l -> p.txt(f.x, f.y + j, l, FISH_COL[f.s - 1], if (big) 0.95 else 0.85) }
            if (big) p.txt(f.x + (if (f.dir > 0) sh[0].length.toDouble() else -2.0), f.y, "!", "#ff6b6b", 0.9)
        }
        for (pop in pops) p.txt(pop.x, pop.y - (t - pop.t) * 4, pop.txt, "#fde68a", 1 - (t - pop.t) / 0.7)
        val sh = fishShape(me.size).map { if (me.dir > 0) it else flip(it) }
        val blink = me.inv > 0 && floor(t * 8).toInt() % 2 == 1
        if (!blink) {
            val w = sh[min(1, sh.size - 1)].length.toDouble()
            sh.forEachIndexed { j, l -> p.txt(me.x - w / 2, me.y - sh.size / 2.0 + j, l, "#ffffff", 1.0) }
            p.txt(me.x - 1, me.y - sh.size / 2.0 - 1.2, "you", "#ffffff", 0.55)
        }
        flash?.let { f -> if (t - f.t < 1.4) p.sans(W / 2, H * 0.28 - 16, f.txt, "#ffffff", 1 - (t - f.t) / 1.4, 20.0) }
    }
}

// ---------- Bot Race: type the words; your bots race you to the finish line.
private val WORDS = "deploy branch commit merge rebase revert stash cherry-pick checkout fetch push pull clone diff patch hotfix release tag fork agent token prompt context embedding model inference latency stream schema review cache socket widget kernel layout canvas thread signal module vector cursor sprite runtime shader lambda tensor router binary compiler linter bundler webpack vite docker container kubernetes cluster node pod ingress endpoint webhook payload header cookie session oauth jwt middleware handler callback promise async await closure iterator generator interface struct enum trait generic typescript javascript python rust golang swift kotlin react hooks state props reducer selector component render virtual effect suspense hydrate server client edge worker queue cron mutex deadlock thread-safe race-condition semaphore pipeline artifact build test mock stub fixture assert coverage regression flaky refactor legacy monorepo package dependency semver changelog migration index query join transaction rollback replica shard postgres redis sqlite graphql rest grpc json yaml toml regex unicode utf8 base64 hash checksum encrypt decrypt certificate localhost staging production rollout canary feature-flag config env secret vault terminal shell bash zsh vim tmux grep curl stdout stderr exit-code segfault null undefined nan overflow recursion stacktrace breakpoint debugger logger telemetry metrics heap stack garbage-collector pointer reference allocation buffer stream chunk throttle debounce memoize idempotent backoff retry".split(" ")
private val RACER_COLOURS = listOf("#E07A5F", "#2dd4bf", "#60A5FA", "#A78BFA")
private const val GOAL = 15

private class RaceGame(e: WaitingGamesEngine, bots: List<WaitingGameRacer>) : WaitingGame(e) {
    data class Racer(val name: String, val col: String, val me: Boolean, var pos: Double = 0.0, var rate: Double = 0.0, var jit: Double = 0.0)

    val lineup: List<Racer> = listOf(Racer("You", "#ffffff", true)) +
        bots.take(4).mapIndexed { i, r -> Racer(r.name, r.color ?: RACER_COLOURS[i], false) }
    var racers = listOf<Racer>()
    var done = mutableListOf<Int>()
    var state = "ready"
    var typed = ""
    var err = -10.0
    var words = listOf<String>()
    var idx = 0
    var t0 = 0.0
    var chars = 0
    var clock = 0.0

    override val hint get() = "Tap the game, then type. Each finished word moves your @ forward. Your bots race at their own pace."
    override val keyboard get() = true

    override fun initGame() { newRace() }

    fun newRace() {
        state = "ready"; done = mutableListOf(); typed = ""; err = -10.0
        words = List(GOAL) { WORDS.randomOrNull() ?: "deploy" }
        racers = lineup.map { it.copy(pos = 0.0, rate = if (it.me) 0.0 else rnd(0.42, 0.62), jit = rnd(0.0, 9.0)) }
        idx = 0; t0 = 0.0; chars = 0
        e.hideBanner()
        e.showBanner("Bot Race", "Type the word on screen. Each word moves you forward.\nStart typing to go.")
    }

    override fun click(px: Double, py: Double) { if (state == "done") newRace() }

    override fun key(k: String): Boolean {
        if (state == "done") { if (k == "Enter") newRace(); return false }
        if (k.length != 1 && k != "Backspace") return false
        if (state == "ready") { state = "go"; t0 = e.now; e.hideBanner() }
        val w = words[idx]
        if (k == "Backspace") { if (typed.isNotEmpty()) typed = typed.dropLast(1); return true }
        if (k == " ") return true
        val next = typed + k.lowercase()
        if (w.startsWith(next)) {
            typed = next; chars += 1
            if (next == w) {
                idx += 1; typed = ""
                racers[0].pos = idx.toDouble() / GOAL
                if (idx >= GOAL) finish(0)
            }
        } else {
            err = clock
        }
        return true
    }

    fun finish(r: Int) {
        if (r !in done) done.add(r)
        if (!(racers[r].me || done.size == racers.size)) return
        state = "done"
        val place = done.indexOf(0) + 1
        val secs = e.now - t0
        val wpm = (chars / 5.0 / (secs / 60)).toIntRounded()
        val ord = listOf("", "1st", "2nd", "3rd", "4th", "5th")
        e.showBanner(
            if (place == 1) "You won!" else if (place > 0) "You came ${ord[place]}" else "The bots won",
            "$wpm words per minute · Tap or press Go to race again",
        )
    }

    override fun step(dt: Double, t: Double) {
        clock = t
        if (state != "go") return
        for ((i, r) in racers.withIndex()) {
            if (r.me || r.pos >= 1) continue
            r.pos = min(1.0, r.pos + dt * r.rate / GOAL * (1 + 0.35 * sin(t * 1.7 + r.jit)))
            if (r.pos >= 1) finish(i)
        }
        val secs = e.now - t0
        e.setHud(listOf("Word ${min(idx + 1, GOAL)} / $GOAL", "${(chars / 5.0 / max(0.1, secs / 60)).toIntRounded()} wpm"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#16121f", "#221a33"))
        for (i in 0 until 60) {
            val fi = i.toDouble()
            p.txt(jmod(fi * 37, C), jmod(fi * 53, R), ".", "#c4b5fd", 0.15 + 0.15 * sin(t + fi))
        }
        // The word stays at the top, under the score line, so the keyboard can never cover it. The lanes fill what's left.
        val wordY = HUD_BOTTOM
        val top = ceil((wordY + e.FS * 4.8) / chh)
        val laneH = max(2.0, min(floor((R - top - 1) / racers.size), floor(R * 0.11)))
        val x0 = 12.0
        val x1 = C - 6
        val len = x1 - x0
        racers.forEachIndexed { i, r ->
            val y = top + i * laneH
            p.txt(1.0, y, r.name.padEnd(8).take(8), r.col, 0.9)
            var x = x0
            while (x < x1) { p.txt(x, y + 1, if (x.toInt() % 2 == 1) "-" else "·", "#6b5b8a", 0.35); x += 1 }
            var k = 0.0
            while (k < laneH - 1) { p.txt(x1, y + k, if (k.toInt() % 2 == 1) "▒" else "█", "#ffffff", 0.5); k += 1 }
            val px = x0 + r.pos * len
            for (j in 1 until 6) p.txt(px - j, y, "=", "#ffffff", 0.18 * (1 - j / 6.0) * (if (state == "go") 1 else 0))
            val bob = if (state == "go") kotlin.math.round(sin(t * 10 + i) * 0.4) else 0.0
            p.txt(px, y + bob, if (r.me) "@>" else "█>", r.col, 1.0)
            val place = done.indexOf(i)
            if (place >= 0) p.txt(x1 + 2, y, "#${place + 1}", r.col, 1.0)
        }
        // the word to type
        val w = if (idx < words.size) words[idx] else ""
        val size = e.FS * 2.4
        val big = WaitingGamesPainter.monoWidth(size, bold = true)
        val sx = W / 2 - w.length * big / 2
        val shaking = t - err < 0.25
        val shake = if (shaking) sin(t * 90) * 4 else 0.0
        w.forEachIndexed { i, ch ->
            val col = if (i < typed.length) "#F2A93B" else if (shaking && i == typed.length) "#f87171" else "#ece8ff"
            p.glyph(sx + i * big + shake, wordY, ch.toString(), col, size, bold = true)
        }
        val nx = words.drop(idx + 1).take(3).joinToString("  ")
        val nw = nx.length * cw
        p.glyph(W / 2 - nw / 2, wordY + e.FS * 3, nx, "#ece8ff", e.FS, bold = false, a = 0.5)
    }
}

// ---------- Token Catch: catch the falling tokens, let the bugs fall.
private class CatchGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Item(var x: Double, var y: Double, val v: Double, val bug: Boolean, val ch: String, var gone: Boolean = false)

    var items = mutableListOf<Item>()
    var x = 0.0
    var tx = 0.0
    var score = 0
    var lives = 3
    var combo = 0
    var next = 0.0
    var over = false

    override val hint get() = "Move your finger to slide the catcher. Catch the green tokens. Let the red bugs fall past."

    override fun initGame() {
        items = mutableListOf(); x = C / 2; tx = C / 2; score = 0; lives = 3; next = 0.0; over = false; combo = 0; flash = null
        e.hideBanner()
    }

    override fun pointer(px: Double, py: Double) { tx = px / cw }
    override fun click(px: Double, py: Double) { if (over) initGame() }

    override fun step(dt: Double, t: Double) {
        if (over) return
        x += (tx - x) * min(1.0, dt * 12)
        next -= dt
        if (next <= 0) {
            next = max(0.16, 0.5 - score * 0.006)
            val bug = r01() < 0.22
            val glyphs = "{}();<>=+"
            items.add(Item(floor(rnd(2.0, C - 3)), -1.0, rnd(7.0, 12.0) + score * 0.08, bug, if (bug) "ж" else glyphs[Random.nextInt(9)].toString()))
        }
        val py = playRows
        for (it in items) {
            it.y += it.v * dt
            if (!it.gone && it.y >= py - 0.6 && it.y <= py + 0.8 && abs(it.x - x) <= 4) {
                it.gone = true
                if (it.bug) {
                    lives -= 1; combo = 0
                    flash = Flash(t, "Bug caught. One life lost", "#f87171")
                    if (lives <= 0) { over = true; e.showBanner("Out of lives", "$score tokens caught · Tap to play again") }
                } else {
                    score += 1; combo += 1
                    if (combo % 10 == 0) flash = Flash(t, "$combo in a row", "#86efac")
                }
            }
        }
        items.removeAll { it.gone || it.y >= R + 1 }
        e.setHud(listOf("Tokens $score", "♥".repeat(max(0, lives)) + "♡".repeat(3 - max(0, lives))))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#04130b", "#062015"))
        var c = 0
        while (c < C) {
            val off = jmod(t * (4 + (c * 7 % 5)) + c * 13, R + 10)
            for (k in 0 until 6) {
                val code = 0x30a0 + ((c * 31 + k * 7 + floor(t * 3).toInt()) % 90)
                p.txt(c.toDouble(), floor(off) - k, String(Character.toChars(code)), "#22c55e", 0.12 * (1 - k / 6.0))
            }
            c += 3
        }
        for (it in items) p.txt(it.x, it.y, it.ch, if (it.bug) "#f87171" else "#86efac", 1.0)
        p.txt(kotlin.math.round(x) - 3, playRows, "[==A==]", "#ffffff", 1.0)
        flashDraw(p, t)
    }
}

// ---------- Meteor Dodge: fly through a meteor shower for as long as you can.
private class DodgeGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Meteor(var x: Double, var y: Double, val vx: Double, val vy: Double, val s: Double)

    var m = mutableListOf<Meteor>()
    var x = 0.0
    var y = 0.0
    var tx = 0.0
    var ty = 0.0
    var time = 0.0
    var next = 0.0
    var over = false

    override val hint get() = "Your finger flies the ship. Dodge the meteors. The shower gets thicker the longer you last."

    override fun initGame() {
        if (best == null) best = 0.0
        m = mutableListOf(); x = C / 2; y = R * 0.6; tx = x; ty = y; time = 0.0; over = false; next = 0.0
        e.hideBanner()
    }

    override fun pointer(px: Double, py: Double) { tx = px / cw; ty = py / chh }
    override fun click(px: Double, py: Double) { if (over) initGame() }

    override fun step(dt: Double, t: Double) {
        if (over) return
        time += dt
        x += (tx - x) * min(1.0, dt * 10); y += (ty - y) * min(1.0, dt * 10)
        next -= dt
        if (next <= 0) {
            next = max(0.1, 0.55 - time * 0.012)
            m.add(Meteor(rnd(C * 0.1, C * 1.3), -2.0, -rnd(5.0, 10.0), rnd(6.0, 11.0) + time * 0.15, if (r01() < 0.25) 2.0 else 1.0))
        }
        for (mt in m) {
            mt.x += mt.vx * dt; mt.y += mt.vy * dt
            if (hypot((mt.x - x) * cw, (mt.y - y) * chh) < mt.s * 6 + 7) {
                over = true
                best = max(best ?: 0.0, time)
                e.showBanner("Hit!", String.format(Locale.ROOT, "You lasted %.1f s · Best %.1f s · Tap to fly again", time, best ?: 0.0))
                break
            }
        }
        m.removeAll { it.y >= R + 3 || it.x <= -8 }
        e.setHud(listOf(String.format(Locale.ROOT, "%.1f s", time), String.format(Locale.ROOT, "Best %.1f s", max(best ?: 0.0, if (over) 0.0 else time))))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#05060f", "#0d1030", "#1a1240"))
        for (i in 0 until 90) {
            val fi = i.toDouble()
            p.txt(jmod(fi * 97, C), jmod(fi * 61, R), if (i % 7 != 0) "." else "+", "#e0e7ff", 0.2 + 0.2 * sin(t * 2 + fi))
        }
        for (mt in m) {
            val n = hypot(mt.vx, mt.vy)
            for (k in 6 downTo 1) {
                val fk = k.toDouble()
                p.txt(mt.x - mt.vx / n * fk * 1.4, mt.y - mt.vy / n * fk * 0.7, if (k < 3) "*" else "·", "#fb923c", 0.7 * (1 - fk / 7))
            }
            p.txt(mt.x, mt.y, if (mt.s > 1) "@" else "o", "#fde68a", 1.0)
        }
        if (!over || floor(t * 6).toInt() % 2 == 1) {
            p.txt(x - 1, y, "<A>", "#ffffff", 1.0)
            p.txt(x, y + 1, if (floor(t * 12).toInt() % 2 == 1) "'" else ".", "#7dd3fc", 0.8)
        }
    }
}

// ---------- Bug Bash: bugs pop out of your files for a moment; tap them before they hide.
private val FILES = listOf("api.ts", "App.tsx", "auth.rs", "schema.sql", "main.go", "index.css", "router.ts", "utils.py", "Dockerfile")

private class BugBashGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Hole(val f: String, var from: Double = 0.0, var until: Double = 0.0, var gold: Boolean = false, var hit: Double = 0.0)
    class Splat(val x: Double, val y: Double, val t: Double, val txt: String)
    class Layout(val size: Double, val gap: Double, val ox: Double, val oy: Double)

    var holes = listOf<Hole>()
    var score = 0
    var left = 30.0
    var state = "ready"
    var next = 0.0
    var splat = mutableListOf<Splat>()
    var clock = 0.0

    override val hint get() = "Bugs pop out of your files for a moment. Tap one before it hides. Gold bugs are worth 3. You have 30 seconds."

    override fun initGame() {
        if (best == null) best = 0.0
        holes = FILES.map { Hole(it) }
        score = 0; left = 30.0; state = "ready"; next = 0.0; splat = mutableListOf()
        e.showBanner("Bug Bash", "Bugs pop out of your files. Tap them before they hide.\nTap to start. You have 30 seconds.")
    }

    fun layout(): Layout {
        val gap = 14.0
        val top = HUD_BOTTOM
        val bottom = 12.0
        val size = min((W - gap * 4) / 3, (H - top - bottom - gap * 2) / 3)
        return Layout(size, gap, (W - (size * 3 + gap * 2)) / 2, top + ((H - top - bottom) - (size * 3 + gap * 2)) / 2)
    }

    /** A hole's square: x, y, side. */
    fun rect(i: Int): Triple<Double, Double, Double> {
        val l = layout()
        return Triple(l.ox + (i % 3) * (l.size + l.gap), l.oy + (i / 3) * (l.size + l.gap), l.size)
    }

    override fun click(px: Double, py: Double) {
        val t = clock
        if (state != "go") { initGame(); state = "go"; e.hideBanner(); return }
        holes.forEachIndexed { i, h ->
            val (rx, ry, rs) = rect(i)
            if (px >= rx && px <= rx + rs && py >= ry && py <= ry + rs && h.until > t && h.hit == 0.0) {
                h.hit = t; h.until = t + 0.35
                val pts = if (h.gold) 3 else 1
                score += pts
                splat.add(Splat(px, py, t, "+$pts"))
            }
        }
    }

    override fun step(dt: Double, t: Double) {
        clock = t
        if (state != "go") return
        left -= dt
        if (left <= 0) {
            left = 0.0; state = "done"
            best = max(best ?: 0.0, score.toDouble())
            e.showBanner("Time", "$score bugs squashed · Best ${(best ?: 0.0).toInt()} · Tap to play again")
            return
        }
        next -= dt
        if (next <= 0) {
            val speed = 1 - (30 - left) / 30 * 0.45
            next = rnd(0.35, 0.7) * speed
            val free = holes.filter { it.until < t }
            free.randomOrNull()?.let { h ->
                h.from = t; h.until = t + rnd(0.8, 1.2) * speed + 0.2
                h.gold = r01() < 0.12; h.hit = 0.0
            }
        }
        splat.removeAll { t - it.t >= 0.7 }
        e.setHud(listOf("Squashed $score", "${ceil(left).toInt()} s", "Best ${(best ?: 0.0).toInt()}"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#0f1115", "#171a21"))
        holes.forEachIndexed { i, h ->
            val (rx, ry, rs) = rect(i)
            p.roundRect(rx, ry, rs, rs, 14.0, Ink.color("#232733"))
            for (k in 0 until 5) {
                if (16 + k * 14.0 + 5 >= rs - 30) continue
                p.rect(rx + 14, ry + 16 + k * 14.0, (rs - 28) * ((k * 37 + i * 13) % 60 + 30) / 100.0, 5.0, Ink.color("#2f3545"))
            }
            p.ptxt(rx + rs / 2, ry + rs - 16, h.f, "#9aa3b5", 1.0, 12.0)
            if (h.until > t) {
                val rise = max(0.0, minOf(1.0, (t - h.from) / 0.12, (h.until - t) / 0.12))
                val fs = rs * 0.16
                val cy = ry + rs * 0.45 + (1 - rise) * rs * 0.3
                val col = if (h.hit > 0) "#a3e635" else if (h.gold) "#fde047" else "#f87171"
                val lines = if (h.hit > 0) listOf("\\ | /", "- * -", "/ | \\") else listOf(" \\ / ", "(•ж•)", " /|\\ ")
                p.clipped(rx, ry, rs, rs - 30) {
                    lines.forEachIndexed { j, l -> p.ptxt(rx + rs / 2, cy + (j - 1) * fs * 1.05, l, col, rise, fs) }
                }
            }
        }
        for (s in splat) p.ptxt(s.x, s.y - 30 * (t - s.t), s.txt, "#a3e635", 1 - (t - s.t) / 0.7, 18.0)
    }
}

// ---------- Stack Deploy: drop each release on the stack; only the part that lands stays.
private class StackGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Block(val x: Double, val w: Double)
    class Debris(val x: Double, var y: Double, val w: Double, var vy: Double, val c: Int)
    class Mover(var x: Double, val lo: Double, val hi: Double, val w: Double, var dir: Double, val v: Double)

    val bh = 26.0
    var base = 0.0
    var blocks = mutableListOf<Block>()
    var debris = mutableListOf<Debris>()
    var mov = Mover(0.0, 0.0, 0.0, 0.0, 1.0, 150.0)
    var over = false
    var cam = 0.0
    var streak = 0
    var clock = 0.0

    override val hint get() = "Tap to drop the release. Only the part that lands on the stack stays. Line it up exactly for a Perfect; three in a row widens the stack again."

    override fun initGame() {
        if (best == null) best = 0.0
        base = H - 96
        val w = min(260.0, W * 0.5)
        blocks = mutableListOf(Block((W - w) / 2, w))
        debris = mutableListOf(); over = false; cam = 0.0; flash = null; streak = 0
        e.hideBanner()
        spawn()
    }

    fun spawn() {
        val top = blocks.last()
        val left = r01() < 0.5
        val span = top.w * 0.85
        mov = Mover(if (left) top.x - span else top.x + span, top.x - span, top.x + span, top.w, if (left) 1.0 else -1.0, 150 + blocks.size * 9.0)
    }

    fun yOf(i: Int): Double = base - (i + 1) * bh + cam

    /** hsl(h 70% 62%) as hue / saturation / brightness (iOS Color(hue:saturation:brightness:)). */
    fun col(i: Int): Int = android.graphics.Color.HSVToColor(floatArrayOf(((18 + i * 14) % 360).toFloat(), 0.6f, 0.886f))

    fun drop(t: Double) {
        if (over) { initGame(); return }
        val top = blocks.last()
        val m = mov
        var l = max(m.x, top.x)
        var r = min(m.x + m.w, top.x + top.w)
        val n = blocks.size
        val y = yOf(n)
        if (r - l < 8) {
            over = true
            debris.add(Debris(m.x, y, m.w, 0.0, col(n)))
            best = max(best ?: 0.0, (n - 1).toDouble())
            e.showBanner("It fell over", "${n - 1} releases shipped · Best ${(best ?: 0.0).toInt()} · Tap to build again")
            return
        }
        if (abs(m.x - top.x) < 6) {
            l = top.x; r = top.x + top.w
            streak += 1
            flash = Flash(t, if (streak > 1) "Perfect ×$streak" else "Perfect!", "#fde68a")
            if (streak > 2) r = min(W * 0.6 + l, r + 8)
        } else {
            streak = 0
            debris.add(Debris(if (m.x < top.x) m.x else r, y, m.w - (r - l), 0.0, col(n)))
        }
        blocks.add(Block(l, r - l))
        spawn()
    }

    override fun resize() { base = H - 96 }
    override fun click(px: Double, py: Double) { drop(clock) }
    override fun key(k: String): Boolean {
        if (k == " " || k == "Enter") { drop(clock); return true }
        return false
    }

    override fun step(dt: Double, t: Double) {
        clock = t
        val target = max(0.0, (blocks.size + 1) * bh - H * 0.5)
        cam += (target - cam) * min(1.0, dt * 5)
        if (!over) {
            mov.x += mov.dir * mov.v * dt
            if (mov.dir < 0 && mov.x < mov.lo) mov.dir = 1.0
            if (mov.dir > 0 && mov.x > mov.hi) mov.dir = -1.0
        }
        for (d in debris) { d.vy += 900 * dt; d.y += d.vy * dt }
        debris.removeAll { it.y >= H + 60 }
        e.setHud(listOf("Shipped ${blocks.size - 1}", "Best ${max(best ?: 0.0, (blocks.size - 1).toDouble()).toInt()}"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#14121c", "#1f1a2e"))
        for (i in 0 until 80) {
            val fi = i.toDouble()
            p.txt(jmod(fi * 47, C), jmod(fi * 71, R), "·", "#c4b5fd", 0.12 + 0.08 * sin(t + fi))
        }
        val labelInk = Ink.alpha(android.graphics.Color.BLACK, 0.42)
        fun blk(x: Double, y: Double, w: Double, c: Int, label: String, a: Double = 1.0) {
            p.rect(x, y, w, bh - 2, Ink.alpha(c, a))
            if (label.isNotEmpty() && w > 48) p.label(x + 8, y + bh / 2 - 1, label, 11.0, labelInk)
        }
        blocks.forEachIndexed { i, b ->
            val y = yOf(i)
            if (y > -30 && y < H) blk(b.x, y, b.w, if (i > 0) col(i) else Ink.color("#3f3a52"), if (i > 0) "v0.$i" else "main")
        }
        if (!over) blk(mov.x, yOf(blocks.size), mov.w, col(blocks.size), "v0.${blocks.size}")
        for (d in debris) blk(d.x, d.y, d.w, d.c, "", 0.7)
        flashDraw(p, t)
    }
}

// ---------- Context Snake: your thread grows with every token. Fill the context window and the thread ends.
private class SnakeGame(e: WaitingGamesEngine) : WaitingGame(e) {
    data class Cell(val x: Int, val y: Int)

    var gx = 0
    var gy = 0
    var s = mutableListOf<Cell>()
    var d = Cell(1, 0)
    var nd = Cell(1, 0)
    var acc = 0.0
    var over = false
    var score = 0
    var food = Cell(0, 0)
    /** The first playable row, clear of the top bar and score line. */
    var top = 2

    override val hint get() = "Tap the side you want to turn to (or use arrow keys). Each token makes the thread longer. Hit the edge or yourself and the context is full."

    override fun initGame() {
        gx = floor(C / 2).toInt(); gy = playRows.toInt()
        top = ceil(HUD_BOTTOM / chh).toInt()
        val sx = gx / 3
        val sy = (top + gy) / 2
        s = mutableListOf(Cell(sx, sy), Cell(sx - 1, sy), Cell(sx - 2, sy))
        d = Cell(1, 0); nd = d; acc = 0.0; over = false; score = 0
        food = place()
        e.hideBanner()
    }

    fun place(): Cell {
        if (gx <= 2 || gy <= top + 1) return Cell(1, top)
        repeat(500) {
            val f = Cell(floor(rnd(1.0, (gx - 1).toDouble())).toInt(), floor(rnd(top.toDouble(), (gy - 1).toDouble())).toInt())
            if (f !in s) return f
        }
        return Cell(1, top)
    }

    override fun key(k: String): Boolean {
        if (over && (k == "Enter" || k == " ")) { initGame(); return true }
        val v = when (k) {
            "ArrowUp", "w" -> 0 to -1
            "ArrowDown", "s" -> 0 to 1
            "ArrowLeft", "a" -> -1 to 0
            "ArrowRight", "d" -> 1 to 0
            else -> return false
        }
        turn(v.first, v.second)
        return true
    }

    fun turn(x: Int, y: Int) {
        if (x == -d.x && y == -d.y) return
        nd = Cell(x, y)
    }

    override fun click(px: Double, py: Double) {
        if (over) { initGame(); return }
        val h = s[0]
        val dx = px / cw / 2 - h.x
        val dy = py / chh - h.y
        if (d.x != 0) turn(0, if (dy > 0) 1 else -1) else turn(if (dx > 0) 1 else -1, 0)
    }

    override fun step(dt: Double, t: Double) {
        if (over) return
        acc += dt
        val sp = max(0.055, 0.12 - score * 0.002)
        while (acc > sp) {
            acc -= sp
            d = nd
            val h = s[0]
            val n = Cell(h.x + d.x, h.y + d.y)
            if (n.x < 0 || n.y < top || n.x >= gx || n.y >= gy || n in s) {
                over = true
                e.showBanner("Context window full", "${s.size * 4}k tokens · Tap to start a new thread")
                return
            }
            s.add(0, n)
            if (n == food) { score += 1; food = place() } else s.removeAt(s.lastIndex)
        }
        e.setHud(listOf("Context ${s.size * 4}k / 200k", "Tokens $score"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#111318", "#1b1e26"))
        for (x in 0 until max(0, gx)) {
            p.txt(x * 2.0, (top - 1).toDouble(), "··", "#52525b", 0.6)
            p.txt(x * 2.0, gy.toDouble(), "··", "#52525b", 0.6)
        }
        s.forEachIndexed { i, c ->
            p.txt(c.x * 2.0, c.y.toDouble(), "██", if (i > 0) "#f0a58a" else "#D97757", if (i > 0) max(0.35, 1 - i.toDouble() / s.size) else 1.0)
        }
        p.txt(food.x * 2.0, food.y.toDouble(), "◆", "#F2A93B", 0.6 + 0.4 * sin(t * 6))
    }
}

// ---------- Bot Memory: flip cards and match your bots' faces.
private val FACES = listOf(
    "(◕‿◕)" to "#c2410c", "[•_•]" to "#0f766e", "<o_o>" to "#1d4ed8", "{^_^}" to "#6d28d9",
    "(⌐■_■)" to "#b45309", "[@_@]" to "#be185d", "(-_-)" to "#475569", "\\ò_ó/" to "#15803d",
)

private class MemoryGame(e: WaitingGamesEngine) : WaitingGame(e) {
    class Card(val f: Int, var up: Boolean = false, var done: Boolean = false, var flip: Double = 0.0)
    class Layout(val cwid: Double, val ch: Double, val gap: Double, val ox: Double, val oy: Double)

    var cards = listOf<Card>()
    var open = mutableListOf<Int>()
    var moves = 0
    var lock = 0.0
    var t0 = 0.0
    var won = false
    var clock = 0.0

    override val hint get() = "Tap a card to flip it, then find its pair. Match all eight bot faces in as few moves as you can."

    override fun initGame() {
        cards = (0 until 16).map { it shr 1 }.shuffled().map { Card(it) }
        open = mutableListOf(); moves = 0; lock = 0.0; t0 = 0.0; won = false
        e.hideBanner()
    }

    fun layout(): Layout {
        val gap = 10.0
        val top = HUD_BOTTOM
        val bottom = 12.0
        val cwid = min((W - gap * 5) / 4, ((H - top - bottom - gap * 3) / 4) * 1.3)
        val ch = cwid / 1.3
        return Layout(cwid, ch, gap, (W - (cwid * 4 + gap * 3)) / 2, top + ((H - top - bottom) - (ch * 4 + gap * 3)) / 2)
    }

    override fun click(px: Double, py: Double) {
        if (won) { initGame(); return }
        if (lock > 0) return
        val l = layout()
        val c = floor((px - l.ox) / (l.cwid + l.gap)).toInt()
        val r = floor((py - l.oy) / (l.ch + l.gap)).toInt()
        if (c < 0 || c > 3 || r < 0 || r > 3) return
        val i = r * 4 + c
        if (cards[i].up || cards[i].done) return
        if (t0 == 0.0) t0 = clock
        cards[i].up = true
        open.add(i)
        if (open.size == 2) {
            moves += 1
            val a = open[0]
            val b = open[1]
            if (cards[a].f == cards[b].f) {
                cards[a].done = true; cards[b].done = true
                open = mutableListOf()
                if (cards.all { it.done }) {
                    won = true
                    e.showBanner("All matched", "$moves moves · ${(clock - t0).toIntRounded()} s · Tap to deal again")
                }
            } else {
                lock = clock + 0.8
            }
        }
    }

    override fun step(dt: Double, t: Double) {
        clock = t
        if (lock > 0 && t > lock) {
            for (i in open) cards[i].up = false
            open = mutableListOf(); lock = 0.0
        }
        for (k in cards) k.flip += ((if (k.up || k.done) 1.0 else 0.0) - k.flip) * min(1.0, dt * 12)
        e.setHud(listOf("Moves $moves", "Pairs ${cards.count { it.done } / 2} / 8"))
    }

    override fun draw(p: WaitingGamesPainter, t: Double) {
        p.bg(listOf("#1a1625", "#251d33"))
        for (i in 0 until 70) {
            val fi = i.toDouble()
            p.txt(jmod(fi * 41, C), jmod(fi * 67, R), "·", "#c4b5fd", 0.15 + 0.1 * sin(t + fi))
        }
        val l = layout()
        cards.forEachIndexed { i, k ->
            val x = l.ox + (i % 4) * (l.cwid + l.gap)
            val y = l.oy + (i shr 2) * (l.ch + l.gap)
            val save = p.canvas.save()
            p.canvas.translate((x + l.cwid / 2).toFloat(), (y + l.ch / 2).toFloat())
            p.canvas.scale(max(0.04, abs(1 - 2 * k.flip)).toFloat(), 1f)
            p.opacity = if (k.done) 0.6 else 1.0
            val face = k.flip > 0.5
            p.roundRect(-l.cwid / 2, -l.ch / 2, l.cwid, l.ch, 10.0, p.withAlpha(if (face) "#f4f1fb" else "#3b2f55", 1.0))
            if (face) p.ptxt(0.0, 0.0, FACES[k.f].first, FACES[k.f].second, 1.0, kotlin.math.round(l.ch * 0.24))
            else p.ptxt(0.0, 0.0, "A://", "#8b7bb0", 1.0, kotlin.math.round(l.ch * 0.16))
            p.opacity = 1.0
            p.canvas.restoreToCount(save)
        }
    }
}
