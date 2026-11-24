package com.allternit.core.design

import android.os.Build
import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlurEffect
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.TileMode
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlin.math.PI
import kotlin.math.floor
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

/*
 * Docking block title (Android port of the approved DockTitle, A2 block of
 * reports/mobile-revamp-2026-11-20/draft.md). Numbers follow
 * handoffs/mobile-refresh-2026-11-23/CONTRACT.md exactly; do not tune them here.
 * All distances are dp, all scroll offsets are dp.
 */

/** Frozen numbers from CONTRACT.md. */
object DockSpec {
    const val DOCK = 170f
    const val BH = 22f
    const val SH = 11f
    const val LG = BH * 1.36f
    const val TITLE_MAX_WIDTH = 316f
    const val BAND_PAD = 22f
    const val LEFT = 14f
    const val TOP = 104f
    const val TOP_FREEWEIGHTS = 112f
    const val BAR_CENTER = 71f
    const val BAR_CENTER_FREEWEIGHTS = 75f
    const val FLASH_MS = 900
    const val FLASH_STAGGER_MS = 12f
    const val GLYPH_PITCH = 6
    const val GLYPH_SPACE = 3

    /** Letter spring (stiffness, damping) for letter [i]: 520 - (i % 9) * 25, damping 28. */
    fun letterStiffness(i: Int): Float = 520f - (i % 9) * 25f
    const val LETTER_DAMPING = 28f
}

/** One frame of the dock, derived from the scroll offset [y] (dp). */
data class DockFrame(
    val t: Float,
    val landed: Int,
    val docked: Boolean,
    val bandFactor: Float,
    val descOpacity: Float,
    val descBlur: Float,
    val descDrift: Float,
    /** Reduced motion only: the title has crossfaded into the bar (y >= DOCK / 2). */
    val reducedDone: Boolean,
)

fun dockEase(v: Float): Float {
    if (v < 0.5f) return 4f * v * v * v
    val u = -2f * v + 2f
    return 1f - u * u * u / 2f
}

fun dockFrame(y: Float, n: Int, reduced: Boolean): DockFrame {
    val yy = max(0f, y)
    val t = (yy / DockSpec.DOCK).coerceIn(0f, 1f)
    if (reduced) {
        val done = yy >= DockSpec.DOCK / 2f
        return DockFrame(
            t = t,
            landed = if (done) n else 0,
            docked = done,
            bandFactor = if (done) 0f else 1f,
            descOpacity = if (done) 0f else 1f,
            descBlur = 0f,
            descDrift = 0f,
            reducedDone = done,
        )
    }
    val f = min(1f, yy / (DockSpec.DOCK * 0.6f))
    return DockFrame(
        t = t,
        landed = min(n, floor(t * (n + 0.6f) + 0.0001f).toInt()),
        docked = t >= 1f,
        bandFactor = 1f - dockEase(((t - 0.82f) / 0.18f).coerceIn(0f, 1f)),
        descOpacity = 1f - f,
        descBlur = f * 3f,
        descDrift = yy * 0.35f,
        reducedDone = false,
    )
}

/** Letter [letter]'s flight progress (0 = in the page, 1 = docked in the bar). */
fun dockLetterProgress(t: Float, n: Int, letter: Int): Float =
    dockEase((t * (n + 0.6f) - letter).coerceIn(0f, 1f))

/** Columns the character [ch] advances (glyph 6, punctuation width + 1, space 3, undrawn 0), as web `layoutBlockText`. */
private fun advanceOf(ch: Char): Int = when {
    ch == ' ' -> DockSpec.GLYPH_SPACE
    BlockGlyphs.glyphs.containsKey(ch) -> DockSpec.GLYPH_PITCH
    else -> BlockGlyphs.punct[ch]?.let { it.w + 1 } ?: 0
}

/** The cells of one drawn character at column [col], or null when it has no glyph or punctuation entry. */
private fun cellsOf(ch: Char): List<Pair<Int, Int>>? = BlockGlyphs.glyphs[ch] ?: BlockGlyphs.punct[ch]?.cells

/** Columns a string takes: 6 per glyph, punctuation width + 1, 3 per space, minus the trailing gap. */
fun blockCols(s: String): Int {
    var col = 0
    for (ch in s.uppercase()) col += advanceOf(ch)
    return max(0, col - 1)
}

