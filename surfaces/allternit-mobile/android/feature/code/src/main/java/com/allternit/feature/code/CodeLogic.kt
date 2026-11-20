package com.allternit.feature.code

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration

// MARK: - Diff lines (iOS DiffLine / DiffRenderer)

enum class DiffKind { Context, Added, Removed, Hunk }

data class DiffLine(val kind: DiffKind, val text: String, val oldNo: Int?, val newNo: Int?) {
    companion object {
        /** iOS DiffLine.diffLines(before:after:): a line diff (LCS) of two full texts. */
        fun diffLines(before: String, after: String): List<DiffLine> {
            val a = if (before.isEmpty()) emptyList() else before.split("\n")
            val b = if (after.isEmpty()) emptyList() else after.split("\n")
            var start = 0
            while (start < a.size && start < b.size && a[start] == b[start]) start++
            var endA = a.size
            var endB = b.size
            while (endA > start && endB > start && a[endA - 1] == b[endB - 1]) { endA--; endB-- }
            val out = ArrayList<DiffLine>()
            for (i in 0 until start) out.add(DiffLine(DiffKind.Context, a[i], i + 1, i + 1))
            val ma = a.subList(start, endA)
            val mb = b.subList(start, endB)
            var oldNo = start + 1
            var newNo = start + 1
            if (ma.size.toLong() * mb.size.toLong() > 4_000_000L) {
                // Too large for an LCS table on a phone: show the changed block as removed then added.
                ma.forEach { out.add(DiffLine(DiffKind.Removed, it, oldNo++, null)) }
                mb.forEach { out.add(DiffLine(DiffKind.Added, it, null, newNo++)) }
            } else {
                val n = ma.size
                val m = mb.size
                val lcs = Array(n + 1) { IntArray(m + 1) }
                for (i in n - 1 downTo 0) for (j in m - 1 downTo 0) {
                    lcs[i][j] = if (ma[i] == mb[j]) lcs[i + 1][j + 1] + 1 else maxOf(lcs[i + 1][j], lcs[i][j + 1])
                }
                var i = 0
                var j = 0
                while (i < n || j < m) {
                    when {
                        i < n && j < m && ma[i] == mb[j] -> { out.add(DiffLine(DiffKind.Context, ma[i], oldNo++, newNo++)); i++; j++ }
                        j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) -> { out.add(DiffLine(DiffKind.Added, mb[j], null, newNo++)); j++ }
                        else -> { out.add(DiffLine(DiffKind.Removed, ma[i], oldNo++, null)); i++ }
                    }
                }
            }
            oldNo = endA + 1
            newNo = endB + 1
            for (k in endA until a.size) out.add(DiffLine(DiffKind.Context, a[k], oldNo++, newNo++))
            return out
        }

        /** iOS DiffLine.parse(unifiedDiff:): a unified diff string as lines. */
        fun parse(unified: String): List<DiffLine> {
            val out = ArrayList<DiffLine>()
            var oldNo = 0
            var newNo = 0
            val header = Regex("""^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@""")
            for (raw in unified.split("\n")) {
                when {
                    raw.startsWith("@@") -> {
                        header.find(raw)?.let { oldNo = it.groupValues[1].toInt(); newNo = it.groupValues[2].toInt() }
                        out.add(DiffLine(DiffKind.Hunk, raw, null, null))
                    }
                    raw.startsWith("+++") || raw.startsWith("---") || raw.startsWith("diff ") || raw.startsWith("index ") -> Unit
                    raw.startsWith("+") -> out.add(DiffLine(DiffKind.Added, raw.drop(1), null, newNo++))
                    raw.startsWith("-") -> out.add(DiffLine(DiffKind.Removed, raw.drop(1), oldNo++, null))
                    raw.startsWith("\\") -> Unit
                    else -> out.add(DiffLine(DiffKind.Context, raw.removePrefix(" "), oldNo++, newNo++))
                }
            }
            if (out.lastOrNull()?.let { it.kind == DiffKind.Context && it.text.isEmpty() } == true) out.removeAt(out.lastIndex)
            return out
        }
    }
}

