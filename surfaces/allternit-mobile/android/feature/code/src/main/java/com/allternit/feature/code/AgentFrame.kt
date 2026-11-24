package com.allternit.feature.code

import java.util.regex.Matcher
import java.util.regex.Pattern

/*
 * Reads a coding agent's on-screen frame (Claude Code, Codex, Kimi, Antigravity, opencode, Gizzi, …)
 * from the visible rows of a terminal, so the Code terminal can hide the agent's own input box and a
 * native composer can drive it.
 *
 * A line-for-line Kotlin port of `allternit-ai/src/lib/terminal/agent-frame.ts` (the reference; iOS
 * has the same port in Features/Code/AgentFrame.swift). Same patterns, same order, same result. The
 * shared screen corpus in `surfaces/allternit-mobile/agent-frame-fixtures/` locks the ports together
 * (src/test/.../AgentFrameTest.kt). Change the TypeScript first.
 *
 * Shapes, from real screens captured 2026-11-20:
 * - Claude / Antigravity: `────` border, `❯ …` (or `> …`) prompt, `────` border,
 *   then 1-2 footer rows (`⏵⏵ bypass permissions on (shift+tab to cycle)`).
 * - Kimi: a `╭──╮ │ > │ ╰──╯` box, then footer rows.
 * - Codex: no borders: `› Ask Codex…` then footer rows.
 * - opencode: a `┃  Ask anything…` block closed by `╹▀▀▀`, then footer rows.
 * - A permission / choice dialog REPLACES the input box: a question, numbered or
 *   arrow-selected options, then a hint (`Esc to cancel · Tab to amend`).
 *
 * Anything not recognised returns null, and the caller shows the full screen.
 *
 * Pure Kotlin (no Android imports), so the JVM unit test runs it as the app does.
 */

data class AgentDialogOption(
    /** The key that picks this option: a digit, or "" for arrow-select dialogs. */
    val key: String,
    val label: String,
    val selected: Boolean,
    /** 0-based position in the list (arrow-select dialogs move by this). */
    val index: Int,
)

data class AgentDialog(val question: String, val options: List<AgentDialogOption>)

enum class AgentStatus(val raw: String) {
    Idle("idle"),
    Working("working"),
    NeedsYou("needs-you");

    companion object {
        fun from(raw: String): AgentStatus? = entries.firstOrNull { it.raw == raw }
    }
}

data class AgentFrame(
    /** Display name when the screen or title names the agent ("Claude Code", "Codex", …). */
    val agent: String?,
    /** First row (0-based) of the agent's own input area / dialog: everything from here down is hidden. */
    val hideFrom: Int,
    val status: AgentStatus,
    val dialog: AgentDialog?,
    /** Permission mode named in the footer ("Bypass permissions", "Plan mode", …). */
    val mode: String?,
)

// MARK: - JavaScript regex semantics on java.util.regex

/**
 * A JavaScript regex (no `u`, `m`, `s` flags) run on java.util.regex. The source is written exactly as
 * in agent-frame.ts; [translate] rewrites the tokens whose meaning differs, so matching follows JS on
 * both the JVM (OpenJDK's engine, the unit tests) and Android (ART's java.util.regex is ICU underneath):
 * - `\s` / `\S`: JS's whitespace set. Java's is ASCII only (no U+00A0, U+2028, U+3000, U+FEFF…);
 *   ICU's is Unicode but adds U+0085 and lacks U+FEFF.
 * - `\d`: ASCII digits. Java's already is; ICU's matches every Unicode digit.
 * - `\b`: an ASCII word boundary. OpenJDK before 19 and ICU use Unicode letters for it.
 * - `.`: anything but \n \r U+2028 U+2029 (Java and ICU also stop at U+0085).
 * - `$`: end of input only (Java and ICU also match before a final line break).
 * - in a class: `[` and `&` are escaped (Java reads `[` as a nested class and `&&` as an intersection),
 *   and a `-` first or right before `]` is escaped; `\/` becomes `/`.
 * - the `i` flag: ASCII letters are expanded to both cases (`a` → `[aA]`, `a-z` → `a-zA-Z`) instead of
 *   CASE_INSENSITIVE, which on ICU folds Unicode too (the Kelvin sign would match `k`; JS without `u`
 *   never maps a non-ASCII character onto an ASCII one).
 * A character outside the BMP is one char to Java and two to JS; no pattern here counts or splits one.
 */
internal class JsRegex(source: String, ignoreCase: Boolean = false) {
    // Pattern is immutable and thread-safe; a Matcher is made per call.
    val pattern: Pattern = Pattern.compile(translate(source, ignoreCase))