fun wrapLines(text: String, bh: Float = DockSpec.BH, maxWidth: Float = DockSpec.TITLE_MAX_WIDTH): List<String> {
    val lines = mutableListOf<String>()
    var cur = ""
    for (w in text.split(" ")) {
        val t = if (cur.isNotEmpty()) "$cur $w" else w
        if (cur.isNotEmpty() && blockCols(t) * bh / 5f > maxWidth) {
            lines += cur
            cur = w
        } else cur = t
    }
    lines += cur
    return lines
}

/** A cell of the big (wrapped) title. [bx] is in columns inside its line, [li] the line, [letter] the letter index. */
class BigCell(val bx: Int, val by: Int, val li: Int, val letter: Int)

/** A cell of the docked (single row) title, columns and rows. */
class SmallCell(val x: Int, val y: Int)

class TitleCells(
    val big: List<BigCell>,
    val small: List<SmallCell>,
    /** Letters (glyphs, not spaces). */
    val n: Int,
    /** Columns of the single-line docked title. */
    val cols: Int,
    val lines: Int,
)

/**
 * big[i] pairs with small[i] (same glyph order). Glyph and punctuation tables are the generated [BlockGlyphs],
 * advanced the way web `layoutBlockText` does. Only drawn characters count as letters.
 */
fun titleCells(text: String): TitleCells {
    val lines = wrapLines(text)
    val big = ArrayList<BigCell>()
    val small = ArrayList<SmallCell>()
    var letter = 0
    lines.forEachIndexed { li, ln ->
        var col = 0
        for (ch in ln.uppercase()) {
            if (ch == ' ') {
                col += DockSpec.GLYPH_SPACE
                continue
            }
            val cs = cellsOf(ch) ?: continue
            cs.forEach { (x, y) -> big += BigCell(x + col, y, li, letter) }
            col += advanceOf(ch)
            letter++
        }
    }
    var col = 0
    for (ch in text.uppercase()) {
        if (ch == ' ') {
            col += DockSpec.GLYPH_SPACE
            continue
        }
        val cs = cellsOf(ch) ?: continue
        cs.forEach { (x, y) -> small += SmallCell(x + col, y) }
        col += advanceOf(ch)
    }
    return TitleCells(big, small, letter, max(0, col - 1), lines.size)
}

/** True when the system animator duration scale is 0 (Android's reduce-motion switch). */
@Composable
fun rememberSystemReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
}

/**
 * A scroll offset provider in dp for [DockingBlockTitle] and [dockDescription]. When the content does not
 * overflow (nothing to scroll in either direction) the offset stays 0, so a short page's title does not dock.
 */
@Composable
fun rememberDockOffset(state: ScrollState): () -> Float {
    val d = LocalDensity.current.density
    return remember(state, d) {
        { if (state.canScrollForward || state.canScrollBackward) state.value / d else 0f }
    }
}

@Composable
fun rememberDockOffset(state: LazyListState): () -> Float {
    val d = LocalDensity.current.density
    return remember(state, d) {
        {
            when {
                !state.canScrollForward && !state.canScrollBackward -> 0f
                state.firstVisibleItemIndex > 0 -> DockSpec.DOCK * 2f
                else -> state.firstVisibleItemScrollOffset / d
            }
        }
    }
}

/**
 * The page scroll in dp that the nearest [ProvideDockScroll] gave, or null when no screen
 * provides one. Shared headers (LibraryHeader, SPage, ACI headers) dock only when this is
 * non-null, so a screen that never opts in keeps today's header. A provided offset of 0
 * (page at rest) means "docking page, title big", which is not the same as "no provider".
 */
val LocalDockOffset = compositionLocalOf<(() -> Float)?> { null }

/** Gives headers inside [content] (LibraryHeader, SPage, ACI headers) the page scroll [offset] in dp. */
@Composable
fun ProvideDockScroll(offset: () -> Float, content: @Composable () -> Unit) {
    CompositionLocalProvider(LocalDockOffset provides offset, content = content)
}

@Composable
fun ProvideDockScroll(state: ScrollState, content: @Composable () -> Unit) =
    ProvideDockScroll(rememberDockOffset(state), content)