// MARK: - Terminal screen (the part of a VT100 that shells and pane screens need)

/** iOS TerminalPalette. */
object TerminalColors {
    val background = Color(0xFF0D0F12)
    val foreground = Color(0xFFE4E6EA)
    val secondary = Color(0xFF9AA0A8)
    val strip = Color(0xFF14171B)
    val key = Color(0xFF1D2126)
    val keyPressed = Color(0xFF2A2F36)
    val line = Color(0xFF23272D)
}

/** One cell's look; colors are palette indexes (0-255), packed 24-bit RGB (|0x1000000), or -1 for default. */
data class CellStyle(val fg: Int = -1, val bg: Int = -1, val bold: Boolean = false, val underline: Boolean = false, val inverse: Boolean = false)

/**
 * A small terminal emulator: a cols x rows grid with cursor movement, erase, SGR colors and
 * scrollback. The iOS terminal is SwiftTerm; this covers what the Code shell and a Terminal bot's
 * `screen` frames print (full VT100 apps like vim fall back to what they draw as plain text).
 */
class VtScreen(var cols: Int = 80, var rows: Int = 24) {
    private var cells = Array(rows) { CharArray(cols) { ' ' } }
    private var styles = Array(rows) { Array(cols) { CellStyle() } }
    private val scrollback = ArrayList<Pair<String, List<CellStyle>>>()
    private var cx = 0
    private var cy = 0
    private var style = CellStyle()
    private var esc = StringBuilder()
    private var inEsc = false
    private var savedX = 0
    private var savedY = 0
    /** Bumped on every change so Compose can re-read. */
    var version = 0
        private set

    fun resize(newCols: Int, newRows: Int) {
        if (newCols < 2 || newRows < 2 || (newCols == cols && newRows == rows)) return
        val nc = Array(newRows) { CharArray(newCols) { ' ' } }
        val ns = Array(newRows) { Array(newCols) { CellStyle() } }
        for (y in 0 until minOf(rows, newRows)) for (x in 0 until minOf(cols, newCols)) { nc[y][x] = cells[y][x]; ns[y][x] = styles[y][x] }
        cells = nc; styles = ns; cols = newCols; rows = newRows
        cx = cx.coerceIn(0, cols - 1); cy = cy.coerceIn(0, rows - 1)
        version++
    }

    fun reset() {
        cells = Array(rows) { CharArray(cols) { ' ' } }
        styles = Array(rows) { Array(cols) { CellStyle() } }
        scrollback.clear()
        cx = 0; cy = 0; style = CellStyle(); inEsc = false; esc.clear()
        version++
    }

    fun feed(text: String) {
        for (ch in text) put(ch)
        version++
    }

    private fun put(ch: Char) {
        if (inEsc) {
            esc.append(ch)
            val s = esc
            if (s.length == 1) {
                // ESC x: only `[` (CSI) and `]` (OSC) open a longer sequence; the rest end here.
                if (ch != '[' && ch != ']') { if (ch == '7') { savedX = cx; savedY = cy } else if (ch == '8') { cx = savedX; cy = savedY }; inEsc = false }
                return
            }
            if (s[0] == '[') {
                if (ch in '@'..'~') { csi(s.substring(1, s.length - 1), ch); inEsc = false }
            } else if (s[0] == ']') {
                if (ch == '\u0007' || (ch == '\\' && s.length >= 2 && s[s.length - 2] == '\u001b')) inEsc = false
            } else inEsc = false
            if (esc.length > 200) inEsc = false
            return
        }
        when (ch) {
            '\u001b' -> { inEsc = true; esc.setLength(0) }
            '\r' -> cx = 0
            '\n' -> lineFeed()
            '\b' -> if (cx > 0) cx--
            '\t' -> cx = minOf(cols - 1, (cx / 8 + 1) * 8)
            '\u0007' -> Unit
            else -> if (ch >= ' ') {
                if (cx >= cols) { cx = 0; lineFeed() }
                cells[cy][cx] = ch
                styles[cy][cx] = style
                cx++
            }
        }
    }

