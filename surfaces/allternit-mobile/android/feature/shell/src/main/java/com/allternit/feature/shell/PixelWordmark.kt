package com.allternit.feature.shell

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.allternit.core.design.AllternitTheme

/** The A core block's coral (#D97757), the same on every mark. */
internal val PixelCoral = Color(0xFFD97757)

/** MatrixLogo.svg's sand blocks (#D4B08C). */
internal val MatrixSand = Color(0xFFD4B08C)

/**
 * The pixel A://TERNIT wordmark with an optional pixel-letter suffix (iOS PixelWordmark,
 * web AProtocolWordmark): the A:// mark in cols 0–14, letters at pitch 6 from col 16,
 * 10-unit cells with 8.5 blocks and 1.5 corner radius, the A core block in coral.
 */
@Composable
internal fun PixelWordmark(height: Dp = 16.dp, suffix: String = "", ink: Color = AllternitTheme.colors.textPrimary, modifier: Modifier = Modifier) {
    val (letters, cols) = PixelGlyphs.layout("TERNIT $suffix".trim())
    val width = height * (cols.toFloat() / PixelGlyphs.ROWS)
    val label = "Allternit $suffix".trim()
    Canvas(modifier.size(width, height).semantics { contentDescription = label }) {
        val scale = size.height / (PixelGlyphs.ROWS * PixelGlyphs.CELL)
        PixelGlyphs.markCells.forEach { pixel(it, scale, ink) }
        letters.forEach { pixel(it, scale, ink) }
        pixel(PixelGlyphs.coreCell, scale, PixelCoral)
    }
}

/** The condensed A:// mark (iOS AProtocolMark): the wordmark's A:// with its coral core, 3:1. */
@Composable
internal fun AProtocolMark(height: Dp = 14.dp, ink: Color = AllternitTheme.colors.textPrimary, modifier: Modifier = Modifier) {
    Canvas(modifier.size(height * 3, height).semantics { contentDescription = "A://" }) {
        val scale = size.height / (PixelGlyphs.ROWS * PixelGlyphs.CELL)
        PixelGlyphs.markCells.forEach { pixel(it, scale, ink) }
        pixel(PixelGlyphs.coreCell, scale, PixelCoral)
    }
}

/**
 * The Allternit matrix mark (iOS Assets MatrixLogo.svg): the pixel A on an 18-unit pitch,
 * 14-unit blocks, coral core. [ink] defaults to the sand of the model chip; the drawer's
 * Phones button draws it in the text colour (iOS AllternitMarkView).
 */
@Composable
internal fun MatrixLogo(size: Dp, ink: Color = MatrixSand, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) {
        val s = this.size.width / 100f
        val r = CornerRadius(2f * s, 2f * s)
        fun block(x: Int, y: Int, color: Color) =
            drawRoundRect(color, topLeft = Offset(x * s, y * s), size = Size(14f * s, 14f * s), cornerRadius = r)
        listOf(43 to 7, 25 to 25, 61 to 25, 7 to 43, 25 to 43, 61 to 43, 79 to 43, 7 to 61, 79 to 61, 7 to 79, 79 to 79)
            .forEach { (x, y) -> block(x, y, ink) }
        block(43, 43, PixelCoral)
    }
}

/**
 * The pixel GIZZI CODE lockup the Code drawer shows (iOS GizziCodeWordmark, without the
 * Gizzi head mark): 10-unit cells, 8.5 blocks.
 */
@Composable
internal fun GizziCodeWordmark(height: Dp = 15.dp, ink: Color = AllternitTheme.colors.textPrimary) {
    val rows = PixelGlyphs.gizziCodeRows
    val width = height * (rows[0].length / 5f)
    Canvas(Modifier.size(width, height).semantics { contentDescription = "gizzi code" }) {
        val scale = size.height / 50f
        rows.forEachIndexed { y, row ->
            row.forEachIndexed { x, cell -> if (cell == '#') pixel(x to y, scale, ink) }
        }
    }
}

private fun DrawScope.pixel(cell: Pair<Int, Int>, scale: Float, color: Color) {
    val off = (PixelGlyphs.CELL - PixelGlyphs.BLOCK) / 2f
    drawRoundRect(
        color = color,
        topLeft = Offset((cell.first * PixelGlyphs.CELL + off) * scale, (cell.second * PixelGlyphs.CELL + off) * scale),
        size = Size(PixelGlyphs.BLOCK * scale, PixelGlyphs.BLOCK * scale),
        cornerRadius = CornerRadius(1.5f * scale, 1.5f * scale),
    )
}

