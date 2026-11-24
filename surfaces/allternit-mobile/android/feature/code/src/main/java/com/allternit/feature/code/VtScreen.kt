package com.allternit.feature.code

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration

// MARK: - Terminal screen (a VT220/xterm subset that vim, htop, less, tmux and shells need)

/** iOS TerminalPalette. */
object TerminalColors {
    val background = Color(0xFF0D0F12)
    val foreground = Color(0xFFE4E6EA)
    val secondary = Color(0xFF9AA0A8)
    val strip = Color(0xFF14171B)
    val key = Color(0xFF1D2126)
    val keyPressed = Color(0xFF2A2F36)
    val line = Color(0xFF23272D)
    // The coding-agent composer on the terminal (iOS CodeInk(terminal: true), GizziCodeWordmark.coral, Theme.status*).
    val tertiary = Color(0xB39AA0A8)
    val keyBorder = Color(0xFF2C3137)
    val coral = Color(0xFFD97757)
    val success = Color(0xFF4ADE80)
    val warning = Color(0xFFFBBF24)
}

/** One cell's look; colors are palette indexes (0-255), packed 24-bit RGB (|0x1000000), or -1 for default. */
data class CellStyle(
    val fg: Int = -1, val bg: Int = -1, val bold: Boolean = false, val underline: Boolean = false, val inverse: Boolean = false,
    val italic: Boolean = false, val dim: Boolean = false, val strike: Boolean = false,
)

/**
 * The terminal emulator behind the Code shell and a Terminal bot's `screen` frames (iOS draws with
 * SwiftTerm). It keeps a main grid with scrollback and an alternate grid (vim, htop, less), scroll
 * regions (DECSTBM), origin and autowrap modes, application cursor keys and keypad, bracketed paste,
 * cursor visibility, insert/delete of lines and characters, scroll up/down, the DEC line-drawing
 * charset, SGR 16/256/truecolour (also the `38:2::r:g:b` colon form), the usual CSI cursor, erase and
 * report sequences (DSR, DA) and OSC titles. Answers the app must send back go to [onReply].
 */
class VtScreen(var cols: Int = 80, var rows: Int = 24) {
    private class Grid(var cells: Array<CharArray>, var styles: Array<Array<CellStyle>>)

    private fun newGrid(c: Int, r: Int) = Grid(Array(r) { CharArray(c) { ' ' } }, Array(r) { Array(c) { CellStyle() } })

    private val mainGrid = newGrid(cols, rows)
    private val altGrid = newGrid(cols, rows)
    private var g = mainGrid
    private val scrollback = ArrayList<Pair<String, List<CellStyle>>>()

    private var cx = 0
    private var cy = 0
    private var style = CellStyle()
    private var top = 0
    private var bot = rows - 1
    private var wrapPending = false
    private var savedX = 0
    private var savedY = 0
    private var savedStyle = CellStyle()
    private var mainSavedX = 0
    private var mainSavedY = 0
    private var mainSavedStyle = CellStyle()
    private var lastChar = ' '

    private var parse = GROUND
    private val buf = StringBuilder()
    private var charsetSlot = '('
    private var g0Line = false
    private var g1Line = false
    private var shifted = false

    // Modes the UI and the key bar read.
    var appCursor = false; private set
    var appKeypad = false; private set
    var bracketedPaste = false; private set
    var cursorVisible = true; private set
    var altScreen = false; private set
    var title = ""; private set
    private var originMode = false
    private var autowrap = true
    private var insertMode = false

    /** Bytes the terminal answers with (cursor position reports, device attributes). */
    var onReply: ((String) -> Unit)? = null

    /** Bumped on every change so Compose can re-read. */
    var version = 0
        private set