    private fun lineFeed() {
        if (cy < rows - 1) { cy++; return }
        scrollback.add(String(cells[0]) to styles[0].toList())
        if (scrollback.size > 2000) scrollback.removeAt(0)
        for (y in 1 until rows) { cells[y - 1] = cells[y]; styles[y - 1] = styles[y] }
        cells[rows - 1] = CharArray(cols) { ' ' }
        styles[rows - 1] = Array(cols) { CellStyle() }
    }

    private fun csi(params: String, final: Char) {
        val p = params.removePrefix("?").split(';').map { it.toIntOrNull() }
        fun n(i: Int, d: Int) = (p.getOrNull(i) ?: d).let { if (it == 0 && d == 1) 1 else it }
        when (final) {
            'A' -> cy = maxOf(0, cy - n(0, 1))
            'B' -> cy = minOf(rows - 1, cy + n(0, 1))
            'C' -> cx = minOf(cols - 1, cx + n(0, 1))
            'D' -> cx = maxOf(0, cx - n(0, 1))
            'E' -> { cy = minOf(rows - 1, cy + n(0, 1)); cx = 0 }
            'F' -> { cy = maxOf(0, cy - n(0, 1)); cx = 0 }
            'G' -> cx = (n(0, 1) - 1).coerceIn(0, cols - 1)
            'H', 'f' -> { cy = (n(0, 1) - 1).coerceIn(0, rows - 1); cx = (n(1, 1) - 1).coerceIn(0, cols - 1) }
            'd' -> cy = (n(0, 1) - 1).coerceIn(0, rows - 1)
            'J' -> when (p.getOrNull(0) ?: 0) {
                0 -> { eraseLine(cy, cx, cols); for (y in cy + 1 until rows) eraseLine(y, 0, cols) }
                1 -> { for (y in 0 until cy) eraseLine(y, 0, cols); eraseLine(cy, 0, cx + 1) }
                else -> for (y in 0 until rows) eraseLine(y, 0, cols)
            }
            'K' -> when (p.getOrNull(0) ?: 0) { 0 -> eraseLine(cy, cx, cols); 1 -> eraseLine(cy, 0, cx + 1); else -> eraseLine(cy, 0, cols) }
            'P' -> { val k = n(0, 1); for (x in cx until cols) { val src = x + k; cells[cy][x] = if (src < cols) cells[cy][src] else ' '; styles[cy][x] = if (src < cols) styles[cy][src] else CellStyle() } }
            'X' -> eraseLine(cy, cx, minOf(cols, cx + n(0, 1)))
            's' -> { savedX = cx; savedY = cy }
            'u' -> { cx = savedX; cy = savedY }
            'm' -> sgr(p.map { it ?: 0 }.ifEmpty { listOf(0) })
        }
    }

    private fun eraseLine(y: Int, from: Int, to: Int) {
        for (x in from.coerceAtLeast(0) until to.coerceAtMost(cols)) { cells[y][x] = ' '; styles[y][x] = CellStyle() }
    }