    /** `re.test(s)`. */
    fun test(s: String): Boolean = pattern.matcher(s).find()

    /** `re.exec(s)`: the whole match then each group (null when a group took no part). */
    fun exec(s: String): List<String?>? {
        val m = pattern.matcher(s)
        if (!m.find()) return null
        return (0..m.groupCount()).map { m.group(it) }
    }

    /** `s.replace(re, with)` for a non-global regex: the first match only. */
    fun replaceFirst(s: String, with: String): String = pattern.matcher(s).replaceFirst(Matcher.quoteReplacement(with))

    /** `s.replace(/re/g, with)`. */
    fun replaceAll(s: String, with: String): String = pattern.matcher(s).replaceAll(Matcher.quoteReplacement(with))

    companion object {
        /** JS `\s`: WhiteSpace + LineTerminator (ECMA-262), as class contents. */
        const val WHITESPACE_CLASS = "\\t\\n\\x0B\\f\\r \\x{A0}\\x{1680}\\x{2000}-\\x{200A}\\x{2028}\\x{2029}\\x{202F}\\x{205F}\\x{3000}\\x{FEFF}"
        private const val WORD = "[A-Za-z0-9_]"

        private fun swapCase(c: Char): Char = if (c in 'a'..'z') c.uppercaseChar() else c.lowercaseChar()

        fun translate(js: String, ignoreCase: Boolean): String {
            val out = StringBuilder()
            var inClass = false
            var classStart = false // right after `[` or `[^`
            var i = 0
            while (i < js.length) {
                val c = js[i]
                if (c == '\\' && i + 1 < js.length) {
                    val n = js[i + 1]
                    i += 2
                    classStart = false
                    when {
                        n == 's' -> out.append(if (inClass) WHITESPACE_CLASS else "[$WHITESPACE_CLASS]")
                        n == 'S' -> out.append("[^$WHITESPACE_CLASS]") // never used inside a class
                        n == 'd' -> out.append(if (inClass) "0-9" else "[0-9]")
                        n == 'b' && !inClass -> out.append("(?:(?<=$WORD)(?!$WORD)|(?<!$WORD)(?=$WORD))")
                        n == '/' -> out.append('/')
                        else -> out.append('\\').append(n)
                    }
                    continue
                }
                if (inClass) {
                    when {
                        c == ']' -> { inClass = false; out.append(c) }
                        c == '^' && classStart -> { out.append(c); i++; continue } // stays classStart
                        c == '-' && (classStart || (i + 1 < js.length && js[i + 1] == ']')) -> out.append("\\-")
                        c == '[' -> out.append("\\[")
                        c == '&' -> out.append("\\&")
                        ignoreCase && c.isAsciiLetter() -> {
                            // A range of letters (`a-z`) or one letter: add the other case of each.
                            if (i + 2 < js.length && js[i + 1] == '-' && js[i + 2] != ']' && js[i + 2].isAsciiLetter()) {
                                val hi = js[i + 2]
                                out.append(c).append('-').append(hi).append(swapCase(c)).append('-').append(swapCase(hi))
                                i += 3
                                classStart = false
                                continue
                            }
                            out.append(c).append(swapCase(c))
                        }
                        else -> out.append(c)
                    }
                    classStart = false
                } else {
                    when {
                        c == '[' -> { inClass = true; classStart = true; out.append(c); i++; continue }
                        c == '.' -> out.append("[^\\n\\r\\x{2028}\\x{2029}]")
                        c == '$' -> out.append("\\z")
                        ignoreCase && c.isAsciiLetter() -> out.append('[').append(c).append(swapCase(c)).append(']')
                        else -> out.append(c)
                    }
                }
                i++
            }
            return out.toString()
        }

        private fun Char.isAsciiLetter() = this in 'a'..'z' || this in 'A'..'Z'
    }
}

/** JS whitespace (the `\s` set and what `String.prototype.trim()` strips); all of it is in the BMP. */
internal fun isJsWhitespace(c: Char): Boolean = when (c.code) {
    in 0x09..0x0D, 0x20, 0xA0, 0x1680, in 0x2000..0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF -> true
    else -> false
}

/** JS `String.prototype.trim()`. Kotlin's trim() differs (it strips U+001C-U+001F, keeps U+FEFF). */
internal fun jsTrim(s: String): String {
    var start = 0
    var end = s.length
    while (start < end && isJsWhitespace(s[start])) start++
    while (end > start && isJsWhitespace(s[end - 1])) end--
    return s.substring(start, end)
}

/** `!s.trim()`: the row is empty or whitespace only. */
private fun isBlankJs(s: String): Boolean = s.all(::isJsWhitespace)