    fun resize(newCols: Int, newRows: Int) {
        if (newCols < 2 || newRows < 2 || (newCols == cols && newRows == rows)) return
        // Shrinking the main screen pushes the rows above the cursor into scrollback.
        if (newRows < rows && !altScreen) {
            var drop = maxOf(0, cy - newRows + 1)
            while (drop-- > 0) {
                scrollback.add(String(mainGrid.cells[0]) to mainGrid.styles[0].toList())
                for (y in 1 until rows) { mainGrid.cells[y - 1] = mainGrid.cells[y]; mainGrid.styles[y - 1] = mainGrid.styles[y] }
                mainGrid.cells[rows - 1] = CharArray(cols) { ' ' }
                mainGrid.styles[rows - 1] = Array(cols) { CellStyle() }
                cy--
            }
        }
        for (grid in listOf(mainGrid, altGrid)) {
            val nc = Array(newRows) { CharArray(newCols) { ' ' } }
            val ns = Array(newRows) { Array(newCols) { CellStyle() } }
            for (y in 0 until minOf(rows, newRows)) for (x in 0 until minOf(cols, newCols)) { nc[y][x] = grid.cells[y][x]; ns[y][x] = grid.styles[y][x] }
            grid.cells = nc; grid.styles = ns
        }
        cols = newCols; rows = newRows
        top = 0; bot = rows - 1
        cx = cx.coerceIn(0, cols - 1); cy = cy.coerceIn(0, rows - 1)
        wrapPending = false
        version++
    }

    /** Full reset (RIS). */
    fun reset() {
        for (grid in listOf(mainGrid, altGrid)) {
            grid.cells = Array(rows) { CharArray(cols) { ' ' } }
            grid.styles = Array(rows) { Array(cols) { CellStyle() } }
        }
        g = mainGrid; altScreen = false
        scrollback.clear()
        cx = 0; cy = 0; style = CellStyle(); top = 0; bot = rows - 1; wrapPending = false
        appCursor = false; appKeypad = false; bracketedPaste = false; cursorVisible = true
        originMode = false; autowrap = true; insertMode = false
        g0Line = false; g1Line = false; shifted = false
        parse = GROUND; buf.setLength(0)
        version++
    }

    fun feed(text: String) {
        for (ch in text) put(ch)
        version++
    }

    // MARK: parser

    private fun put(ch: Char) {
        when (parse) {
            ESC -> escape(ch)
            CSI -> when {
                ch in '@'..'~' -> { parse = GROUND; csi(buf.toString(), ch) }
                ch == '\u001b' -> { parse = ESC }
                ch < ' ' -> control(ch)
                else -> { buf.append(ch); if (buf.length > 80) parse = GROUND }
            }
            OSC -> when {
                ch == '\u0007' -> { parse = GROUND; osc(buf.toString()) }
                ch == '\u001b' -> parse = OSC_ESC
                else -> { if (buf.length < 512) buf.append(ch) }
            }
            OSC_ESC -> {
                parse = GROUND
                osc(buf.toString())
                if (ch != '\\') { parse = ESC; put(ch) }
            }
            STR -> if (ch == '\u001b') parse = STR_ESC
            STR_ESC -> parse = if (ch == '\\') GROUND else STR
            CHARSET -> {
                parse = GROUND
                if (charsetSlot == '(') g0Line = ch == '0' else if (charsetSlot == ')') g1Line = ch == '0'
            }
            SKIP1 -> parse = GROUND
            else -> if (ch < ' ' || ch == '\u007f') control(ch) else print(ch)
        }
    }

    private fun control(ch: Char) {
        when (ch) {
            '\u001b' -> parse = ESC
            '\r' -> { cx = 0; wrapPending = false }
            '\n', '\u000b', '\u000c' -> { index(); wrapPending = false }
            '\b' -> { if (cx > 0) cx--; wrapPending = false }
            '\t' -> { cx = minOf(cols - 1, (cx / 8 + 1) * 8); wrapPending = false }
            '\u000e' -> shifted = true
            '\u000f' -> shifted = false
            else -> Unit
        }
    }

    private fun escape(ch: Char) {
        buf.setLength(0)
        when (ch) {
            '[' -> parse = CSI
            ']' -> parse = OSC
            'P', 'X', '^', '_' -> parse = STR
            '(', ')', '*', '+' -> { charsetSlot = ch; parse = CHARSET }
            '#', '%', ' ' -> parse = SKIP1
            '7' -> { saveCursor(); parse = GROUND }
            '8' -> { restoreCursor(); parse = GROUND }
            'D' -> { index(); parse = GROUND }
            'E' -> { cx = 0; index(); parse = GROUND }
            'M' -> { reverseIndex(); parse = GROUND }
            'c' -> reset()
            '=' -> { appKeypad = true; parse = GROUND }
            '>' -> { appKeypad = false; parse = GROUND }
            '\u001b' -> Unit
            else -> parse = GROUND
        }
    }