@Composable
fun ProvideDockScroll(state: LazyListState, content: @Composable () -> Unit) =
    ProvideDockScroll(rememberDockOffset(state), content)

@Composable
fun DockingBlockTitle(
    text: String,
    scrollState: ScrollState,
    modifier: Modifier = Modifier,
    barCenter: Dp = DockSpec.BAR_CENTER.dp,
    top: Dp = DockSpec.TOP.dp,
    reduceMotion: Boolean = rememberSystemReduceMotion(),
    markLabel: String? = null,
) = DockingBlockTitle(text, rememberDockOffset(scrollState), barCenter, modifier, top, reduceMotion, markLabel)

@Composable
fun DockingBlockTitle(
    text: String,
    listState: LazyListState,
    modifier: Modifier = Modifier,
    barCenter: Dp = DockSpec.BAR_CENTER.dp,
    top: Dp = DockSpec.TOP.dp,
    reduceMotion: Boolean = rememberSystemReduceMotion(),
    markLabel: String? = null,
) = DockingBlockTitle(text, rememberDockOffset(listState), barCenter, modifier, top, reduceMotion, markLabel)

/**
 * The page title that moves into the top bar one letter per scroll step. Draw it as an overlay over
 * the scrolling page ([modifier] fills the page); it never takes touches. One Canvas draws the white
 * band, the hairline and every cell; per-letter springs and the scroll offset are read in the draw
 * phase only, so scrolling does not recompose.
 *
 * [scrollOffset] is the page scroll in dp. [barCenter] is the docked title's vertical centre (71 dp,
 * Freeweights 75 dp); the hairline sits at the bottom of the bar, 2 x [barCenter].
 * [markLabel] is the accessibility label (default [text]). Short pages (no overflow) never dock: use
 * [rememberDockOffset] (or [ProvideDockScroll]) so the offset stays 0.
 * Haptics: [Haptics.Kind.Tap] each time a letter lands, [Haptics.Kind.Success] when the title docks
 * (the closest shared kinds to selection and soft).
 */