/** The grid and glyph table, copied from iOS PixelWordmark.swift. */
internal object PixelGlyphs {
    const val CELL = 10f
    const val BLOCK = 8.5f
    const val ROWS = 5
    private const val TERNIT_START = 16
    private const val PITCH = 6
    private const val SPACE_COLS = 3

    val markCells: List<Pair<Int, Int>> = listOf(
        2 to 0, 1 to 1, 3 to 1, 0 to 2, 1 to 2, 3 to 2, 4 to 2, 0 to 3, 4 to 3, 0 to 4, 4 to 4,
        6 to 1, 6 to 3,
        8 to 3, 8 to 4, 9 to 2, 10 to 0, 10 to 1,
        12 to 3, 12 to 4, 13 to 2, 14 to 0, 14 to 1,
    )
    val coreCell: Pair<Int, Int> = 2 to 2

    val gizziCodeRows = listOf(
        ".###..###.#####.#####.###....####..###..####..#####",
        "#......#.....#.....#...#....#.....#...#.#...#.#....",
        "#.###..#....#.....#....#....#.....#...#.#...#.####.",
        "#...#..#...#.....#.....#....#.....#...#.#...#.#....",
        ".###..###.#####.#####.###....####..###..####..#####",
    )

    private fun g(vararg cells: Int): List<Pair<Int, Int>> = cells.toList().chunked(2).map { it[0] to it[1] }

    private val glyphs: Map<Char, List<Pair<Int, Int>>> = mapOf(
        'T' to g(0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 2, 1, 2, 2, 2, 3, 2, 4),
        'E' to g(0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 0, 1, 0, 2, 1, 2, 2, 2, 3, 2, 0, 3, 0, 4, 1, 4, 2, 4, 3, 4, 4, 4),
        'R' to g(0, 0, 1, 0, 2, 0, 3, 0, 0, 1, 4, 1, 0, 2, 1, 2, 2, 2, 3, 2, 0, 3, 2, 3, 0, 4, 3, 4, 4, 4),
        'N' to g(0, 0, 0, 1, 1, 1, 0, 2, 2, 2, 0, 3, 3, 3, 0, 4, 4, 0, 4, 1, 4, 2, 4, 3, 4, 4),
        'I' to g(1, 0, 2, 0, 3, 0, 2, 1, 2, 2, 2, 3, 1, 4, 2, 4, 3, 4),
        'S' to g(0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 0, 1, 0, 2, 1, 2, 2, 2, 3, 2, 4, 2, 4, 3, 0, 4, 1, 4, 2, 4, 3, 4, 4, 4),
        'O' to g(0, 0, 0, 1, 0, 2, 0, 3, 0, 4, 4, 0, 4, 1, 4, 2, 4, 3, 4, 4, 1, 0, 2, 0, 3, 0, 1, 4, 2, 4, 3, 4),
        'A' to g(2, 0, 1, 1, 3, 1, 0, 2, 1, 2, 2, 2, 3, 2, 4, 2, 0, 3, 4, 3, 0, 4, 4, 4),
        'B' to g(0, 0, 0, 1, 0, 2, 0, 3, 0, 4, 1, 0, 2, 0, 3, 0, 4, 1, 1, 2, 2, 2, 3, 2, 4, 3, 1, 4, 2, 4, 3, 4),
        'C' to g(0, 0, 1, 0, 2, 0, 3, 0, 4, 0, 0, 1, 0, 2, 0, 3, 0, 4, 1, 4, 2, 4, 3, 4, 4, 4),
    )

    /** Every inked letter cell and the total column count (iOS PixelWordmark.layout). */
    fun layout(word: String): Pair<List<Pair<Int, Int>>, Int> {
        val cells = mutableListOf<Pair<Int, Int>>()
        var col = TERNIT_START
        for (ch in word.uppercase()) {
            if (ch == ' ') {
                col += SPACE_COLS
                continue
            }
            glyphs[ch]?.let { glyph -> cells += glyph.map { (x, y) -> (x + col) to y } }
            col += PITCH
        }
        return cells to (col - 1)
    }
}