/** `rows.slice(from, to)` with JS clamping. */
private fun slice(rows: List<String>, from: Int, to: Int): List<String> {
    val lo = from.coerceIn(0, rows.size)
    val hi = to.coerceIn(lo, rows.size)
    return rows.subList(lo, hi)
}

// MARK: - Parser

object AgentFrameParser {
    private val BORDER = JsRegex("^\\s*[─━═╌]{12,}\\s*$")
    private val BOX_TOP = JsRegex("^\\s*[╭┌][─━]{8,}[╮┐]\\s*$")
    private val BOX_BOTTOM = JsRegex("^\\s*[╰└][─━]{8,}[╯┘]\\s*$")
    private val BLOCK_BOTTOM = JsRegex("^\\s*╹[▀━─]{8,}\\s*$")
    private val PROMPT = JsRegex("^\\s*(?:[│┃]\\s*)?[❯›>](?:\\s|$)")
    private val BLOCK_PROMPT = JsRegex("^\\s*┃(?:\\s|$)")
    // Dialogs are matched first, so a plain "esc to cancel" footer here means a turn is running (Antigravity).
    private val WORKING = JsRegex("esc to interrupt|esc interrupt|esc to cancel|ctrl\\+c to interrupt|press esc to stop", ignoreCase = true)
    private val DIALOG_END = JsRegex("(esc to cancel|enter to confirm|enter confirm|enter continue|esc back|↑\\/?↓\\s*(?:to\\s+)?navigate|enter select|tab to amend)", ignoreCase = true)
    private val NUMBERED = JsRegex("^\\s*(?:[❯›>]\\s*)?(\\d{1,2})[.)]\\s+(.+?)\\s*$")
    private val SELECTED = JsRegex("^\\s*[❯›>]\\s")
    private val MODE = JsRegex("(?:⏵⏵|⏸|›|>)\\s*([a-z][a-z -]*?)\\s+(?:mode\\s+)?on\\b", ignoreCase = true)
    private val MODE_ALT = JsRegex("^\\s*(?:>\\s*)?(auto|plan|default|accept[- ]edits)\\s+mode\\b", ignoreCase = true)
    // Codex shows `Plan mode (⇧tab to cycle)` on the right of its footer row.
    private val MODE_INLINE = JsRegex("\\b(auto|plan|default|accept[- ]edits)\\s+mode\\s*\\(", ignoreCase = true)
    // opencode names its agent in the box: `┃  Build · <model>` or `┃  Plan · <model>`.
    private val MODE_BLOCK = JsRegex("^\\s*┃\\s+(Build|Plan)\\s+·")
    // Claude's right-aligned effort row just above its box (`● high · /effort`), not a wrapped menu line.
    private val EFFORT_ROW = JsRegex("^\\s{30,}(?:[●○◐◑◒◓]\\s|\\S.*\\/effort\\s*$)")
    // Inline patterns in agent-frame.ts.
    private val INDENTED = JsRegex("^\\s{2,}\\S")
    private val QUESTION_END = JsRegex("\\?\\s*$")
    private val CODEX_PROMPT = JsRegex("^\\s*›\\s")
    private val SPACES = JsRegex("\\s+")

    private val AGENT_NAMES: List<Pair<JsRegex, String>> = listOf(
        JsRegex("claude code", ignoreCase = true) to "Claude Code",
        JsRegex("\\bcodex\\b", ignoreCase = true) to "Codex",
        JsRegex("\\bkimi\\b|\\bK2[.\\d]", ignoreCase = true) to "Kimi",
        JsRegex("antigravity", ignoreCase = true) to "Antigravity",
        JsRegex("opencode", ignoreCase = true) to "opencode",
        JsRegex("\\bgizzi\\b", ignoreCase = true) to "Gizzi",
        JsRegex("\\bgrok\\b", ignoreCase = true) to "Grok",
        JsRegex("\\bgemini\\b", ignoreCase = true) to "Gemini",
        JsRegex("\\bqwen\\b", ignoreCase = true) to "Qwen",
    )

    /** Names the agent from text on screen or the terminal title; null when unknown. */
    fun agentName(text: String): String? {
        for ((re, name) in AGENT_NAMES) if (re.test(text)) return name
        return null
    }

    private fun lastNonEmpty(rows: List<String>): Int {
        for (i in rows.indices.reversed()) if (!isBlankJs(rows[i])) return i
        return -1
    }