    private fun osc(s: String) {
        val semi = s.indexOf(';')
        if (semi < 0) return
        when (s.substring(0, semi)) { "0", "2" -> title = s.substring(semi + 1).take(200) }
    }

    private fun print(raw: Char) {
        var ch = raw
        if ((if (shifted) g1Line else g0Line) && ch in '`'..'~') ch = LINE_DRAWING[ch - '`']
        if (wrapPending && autowrap) { cx = 0; index() }
        wrapPending = false
        if (insertMode) insertChars(1)
        g.cells[cy][cx] = ch
        g.styles[cy][cx] = style
        lastChar = ch
        if (cx >= cols - 1) { if (autowrap) wrapPending = true } else cx++
    }

    // MARK: motion and scrolling

    private fun index() {
        if (cy == bot) scrollUp(top, bot, 1, toScrollback = top == 0 && bot == rows - 1 && !altScreen)
        else if (cy < rows - 1) cy++
    }

    private fun reverseIndex() {
        if (cy == top) scrollDown(top, bot, 1) else if (cy > 0) cy--
    }

    private fun blank() = CellStyle(bg = style.bg)

    private fun scrollUp(from: Int, to: Int, n: Int, toScrollback: Boolean) {
        repeat(minOf(n, to - from + 1)) {
            if (toScrollback) {
                scrollback.add(String(g.cells[from]) to g.styles[from].toList())
                if (scrollback.size > 2000) scrollback.removeAt(0)
            }
            for (y in from + 1..to) { g.cells[y - 1] = g.cells[y]; g.styles[y - 1] = g.styles[y] }
            val b = blank()
            g.cells[to] = CharArray(cols) { ' ' }
            g.styles[to] = Array(cols) { b }
        }
    }

    private fun scrollDown(from: Int, to: Int, n: Int) {
        repeat(minOf(n, to - from + 1)) {
            for (y in to downTo from + 1) { g.cells[y] = g.cells[y - 1]; g.styles[y] = g.styles[y - 1] }
            val b = blank()
            g.cells[from] = CharArray(cols) { ' ' }
            g.styles[from] = Array(cols) { b }
        }
    }

    private fun saveCursor() { savedX = cx; savedY = cy; savedStyle = style }
    private fun restoreCursor() { cx = savedX.coerceIn(0, cols - 1); cy = savedY.coerceIn(0, rows - 1); style = savedStyle; wrapPending = false }

    private fun moveTo(row: Int, col: Int) {
        val base = if (originMode) top else 0
        val limit = if (originMode) bot else rows - 1
        cy = (base + row).coerceIn(base, limit)
        cx = col.coerceIn(0, cols - 1)
        wrapPending = false
    }

    private fun setAlt(on: Boolean, saveRestore: Boolean) {
        if (on == altScreen) return
        if (on) {
            if (saveRestore) { mainSavedX = cx; mainSavedY = cy; mainSavedStyle = style }
            g = altGrid; altScreen = true
            for (y in 0 until rows) { g.cells[y] = CharArray(cols) { ' ' }; g.styles[y] = Array(cols) { CellStyle() } }
            top = 0; bot = rows - 1
        } else {
            g = mainGrid; altScreen = false
            if (saveRestore) { cx = mainSavedX.coerceIn(0, cols - 1); cy = mainSavedY.coerceIn(0, rows - 1); style = mainSavedStyle }
            top = 0; bot = rows - 1
        }
        wrapPending = false
    }

    // MARK: CSI