@Composable
fun DockingBlockTitle(
    text: String,
    scrollOffset: () -> Float,
    barCenter: Dp = DockSpec.BAR_CENTER.dp,
    modifier: Modifier = Modifier,
    top: Dp = DockSpec.TOP.dp,
    reduceMotion: Boolean = rememberSystemReduceMotion(),
    markLabel: String? = null,
) {
    val cells = remember(text) { titleCells(text) }
    val n = cells.n
    val colors = AllternitTheme.colors
    val ink = colors.textPrimary
    val bg = colors.bgPrimary
    val hairline = colors.textPrimary.copy(alpha = 0.09f)
    val view = LocalView.current

    // One spring per letter (not per cell), driven by the scroll target.
    val letterAnims = remember(text) { List(max(n, 0)) { Animatable(0f) } }
    letterAnims.forEachIndexed { l, anim ->
        LaunchedEffect(text, l, reduceMotion) {
            if (reduceMotion) {
                anim.snapTo(0f)
                return@LaunchedEffect
            }
            val k = DockSpec.letterStiffness(l)
            val spec = spring<Float>(
                dampingRatio = DockSpec.LETTER_DAMPING / (2f * sqrt(k)),
                stiffness = k,
            )
            snapshotFlowOf { dockLetterProgress(dockFrame(scrollOffset(), n, false).t, n, l) }
                .distinctUntilChanged()
                .collectLatest { anim.animateTo(it, spec) }
        }
    }

    // Haptics when the landed count changes.
    LaunchedEffect(text, reduceMotion) {
        var prev = dockFrame(scrollOffset(), n, reduceMotion).landed
        snapshotFlowOf { dockFrame(scrollOffset(), n, reduceMotion).landed }
            .distinctUntilChanged()
            .collectLatest { landed ->
                if (landed != prev) {
                    if (landed == n) Haptics.perform(view, Haptics.Kind.Success)
                    else if (landed > prev) Haptics.perform(view, Haptics.Kind.Tap)
                    prev = landed
                }
            }
    }

    // Coral flash after dock: one clock, per-letter-column delay.
    val maxDelayMs = cells.cols * DockSpec.FLASH_STAGGER_MS
    val flashTotalMs = DockSpec.FLASH_MS + maxDelayMs
    val flash = remember(text) { Animatable(0f) }
    LaunchedEffect(text, reduceMotion) {
        snapshotFlowOf { !reduceMotion && dockFrame(scrollOffset(), n, false).docked }
            .distinctUntilChanged()
            .collectLatest { docked ->
                flash.snapTo(0f)
                if (docked) flash.animateTo(1f, tween(flashTotalMs.toInt(), easing = LinearEasing))
            }
    }

    // Reduced motion: crossfade the page title and the docked title at y >= DOCK / 2.
    val doneState = remember(scrollOffset) { derivedStateOf { reduceMotion && scrollOffset() >= DockSpec.DOCK / 2f } }
    val fade = animateFloatAsState(if (doneState.value) 1f else 0f, tween(150), label = "dock-crossfade")

    Canvas(
        modifier
            .fillMaxSize()
            .semantics {
                heading()
                contentDescription = markLabel ?: text
            },
    ) {
        val d = density
        val y = max(0f, scrollOffset())
        val frame = dockFrame(y, n, reduceMotion)
        val viewW = size.width / d
        val bh = DockSpec.BH
        val sh = DockSpec.SH
        val b = bh / 5f
        val s = sh / 5f
        val topDp = top.value
        val hc = barCenter.value
        val sx0 = viewW / 2f - cells.cols * s / 2f
        val sy0 = hc - sh / 2f

        // Band that holds the waiting letters; collapses at t .82 -> 1.
        val titleH = cells.lines * bh + (cells.lines - 1) * (bh * 1.36f - bh)
        val bandH = (titleH + DockSpec.BAND_PAD) * frame.bandFactor
        if (bandH > 0.5f) {
            val bandTop = (topDp - 8f) * d
            val bandPx = bandH * d
            drawRect(
                brush = Brush.verticalGradient(
                    colorStops = arrayOf(0f to bg, 0.8f to bg, 1f to bg.copy(alpha = 0f)),
                    startY = bandTop,
                    endY = bandTop + bandPx,
                ),
                topLeft = Offset(0f, bandTop),
                size = Size(size.width, bandPx),
            )
        }

        // Hairline under the bar, appears as the title docks.
        val hairA = 1f - frame.bandFactor
        if (hairA > 0f) {
            drawRect(
                color = hairline.copy(alpha = hairline.alpha * hairA),
                topLeft = Offset(0f, hc * 2f * d),
                size = Size(size.width, d),
            )
        }

        val shiftY = max(0f, y - DockSpec.DOCK)
        val flashClockMs = flash.value * flashTotalMs

        for (i in cells.big.indices) {
            val c = cells.big[i]
            val sm = cells.small[i]
            val x0 = DockSpec.LEFT + c.bx * b
            val y0 = topDp - shiftY + c.li * (bh * 1.36f) + c.by * b
            val x1 = sx0 + sm.x * s
            val y1 = sy0 + sm.y * s

            if (reduceMotion) {
                val a = fade.value
                if (a < 1f) cellRect(x0, y0, b, 0f, ink.copy(alpha = ink.alpha * (1f - a)), d)
                if (a > 0f) cellRect(x1, y1, s, 0f, ink.copy(alpha = ink.alpha * a), d)
                continue
            }

            val pk = letterAnims[c.letter].value.coerceIn(0f, 1f)
            val sz = b + (s - b) * pk
            val arc = sin(PI.toFloat() * pk)
            val hop = -arc * (8f + (c.letter % 3) * 3f)
            val rot = arc * (if (c.letter % 2 == 1) 18f else -18f)
            val x = x0 + (x1 - x0) * pk
            val yy = y0 + (y1 - y0) * pk + hop
            val color = when {
                frame.docked -> {
                    val local = (flashClockMs - sm.x * DockSpec.FLASH_STAGGER_MS) / DockSpec.FLASH_MS
                    if (local > 0f && local < 1f) lerp(ink, CORAL, sin(PI.toFloat() * local)) else ink
                }
                pk > 0.001f && pk < 0.999f -> CORAL
                else -> ink
            }
            cellRect(x, yy, sz, rot, color, d)
        }
    }
}

private val CORAL = PixelCoral