    private fun modeFrom(rows: List<String>): String? {
        for (row in rows) {
            val m = MODE.exec(row) ?: MODE_ALT.exec(row) ?: MODE_INLINE.exec(row) ?: MODE_BLOCK.exec(row)
            val group = m?.get(1)
            if (group != null) {
                val label = SPACES.replaceAll(jsTrim(group), " ")
                if (label.isEmpty()) return label
                // JS charAt(0).toUpperCase(): one UTF-16 unit, full (not title) case mapping.
                return label.substring(0, 1).uppercase() + label.substring(1)
            }
        }
        return null
    }

    private class Found(val top: Int, val question: String, val options: List<AgentDialogOption>)

    private fun findDialog(rows: List<String>, last: Int): Found? {
        // The hint line closes the dialog and sits within the last few rows.
        var end = -1
        var i = last
        while (i >= maxOf(0, last - 3)) {
            if (DIALOG_END.test(rows[i])) { end = i; break }
            i -= 1
        }
        if (end < 0) return null
        val options = ArrayList<AgentDialogOption>()
        i = end - 1
        while (i >= 0 && (isBlankJs(rows[i]) || (DIALOG_END.test(rows[i]) && end - i <= 3))) i -= 1
        // Walk up over option rows: numbered, or selection-glyph / same-indent rows.
        val optionRows = ArrayList<Int>()
        while (i >= 0 && optionRows.size < 9) {
            val row = rows[i]
            if (isBlankJs(row)) break
            if (NUMBERED.test(row) || SELECTED.test(row) || INDENTED.test(row)) optionRows.add(0, i)
            else break
            i -= 1
        }
        // Wrapped continuation rows of a long option are indented deeper; keep option heads only.
        val heads = optionRows.filter { NUMBERED.test(rows[it]) || SELECTED.test(rows[it]) || optionRows.size <= 4 }
        if (heads.size < 2) return null
        heads.forEachIndexed { index, r ->
            val row = rows[r]
            val num = NUMBERED.exec(row)
            options.add(
                AgentDialogOption(
                    key = num?.get(1) ?: "",
                    label = jsTrim(if (num != null) num[2].orEmpty() else SELECTED.replaceFirst(row, "")),
                    selected = SELECTED.test(row),
                    index = index,
                ),
            )
        }
        // The question: a row ending in '?' just above the options (else the nearest row).
        var q = optionRows[0] - 1
        while (q >= 0 && isBlankJs(rows[q])) q -= 1
        var asked = -1
        var r = q
        while (r >= maxOf(0, optionRows[0] - 6)) {
            if (QUESTION_END.test(rows[r])) { asked = r; break }
            r -= 1
        }
        val qRow = if (asked >= 0) asked else q
        val question = if (qRow >= 0) jsTrim(rows[qRow]) else ""
        // Hide from the question only when it sits right above the options; else just the options.
        val top = if (asked >= 0 && optionRows[0] - asked <= 2) asked else optionRows[0]
        return Found(top, question, options)
    }

    /** Returns the input area's first row, or -1. Rows after the box beyond 3 footer rows are a menu: -2. */
    private fun findInputTop(rows: List<String>, last: Int): Int {
        val floor = maxOf(0, last - 16)
        val belowBox = { bottom: Int -> slice(rows, bottom + 1, last + 1).count { !isBlankJs(it) } }
        // Bordered box: last bottom border, a prompt row above it, a top border above that.
        var b2 = last
        while (b2 >= floor) {
            if (BORDER.test(rows[b2]) || BOX_BOTTOM.test(rows[b2])) {
                var b1 = b2 - 1
                while (b1 >= maxOf(0, b2 - 6)) {
                    if (BORDER.test(rows[b1]) || BOX_TOP.test(rows[b1])) {
                        val inner = slice(rows, b1 + 1, b2)
                        if (inner.any { PROMPT.test(it) }) {
                            if (belowBox(b2) > 3) return -2
                            // Claude draws a right-aligned status row (`● high · /effort`) just above.
                            val above = b1 - 1
                            if (above >= 0 && EFFORT_ROW.test(rows[above])) return above
                            return b1
                        }
                        break
                    }
                    b1 -= 1
                }
            }
            b2 -= 1
        }
        // opencode: a ┃ block closed by ╹▀▀▀.
        var b = last
        while (b >= floor) {
            if (BLOCK_BOTTOM.test(rows[b])) {
                var top = b - 1
                while (top - 1 >= 0 && BLOCK_PROMPT.test(rows[top - 1])) top -= 1
                // JS reads rows[-1] as undefined (no match); Kotlin must not index it.
                if (top >= 0 && top < b && BLOCK_PROMPT.test(rows[top])) return top
            }
            b -= 1
        }
        // Codex: a borderless `›` prompt near the bottom with only footer rows below.
        var p = last
        while (p >= maxOf(0, last - 5)) {
            if (CODEX_PROMPT.test(rows[p])) return p
            p -= 1
        }
        return -1
    }

