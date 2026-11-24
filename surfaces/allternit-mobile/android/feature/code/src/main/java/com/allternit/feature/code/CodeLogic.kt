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