    private fun csi(params: String, final: Char) {
        var body = params
        val prefix = body.firstOrNull()?.takeIf { it in "?<=>" }
        if (prefix != null) body = body.drop(1)
        val inter = body.lastOrNull()?.takeIf { it in ' '..'/' }
        if (inter != null) {
            if (inter == '!' && final == 'p') softReset()
            return
        }
        val p = body.split(';').map { it.toIntOrNull() }
        fun n(i: Int, d: Int) = (p.getOrNull(i) ?: d).let { if (it == 0 && d == 1) 1 else it }
        if (prefix == '?' && (final == 'h' || final == 'l')) { for (v in p) v?.let { privateMode(it, final == 'h') }; return }
        if (prefix == '>' ) { if (final == 'c') reply("\u001b[>0;0;0c"); return }
        if (prefix != null) return
        when (final) {
            'A' -> { wrapPending = false; cy = maxOf(if (cy >= top) top else 0, cy - n(0, 1)) }
            'B', 'e' -> { wrapPending = false; cy = minOf(if (cy <= bot) bot else rows - 1, cy + n(0, 1)) }
            'C', 'a' -> { wrapPending = false; cx = minOf(cols - 1, cx + n(0, 1)) }
            'D' -> { wrapPending = false; cx = maxOf(0, cx - n(0, 1)) }
            'E' -> { cy = minOf(if (cy <= bot) bot else rows - 1, cy + n(0, 1)); cx = 0; wrapPending = false }
            'F' -> { cy = maxOf(if (cy >= top) top else 0, cy - n(0, 1)); cx = 0; wrapPending = false }
            'G', '`' -> { cx = (n(0, 1) - 1).coerceIn(0, cols - 1); wrapPending = false }
            'H', 'f' -> moveTo(n(0, 1) - 1, n(1, 1) - 1)
            'd' -> moveTo(n(0, 1) - 1, cx)
            'J' -> when (p.getOrNull(0) ?: 0) {
                0 -> { eraseLine(cy, cx, cols); for (y in cy + 1 until rows) eraseLine(y, 0, cols) }
                1 -> { for (y in 0 until cy) eraseLine(y, 0, cols); eraseLine(cy, 0, cx + 1) }
                else -> for (y in 0 until rows) eraseLine(y, 0, cols)
            }
            'K' -> when (p.getOrNull(0) ?: 0) { 0 -> eraseLine(cy, cx, cols); 1 -> eraseLine(cy, 0, cx + 1); else -> eraseLine(cy, 0, cols) }
            'L' -> if (cy in top..bot) { scrollDown(cy, bot, n(0, 1)); cx = 0; wrapPending = false }
            'M' -> if (cy in top..bot) { scrollUp(cy, bot, n(0, 1), toScrollback = false); cx = 0; wrapPending = false }
            '@' -> insertChars(n(0, 1))
            'P' -> deleteChars(n(0, 1))
            'X' -> eraseLine(cy, cx, minOf(cols, cx + n(0, 1)))
            'S' -> scrollUp(top, bot, n(0, 1), toScrollback = top == 0 && bot == rows - 1 && !altScreen)
            'T' -> scrollDown(top, bot, n(0, 1))
            'b' -> repeat(minOf(n(0, 1), 500)) { print(lastChar) }
            'r' -> {
                val t = n(0, 1) - 1
                val b = (p.getOrNull(1)?.takeIf { it > 0 } ?: rows) - 1
                if (t < b && b < rows) { top = t; bot = b } else { top = 0; bot = rows - 1 }
                moveTo(0, 0)
            }
            's' -> saveCursor()
            'u' -> restoreCursor()
            'h', 'l' -> for (v in p) if (v == 4) insertMode = final == 'h'
            'n' -> when (p.getOrNull(0)) {
                5 -> reply("\u001b[0n")
                6 -> reply("\u001b[${(if (originMode) cy - top else cy) + 1};${cx + 1}R")
            }
            'c' -> if ((p.getOrNull(0) ?: 0) == 0) reply("\u001b[?1;2c")
            'm' -> sgr(body)
        }
    }

    private fun reply(s: String) { onReply?.invoke(s) }

    private fun softReset() {
        style = CellStyle(); top = 0; bot = rows - 1; originMode = false; autowrap = true; insertMode = false
        appCursor = false; appKeypad = false; cursorVisible = true; wrapPending = false
    }