    /**
     * Parses the visible rows (top to bottom, one string per row). [title] is the terminal title, used
     * to name the agent when the banner has scrolled away.
     */
    fun parse(rows: List<String>, title: String = ""): AgentFrame? {
        val last = lastNonEmpty(rows)
        if (last < 2) return null
        val tail = slice(rows, maxOf(0, last - 14), last + 1)
        val agent = agentName(title) ?: agentName(rows.joinToString("\n"))

        val dialog = findDialog(rows, last)
        if (dialog != null) {
            return AgentFrame(
                agent = agent,
                hideFrom = dialog.top,
                status = AgentStatus.NeedsYou,
                dialog = AgentDialog(dialog.question, dialog.options),
                mode = modeFrom(tail),
            )
        }

        val top = findInputTop(rows, last)
        if (top == -2) {
            // A menu (slash commands, models) is open under the box: show the whole screen.
            return AgentFrame(agent = agent, hideFrom = rows.size, status = AgentStatus.Idle, dialog = null, mode = null)
        }
        if (top < 0) return null
        val footer = slice(rows, top, last + 1)
        // The spinner row ("✻ Brewing… (21s · esc to interrupt)") sits just above the box; it stays visible.
        val near = slice(rows, maxOf(0, top - 3), last + 1).joinToString("\n")
        return AgentFrame(
            agent = agent,
            hideFrom = top,
            status = if (WORKING.test(near)) AgentStatus.Working else AgentStatus.Idle,
            dialog = null,
            mode = modeFrom(footer),
        )
    }
}

// MARK: - Keys (agent-frame.ts)

object AgentKeys {
    const val ESCAPE = "\u001b"
    /** shift+tab: cycles the agent's permission mode. */
    const val CYCLE_MODE = "\u001b[Z"
    const val ENTER = "\r"

    /**
     * Bracketed paste, then Enter: how a prompt is submitted to a TUI agent. The caller sends the paste,
     * then [ENTER] about 60 ms later (CodeTerminalCanvas `send`), so a multi-line message stays one prompt.
     */
    fun promptPaste(text: String): String = "\u001b[200~$text\u001b[201~"

    /** Keys that answer a dialog option: its digit, or arrows from the selected row then Enter. */
    fun dialogKeys(options: List<AgentDialogOption>, pick: AgentDialogOption): String {
        if (pick.key.isNotEmpty()) return pick.key
        val from = options.firstOrNull { it.selected }?.index ?: 0
        val delta = pick.index - from
        val arrow = if (delta > 0) "\u001b[B" else "\u001b[A"
        return arrow.repeat(kotlin.math.abs(delta)) + "\r"
    }
}

// MARK: - Composer text (agent-composer.tsx, iOS AgentComposerText)

object AgentComposerText {
    /** The composer's placeholder for the agent's state. */
    fun placeholder(frame: AgentFrame): String {
        val name = frame.agent ?: "the agent"
        return when (frame.status) {
            AgentStatus.Working -> "Queue a message for $name…"
            AgentStatus.NeedsYou -> "Or tell $name what to do instead…"
            AgentStatus.Idle -> "Message $name…"
        }
    }

    fun status(status: AgentStatus): String = when (status) {
        AgentStatus.Idle -> "Idle"
        AgentStatus.Working -> "Working"
        AgentStatus.NeedsYou -> "Needs you"
    }

    private val SHORT_LABEL_TAIL = JsRegex("\\s*\\(.*$")
    private val MODE_TAIL = JsRegex("\\s+(permissions|mode)$", ignoreCase = true)
    private val PERMISSION_WORDS = JsRegex("\\b(allow|permission|want to|proceed|approve|trust)\\b", ignoreCase = true)

    /** "Yes, and switch to accept edits (auto-approve …) for this session (shift+tab)" → "Yes, and switch to accept edits". */
    fun shortLabel(label: String): String {
        val cut = jsTrim(SHORT_LABEL_TAIL.replaceFirst(label, ""))
        return cut.ifEmpty { label }
    }

    /** "Bypass permissions" → "Bypass"; "Accept edits" stays. */
    fun shortMode(mode: String): String {
        val m = MODE_TAIL.replaceFirst(mode, "")
        if (m.isEmpty()) return m
        return m.substring(0, 1).uppercase() + m.substring(1)
    }

    /** A permission prompt (lock icon, "Permission required") rather than a plain choice. */
    fun isPermission(question: String): Boolean = PERMISSION_WORDS.test(question)
}