    private fun sgr(codes: List<Int>) {
        var i = 0
        while (i < codes.size) {
            val c = codes[i]
            when {
                c == 0 -> style = CellStyle()
                c == 1 -> style = style.copy(bold = true)
                c == 22 -> style = style.copy(bold = false)
                c == 4 -> style = style.copy(underline = true)
                c == 24 -> style = style.copy(underline = false)
                c == 7 -> style = style.copy(inverse = true)
                c == 27 -> style = style.copy(inverse = false)
                c in 30..37 -> style = style.copy(fg = c - 30)
                c == 39 -> style = style.copy(fg = -1)
                c in 40..47 -> style = style.copy(bg = c - 40)
                c == 49 -> style = style.copy(bg = -1)
                c in 90..97 -> style = style.copy(fg = c - 90 + 8)
                c in 100..107 -> style = style.copy(bg = c - 100 + 8)
                c == 38 || c == 48 -> {
                    val color = when (codes.getOrNull(i + 1)) {
                        5 -> { val v = codes.getOrNull(i + 2) ?: 0; i += 2; v.coerceIn(0, 255) }
                        2 -> { val r = codes.getOrNull(i + 2) ?: 0; val g = codes.getOrNull(i + 3) ?: 0; val b = codes.getOrNull(i + 4) ?: 0; i += 4; 0x1000000 or (r.coerceIn(0, 255) shl 16) or (g.coerceIn(0, 255) shl 8) or b.coerceIn(0, 255) }
                        else -> -1
                    }
                    style = if (c == 38) style.copy(fg = color) else style.copy(bg = color)
                }
            }
            i++
        }
    }

    /** Scrollback plus the visible grid as styled text, trailing blank rows trimmed. */
    fun render(): AnnotatedString {
        var last = rows - 1
        while (last > cy && cells[last].all { it == ' ' }) last--
        return buildAnnotatedString {
            fun line(text: CharArray, st: List<CellStyle>) {
                var end = text.size
                while (end > 0 && text[end - 1] == ' ' && st[end - 1].bg == -1) end--
                var x = 0
                while (x < end) {
                    var y = x
                    while (y < end && st[y] == st[x]) y++
                    val s = st[x]
                    val fg = if (s.inverse) (if (s.bg == -1) TerminalColors.background else color(s.bg)) else if (s.fg == -1) null else color(s.fg, s.bold)
                    val bg = if (s.inverse) (if (s.fg == -1) TerminalColors.foreground else color(s.fg)) else if (s.bg == -1) null else color(s.bg)
                    pushStyle(SpanStyle(color = fg ?: Color.Unspecified, background = bg ?: Color.Unspecified, fontWeight = if (s.bold) FontWeight.Bold else null, textDecoration = if (s.underline) TextDecoration.Underline else null))
                    append(String(text, x, y - x))
                    pop()
                    x = y
                }
                append('\n')
            }
            for ((t, st) in scrollback) line(t.toCharArray(), st)
            for (y in 0..last) line(cells[y], styles[y].toList())
        }
    }

    companion object {
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
                else -> { val g = 8 + 10 * (idx - 232); (g shl 16) or (g shl 8) or g }
            }
            return Color(0xFF000000.toInt() or rgb)
        }
    }
}

// MARK: - Terminal keys (iOS TerminalCtrlLatch + FactoryPaneClient.paneInput)

/** The Ctrl latch: the next keyboard character becomes its control byte. */
fun ctrlByte(ch: Char): Char? = when (ch.lowercaseChar()) {
    in 'a'..'z' -> (ch.lowercaseChar() - 'a' + 1).toChar()
    '[' -> '\u001b'
    '\\' -> '\u001c'
    ']' -> '\u001d'
    ' ' -> '\u0000'
    else -> null
}