    private fun privateMode(mode: Int, on: Boolean) {
        when (mode) {
            1 -> appCursor = on
            6 -> { originMode = on; moveTo(0, 0) }
            7 -> autowrap = on
            25 -> cursorVisible = on
            47, 1047 -> setAlt(on, saveRestore = false)
            1049 -> setAlt(on, saveRestore = true)
            2004 -> bracketedPaste = on
            else -> Unit // mouse reporting (1000-1006), focus events and the rest are accepted and ignored
        }
    }

    private fun eraseLine(y: Int, from: Int, to: Int) {
        val b = blank()
        for (x in from.coerceAtLeast(0) until to.coerceAtMost(cols)) { g.cells[y][x] = ' '; g.styles[y][x] = b }
    }

    private fun insertChars(k: Int) {
        val n = minOf(k, cols - cx)
        val b = blank()
        for (x in cols - 1 downTo cx + n) { g.cells[cy][x] = g.cells[cy][x - n]; g.styles[cy][x] = g.styles[cy][x - n] }
        for (x in cx until cx + n) { g.cells[cy][x] = ' '; g.styles[cy][x] = b }
    }

    private fun deleteChars(k: Int) {
        val n = minOf(k, cols - cx)
        val b = blank()
        for (x in cx until cols) {
            val src = x + n
            g.cells[cy][x] = if (src < cols) g.cells[cy][src] else ' '
            g.styles[cy][x] = if (src < cols) g.styles[cy][src] else b
        }
    }

    /** SGR with `;` and `:` separators: 16, 256 and truecolour, bold, dim, italic, underline, inverse, strike. */
    private fun sgr(body: String) {
        val codes = ArrayList<Int>()
        val tokens = if (body.isEmpty()) listOf("0") else body.split(';')
        for (t in tokens) {
            if (':' in t) {
                val sub = t.split(':').map { it.toIntOrNull() ?: 0 }.toMutableList()
                // 38:2:<colorspace>:r:g:b carries an extra colorspace slot.
                if ((sub[0] == 38 || sub[0] == 48) && sub.getOrNull(1) == 2 && sub.size >= 6) sub.removeAt(2)
                codes.addAll(sub)
            } else codes.add(t.toIntOrNull() ?: 0)
        }
        var i = 0
        while (i < codes.size) {
            val c = codes[i]
            when {
                c == 0 -> style = CellStyle()
                c == 1 -> style = style.copy(bold = true)
                c == 2 -> style = style.copy(dim = true)
                c == 3 -> style = style.copy(italic = true)
                c == 4 -> style = style.copy(underline = true)
                c == 7 -> style = style.copy(inverse = true)
                c == 9 -> style = style.copy(strike = true)
                c == 22 -> style = style.copy(bold = false, dim = false)
                c == 23 -> style = style.copy(italic = false)
                c == 24 -> style = style.copy(underline = false)
                c == 27 -> style = style.copy(inverse = false)
                c == 29 -> style = style.copy(strike = false)
                c in 30..37 -> style = style.copy(fg = c - 30)
                c == 39 -> style = style.copy(fg = -1)
                c in 40..47 -> style = style.copy(bg = c - 40)
                c == 49 -> style = style.copy(bg = -1)
                c in 90..97 -> style = style.copy(fg = c - 90 + 8)
                c in 100..107 -> style = style.copy(bg = c - 100 + 8)
                c == 38 || c == 48 -> {
                    val color = when (codes.getOrNull(i + 1)) {
                        5 -> { val v = codes.getOrNull(i + 2) ?: 0; i += 2; v.coerceIn(0, 255) }
                        2 -> {
                            val r = codes.getOrNull(i + 2) ?: 0; val gr = codes.getOrNull(i + 3) ?: 0; val b = codes.getOrNull(i + 4) ?: 0
                            i += 4
                            0x1000000 or (r.coerceIn(0, 255) shl 16) or (gr.coerceIn(0, 255) shl 8) or b.coerceIn(0, 255)
                        }
                        else -> -1
                    }
                    style = if (c == 38) style.copy(fg = color) else style.copy(bg = color)
                }
            }
            i++
        }
    }

    // MARK: drawing