/** One block: 85% of the cell, 7.5% inset, corner .18 of its size, rotated about its centre. */
private fun DrawScope.cellRect(x: Float, y: Float, sz: Float, deg: Float, color: Color, d: Float) {
    val side = sz * 0.85f * d
    val left = (x + sz * 0.075f) * d
    val top = (y + sz * 0.075f) * d
    val r = sz * 0.18f * d
    if (deg == 0f) {
        drawRoundRect(color, Offset(left, top), Size(side, side), CornerRadius(r, r))
    } else {
        rotate(deg, Offset(left + side / 2f, top + side / 2f)) {
            drawRoundRect(color, Offset(left, top), Size(side, side), CornerRadius(r, r))
        }
    }
}

private fun <T> snapshotFlowOf(block: () -> T) = androidx.compose.runtime.snapshotFlow(block)

/**
 * The page description under a docking title: fades, blurs (API 31+) and drifts down as the page
 * scrolls (`f = min(1, y / (DOCK * .6))`, opacity 1-f, blur f*3, drift y*.35). Under reduced motion it
 * only crossfades at y >= DOCK / 2.
 */
@Composable
fun Modifier.dockDescription(
    scrollOffset: () -> Float,
    reduceMotion: Boolean = rememberSystemReduceMotion(),
): Modifier {
    val done = remember(scrollOffset) { derivedStateOf { scrollOffset() >= DockSpec.DOCK / 2f } }
    val reducedAlpha = animateFloatAsState(if (reduceMotion && done.value) 0f else 1f, tween(150), label = "dock-desc")
    return this.graphicsLayer {
        if (reduceMotion) {
            alpha = reducedAlpha.value
        } else {
            val frame = dockFrame(scrollOffset(), 1, false)
            alpha = frame.descOpacity
            translationY = frame.descDrift * density
            renderEffect = if (frame.descBlur > 0.01f && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                BlurEffect(frame.descBlur * density, frame.descBlur * density, TileMode.Decal)
            } else null
        }
    }
}

/** Cells and total columns of [text] on one row (the glyph table is [BlockGlyphs]). */
fun blockLayout(text: String): Pair<List<Pair<Int, Int>>, Int> {
    val cells = ArrayList<Pair<Int, Int>>()
    var col = 0
    for (ch in text.uppercase()) {
        if (ch == ' ') {
            col += DockSpec.GLYPH_SPACE
            continue
        }
        val g = cellsOf(ch) ?: continue
        cells += g.map { (x, y) -> (x + col) to y }
        col += advanceOf(ch)
    }
    return cells to max(0, col - 1)
}

/** A static block-letter title on one row, [height] tall. The label is the plain text. */
@Composable
fun BlockText(
    text: String,
    modifier: Modifier = Modifier,
    height: Dp = DockSpec.BH.dp,
    color: Color = AllternitTheme.colors.textPrimary,
) {
    val (cells, cols) = remember(text) { blockLayout(text) }
    val width = height * (cols.toFloat() / BlockGlyphs.ROWS)
    Canvas(modifier.size(width, height).semantics { contentDescription = text }) {
        val scale = size.height / (BlockGlyphs.ROWS * BlockGlyphs.CELL)
        val off = (BlockGlyphs.CELL - BlockGlyphs.BLOCK) / 2f
        val side = BlockGlyphs.BLOCK * scale
        val r = BlockGlyphs.RADIUS * scale
        cells.forEach { (cx, cy) ->
            drawRoundRect(
                color,
                Offset((cx * BlockGlyphs.CELL + off) * scale, (cy * BlockGlyphs.CELL + off) * scale),
                Size(side, side),
                CornerRadius(r, r),
            )
        }
    }
}

/** A static title that wraps at 316 dp the way the docking title does (line pitch BH x 1.36). */
@Composable
fun BlockTitle(
    text: String,
    modifier: Modifier = Modifier,
    height: Dp = DockSpec.BH.dp,
    color: Color = AllternitTheme.colors.textPrimary,
) {
    val lines = remember(text, height) { wrapLines(text, height.value, DockSpec.TITLE_MAX_WIDTH) }
    Column(
        modifier.semantics { contentDescription = text },
        verticalArrangement = Arrangement.spacedBy(height * 0.36f),
    ) {
        lines.forEach { BlockText(it, height = height, color = color) }
    }
}