/** Pane input (FactoryPaneClient.paneInput): split typed text into literal text and named keys. */
fun paneInput(data: String): List<Pair<String?, List<String>?>> {
    val out = ArrayList<Pair<String?, List<String>?>>()
    val text = StringBuilder()
    fun flush() { if (text.isNotEmpty()) { out.add(text.toString() to null); text.setLength(0) } }
    var i = 0
    while (i < data.length) {
        val ch = data[i]
        val key: String? = when {
            ch == '\r' || ch == '\n' -> "Enter"
            ch == '\t' -> "Tab"
            ch == '\u007f' || ch == '\b' -> "BSpace"
            ch == '\u0003' -> "C-c"
            ch == '\u0004' -> "C-d"
            ch == '\u001b' && data.startsWith("\u001b[A", i) -> { i += 2; "Up" }
            ch == '\u001b' && data.startsWith("\u001b[B", i) -> { i += 2; "Down" }
            ch == '\u001b' && data.startsWith("\u001b[C", i) -> { i += 2; "Right" }
            ch == '\u001b' && data.startsWith("\u001b[D", i) -> { i += 2; "Left" }
            ch == '\u001b' -> "Escape"
            ch < ' ' -> "C-" + (ch + 96).toString()
            else -> null
        }
        if (key == null) text.append(ch) else { flush(); out.add(null to listOf(key)) }
        i++
    }
    flush()
    return out
}

// MARK: - Editor highlighting (iOS uses Runestone + Tree-sitter; this is a light keyword pass)

fun languageOf(path: String): String = when (path.substringAfterLast('.', "").lowercase()) {
    "kt", "kts" -> "kotlin"; "swift" -> "swift"; "rs" -> "rust"; "ts", "tsx", "js", "jsx", "mjs" -> "js"
    "py" -> "python"; "json" -> "json"; "md", "mdx" -> "markdown"; "sh", "zsh", "bash" -> "shell"; "go" -> "go"
    "html", "xml" -> "markup"; "css" -> "css"; "toml", "yml", "yaml" -> "config"; else -> "text"
}

private val keywords = setOf(
    "fun", "val", "var", "let", "const", "func", "fn", "def", "class", "struct", "enum", "interface", "impl", "trait", "object", "import", "package", "use",
    "return", "if", "else", "when", "match", "for", "while", "in", "do", "switch", "case", "break", "continue", "try", "catch", "throw", "throws", "async", "await",
    "public", "private", "internal", "static", "pub", "mut", "true", "false", "null", "nil", "None", "True", "False", "self", "this", "new", "from", "as", "export", "type",
)

/** A light highlight pass over [text]: comments, strings, numbers, keywords. */
fun highlight(text: String, language: String, accent: Color, string: Color, comment: Color, number: Color): AnnotatedString {
    if (language == "text" || language == "markdown" || text.length > 60_000) return AnnotatedString(text)
    val hashComments = language in setOf("python", "shell", "config")
    return buildAnnotatedString {
        append(text)
        var i = 0
        val n = text.length
        while (i < n) {
            val ch = text[i]
            when {
                (ch == '/' && i + 1 < n && text[i + 1] == '/' && language != "json") || (hashComments && ch == '#') -> {
                    val e = text.indexOf('\n', i).let { if (it < 0) n else it }
                    addStyle(SpanStyle(color = comment), i, e); i = e
                }
                ch == '/' && i + 1 < n && text[i + 1] == '*' && language != "json" -> {
                    val e = text.indexOf("*/", i + 2).let { if (it < 0) n else it + 2 }
                    addStyle(SpanStyle(color = comment), i, e); i = e
                }
                ch == '"' || ch == '\'' || ch == '`' -> {
                    var e = i + 1
                    while (e < n && text[e] != ch && text[e] != '\n') { if (text[e] == '\\') e++; e++ }
                    e = minOf(n, e + 1)
                    addStyle(SpanStyle(color = string), i, e); i = e
                }
                ch.isDigit() && (i == 0 || !text[i - 1].isLetterOrDigit()) -> {
                    var e = i
                    while (e < n && (text[e].isLetterOrDigit() || text[e] == '.')) e++
                    addStyle(SpanStyle(color = number), i, e); i = e
                }
                ch.isLetter() || ch == '_' -> {
                    var e = i
                    while (e < n && (text[e].isLetterOrDigit() || text[e] == '_')) e++
                    if (text.substring(i, e) in keywords) addStyle(SpanStyle(color = accent, fontWeight = FontWeight.Medium), i, e)
                    i = e
                }
                else -> i++
            }
        }
    }
}