    /**
     * The screen's rows as text, top to bottom (xterm `translateToString(true)`: trailing blanks trimmed), for
     * the agent-frame parser. Every row of the active grid, including any the view leaves undrawn.
     */
    fun visibleRows(): List<String> = List(rows) { y -> String(g.cells[y]).trimEnd(' ') }

    /**
     * Scrollback plus the visible grid as styled text (the alternate screen is drawn whole, with no scrollback).
     * The last [hideLast] grid rows are left out: a coding agent's own input box the composer replaces.
     */
    fun render(hideLast: Int = 0): AnnotatedString {
        var last = rows - 1 - hideLast.coerceIn(0, rows - 1)
        if (!altScreen) while (last > cy && g.cells[last].all { it == ' ' }) last--
        return buildAnnotatedString {
            fun line(text: CharArray, st: List<CellStyle>, cursorAt: Int) {
                var end = text.size
                while (end > 0 && text[end - 1] == ' ' && st[end - 1].bg == -1) end--
                if (cursorAt >= end) end = cursorAt + 1
                var x = 0
                while (x < end) {
                    var y = x + 1
                    if (x != cursorAt) {
                        while (y < end && y != cursorAt && st[y] == st[x]) y++
                    }
                    val s = if (x == cursorAt) st[x].copy(inverse = !st[x].inverse) else st[x]
                    var fg = if (s.inverse) (if (s.bg == -1) TerminalColors.background else color(s.bg)) else if (s.fg == -1) null else color(s.fg, s.bold)
                    val bg = if (s.inverse) (if (s.fg == -1) TerminalColors.foreground else color(s.fg)) else if (s.bg == -1) null else color(s.bg)
                    if (s.dim) fg = (fg ?: TerminalColors.foreground).copy(alpha = 0.6f)
                    val deco = listOfNotNull(
                        if (s.underline) TextDecoration.Underline else null,
                        if (s.strike) TextDecoration.LineThrough else null,
                    ).let { if (it.isEmpty()) null else TextDecoration.combine(it) }
                    pushStyle(SpanStyle(
                        color = fg ?: Color.Unspecified, background = bg ?: Color.Unspecified,
                        fontWeight = if (s.bold) FontWeight.Bold else null,
                        fontStyle = if (s.italic) FontStyle.Italic else null, textDecoration = deco,
                    ))
                    append(String(text, x, y - x))
                    pop()
                    x = y
                }
                append('\n')
            }
            if (!altScreen) for ((t, st) in scrollback) line(t.toCharArray(), st, -1)
            for (y in 0..last) line(g.cells[y], g.styles[y].toList(), if (y == cy && cursorVisible) cx.coerceAtMost(cols - 1) else -1)
        }
    }

    companion object {
        private const val GROUND = 0
        private const val ESC = 1
        private const val CSI = 2
        private const val OSC = 3
        private const val OSC_ESC = 4
        private const val STR = 5
        private const val STR_ESC = 6
        private const val CHARSET = 7
        private const val SKIP1 = 8
        private const val LINE_DRAWING = "◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·"

        private val ansi16 = intArrayOf(
            0x1D2126, 0xE5534B, 0x57AB5A, 0xC69026, 0x539BF5, 0xB083F0, 0x39C5CF, 0xADBAC7,
            0x636E7B, 0xFF938A, 0x6BC46D, 0xDAAA3F, 0x6CB6FF, 0xDCBDFB, 0x56D4DD, 0xE4E6EA,
        )

        fun color(code: Int, bold: Boolean = false): Color {
            if (code >= 0x1000000) return Color(0xFF000000.toInt() or (code and 0xFFFFFF))
            val idx = if (bold && code in 0..7) code + 8 else code
            val rgb = when {
                idx < 16 -> ansi16[idx]
                idx < 232 -> {
                    val v = idx - 16
                    fun lv(k: Int) = if (k == 0) 0 else 55 + 40 * k
                    (lv(v / 36) shl 16) or (lv((v / 6) % 6) shl 8) or lv(v % 6)
                }
                else -> { val gr = 8 + 10 * (idx - 232); (gr shl 16) or (gr shl 8) or gr }
            }
            return Color(0xFF000000.toInt() or rgb)
        }
    }
}
