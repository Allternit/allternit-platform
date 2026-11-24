import Foundation

/// Reads a coding agent's on-screen frame (Claude Code, Codex, Kimi, Antigravity,
/// opencode, Gizzi, …) from the visible rows of a terminal, so the Code terminal
/// can hide the agent's own input box and a native composer can drive it.
///
/// A line-for-line Swift port of `allternit-ai/src/lib/terminal/agent-frame.ts`
/// (the reference). Same patterns, same order, same result. The shared screen
/// corpus in `surfaces/allternit-mobile/agent-frame-fixtures/` locks the ports
/// together (Tests/AgentFrameTests.swift). Change the TypeScript first.
///
/// Shapes, from real screens captured 2026-11-20:
/// - Claude / Antigravity: `────` border, `❯ …` (or `> …`) prompt, `────` border,
///   then 1-2 footer rows (`⏵⏵ bypass permissions on (shift+tab to cycle)`).
/// - Kimi: a `╭──╮ │ > │ ╰──╯` box, then footer rows.
/// - Codex: no borders: `› Ask Codex…` then footer rows.
/// - opencode: a `┃  Ask anything…` block closed by `╹▀▀▀`, then footer rows.
/// - A permission / choice dialog REPLACES the input box: a question, numbered or
///   arrow-selected options, then a hint (`Esc to cancel · Tab to amend`).
///
/// Anything not recognised returns `nil`, and the caller shows the full screen.

struct AgentDialogOption: Equatable, Hashable, Sendable, Decodable {
    /// The key that picks this option: a digit, or "" for arrow-select dialogs.
    let key: String
    let label: String
    let selected: Bool
    /// 0-based position in the list (arrow-select dialogs move by this).
    let index: Int
}

struct AgentDialog: Equatable, Sendable, Decodable {
    let question: String
    let options: [AgentDialogOption]
}

struct AgentFrame: Equatable, Sendable, Decodable {
    enum Status: String, Equatable, Sendable, Decodable {
        case idle
        case working
        case needsYou = "needs-you"
    }

    /// Display name when the screen or title names the agent ("Claude Code", "Codex", …).
    let agent: String?
    /// First row (0-based) of the agent's own input area / dialog: everything from here down is hidden.
    let hideFrom: Int
    let status: Status
    let dialog: AgentDialog?
    /// Permission mode named in the footer ("Bypass permissions", "Plan mode", …).
    let mode: String?
}

// MARK: - JavaScript regex semantics on NSRegularExpression

/// A JavaScript regex (no `u`, `m`, `s` flags) run on ICU. The source is written
/// exactly as in agent-frame.ts; `icu(_:)` rewrites the few tokens whose meaning
/// differs, so matching follows JS:
/// - `\s` / `\S`: JS's whitespace set (ICU adds U+0085 and lacks U+FEFF).
/// - `\d`: ASCII digits (ICU matches every Unicode digit).
/// - `\b`: an ASCII word boundary (ICU's `\w` is Unicode-aware).
/// - `.`: anything but \n \r U+2028 U+2029 (ICU also stops at U+0085).
/// - `$`: end of input only (ICU also matches before a final line break).
/// - a `-` right before a class's `]` is escaped, and `\/` becomes `/`.
/// Case-insensitive matching uses ICU's case folding, which equals JS for the
/// ASCII letters these patterns use.
struct JSRegex: @unchecked Sendable {
    // NSRegularExpression is immutable and thread-safe.
    let regex: NSRegularExpression

    init(_ source: String, ignoreCase: Bool = false) {
        // The patterns are constants: a bad one is a programming error caught by the tests.
        regex = try! NSRegularExpression(pattern: Self.icu(source), options: ignoreCase ? [.caseInsensitive] : [])
    }

    /// `re.test(s)`.
    func test(_ s: String) -> Bool {
        regex.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)) != nil
    }

    /// `re.exec(s)`: the whole match then each group (nil when a group took no part).
    func exec(_ s: String) -> [String?]? {
        guard let match = regex.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)) else { return nil }
        return (0..<match.numberOfRanges).map { i in
            Range(match.range(at: i), in: s).map { String(s[$0]) }
        }
    }

    /// `s.replace(re, with)` for a non-global regex: the first match only.
    func replaceFirst(_ s: String, with replacement: String) -> String {
        guard let match = regex.firstMatch(in: s, range: NSRange(s.startIndex..., in: s)),
              let range = Range(match.range, in: s) else { return s }
        return s.replacingCharacters(in: range, with: replacement)
    }

    /// `s.replace(/re/g, with)`.
    func replaceAll(_ s: String, with replacement: String) -> String {
        regex.stringByReplacingMatches(in: s, range: NSRange(s.startIndex..., in: s),
                                       withTemplate: NSRegularExpression.escapedTemplate(for: replacement))
    }

    /// JS `\s`: WhiteSpace + LineTerminator (ECMA-262), as ICU class contents.
    static let whitespaceClass = #"\t\n\x{0B}\f\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}"#
    private static let word = "[A-Za-z0-9_]"

    static func icu(_ js: String) -> String {
        let chars = Array(js)
        var out = ""
        var inClass = false
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c == "\\", i + 1 < chars.count {
                let n = chars[i + 1]
                i += 2
                switch n {
                case "s": out += inClass ? whitespaceClass : "[\(whitespaceClass)]"
                case "S": out += "[^\(whitespaceClass)]" // never used inside a class
                case "d": out += inClass ? "0-9" : "[0-9]"
                case "b" where !inClass:
                    out += "(?:(?<=\(word))(?!\(word))|(?<!\(word))(?=\(word)))"
                case "/": out += "/"
                default: out += "\\\(n)"
                }
                continue
            }
            if inClass {
                if c == "]" { inClass = false; out.append(c) }
                else if c == "-", i + 1 < chars.count, chars[i + 1] == "]" { out += "\\-" }
                else if c == "[" { out += "\\[" }
                else { out.append(c) }
            } else {
                switch c {
                case "[": inClass = true; out.append(c)
                case ".": out += #"[^\n\r\x{2028}\x{2029}]"#
                case "$": out += #"\z"#
                default: out.append(c)
                }
            }
            i += 1
        }
        return out
    }
}

/// JS `String.prototype.trim()`: strips the same set as `\s`.
func jsTrim(_ s: String) -> String {
    let scalars = s.unicodeScalars
    guard let first = scalars.firstIndex(where: { !isJSWhitespace($0) }) else { return "" }
    let last = scalars.lastIndex(where: { !isJSWhitespace($0) })!
    return String(scalars[first...last])
}

/// `!s.trim()`: the row is empty or whitespace only.
private func isBlank(_ s: String) -> Bool {
    s.unicodeScalars.allSatisfy(isJSWhitespace)
}

private func isJSWhitespace(_ u: Unicode.Scalar) -> Bool {
    switch u.value {
    case 0x09...0x0D, 0x20, 0xA0, 0x1680, 0x2000...0x200A, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF: return true
    default: return false
    }
}

/// `rows.slice(from, to)` with JS clamping.
private func slice(_ rows: [String], _ from: Int, _ to: Int) -> ArraySlice<String> {
    let lo = min(max(0, from), rows.count)
    let hi = min(max(lo, to), rows.count)
    return rows[lo..<hi]
}

// MARK: - Parser

enum AgentFrameParser {
    private static let BORDER = JSRegex(#"^\s*[─━═╌]{12,}\s*$"#)
    private static let BOX_TOP = JSRegex(#"^\s*[╭┌][─━]{8,}[╮┐]\s*$"#)
    private static let BOX_BOTTOM = JSRegex(#"^\s*[╰└][─━]{8,}[╯┘]\s*$"#)
    private static let BLOCK_BOTTOM = JSRegex(#"^\s*╹[▀━─]{8,}\s*$"#)
    private static let PROMPT = JSRegex(#"^\s*(?:[│┃]\s*)?[❯›>](?:\s|$)"#)
    private static let BLOCK_PROMPT = JSRegex(#"^\s*┃(?:\s|$)"#)
    // Dialogs are matched first, so a plain "esc to cancel" footer here means a turn is running (Antigravity).
    private static let WORKING = JSRegex(#"esc to interrupt|esc interrupt|esc to cancel|ctrl\+c to interrupt|press esc to stop"#, ignoreCase: true)
    private static let DIALOG_END = JSRegex(#"(esc to cancel|enter to confirm|enter confirm|enter continue|esc back|↑\/?↓\s*(?:to\s+)?navigate|enter select|tab to amend)"#, ignoreCase: true)
    private static let NUMBERED = JSRegex(#"^\s*(?:[❯›>]\s*)?(\d{1,2})[.)]\s+(.+?)\s*$"#)
    private static let SELECTED = JSRegex(#"^\s*[❯›>]\s"#)
    private static let MODE = JSRegex(#"(?:⏵⏵|⏸|›|>)\s*([a-z][a-z -]*?)\s+(?:mode\s+)?on\b"#, ignoreCase: true)
    private static let MODE_ALT = JSRegex(#"^\s*(?:>\s*)?(auto|plan|default|accept[- ]edits)\s+mode\b"#, ignoreCase: true)
    // Codex shows `Plan mode (⇧tab to cycle)` on the right of its footer row.
    private static let MODE_INLINE = JSRegex(#"\b(auto|plan|default|accept[- ]edits)\s+mode\s*\("#, ignoreCase: true)
    // opencode names its agent in the box: `┃  Build · <model>` or `┃  Plan · <model>`.
    private static let MODE_BLOCK = JSRegex(#"^\s*┃\s+(Build|Plan)\s+·"#)
    // Claude's right-aligned effort row just above its box (`● high · /effort`), not a wrapped menu line.
    private static let EFFORT_ROW = JSRegex(#"^\s{30,}(?:[●○◐◑◒◓]\s|\S.*\/effort\s*$)"#)
    // Inline patterns in agent-frame.ts.
    private static let INDENTED = JSRegex(#"^\s{2,}\S"#)
    private static let QUESTION_END = JSRegex(#"\?\s*$"#)
    private static let CODEX_PROMPT = JSRegex(#"^\s*›\s"#)
    private static let SPACES = JSRegex(#"\s+"#)

    private static let AGENT_NAMES: [(JSRegex, String)] = [
        (JSRegex(#"claude code"#, ignoreCase: true), "Claude Code"),
        (JSRegex(#"\bcodex\b"#, ignoreCase: true), "Codex"),
        (JSRegex(#"\bkimi\b|\bK2[.\d]"#, ignoreCase: true), "Kimi"),
        (JSRegex(#"antigravity"#, ignoreCase: true), "Antigravity"),
        (JSRegex(#"opencode"#, ignoreCase: true), "opencode"),
        (JSRegex(#"\bgizzi\b"#, ignoreCase: true), "Gizzi"),
        (JSRegex(#"\bgrok\b"#, ignoreCase: true), "Grok"),
        (JSRegex(#"\bgemini\b"#, ignoreCase: true), "Gemini"),
        (JSRegex(#"\bqwen\b"#, ignoreCase: true), "Qwen"),
    ]

    /// Names the agent from text on screen or the terminal title; nil when unknown.
    static func agentName(from text: String) -> String? {
        for (re, name) in AGENT_NAMES where re.test(text) { return name }
        return nil
    }

    private static func lastNonEmpty(_ rows: [String]) -> Int {
        var i = rows.count - 1
        while i >= 0 {
            if !isBlank(rows[i]) { return i }
            i -= 1
        }
        return -1
    }

    private static func modeFrom<C: Collection>(_ rows: C) -> String? where C.Element == String {
        for row in rows {
            let m = MODE.exec(row) ?? MODE_ALT.exec(row) ?? MODE_INLINE.exec(row) ?? MODE_BLOCK.exec(row)
            if let m, let group = m[1] {
                let label = SPACES.replaceAll(jsTrim(group), with: " ")
                guard let first = label.first else { return label }
                return first.uppercased() + label.dropFirst()
            }
        }
        return nil
    }

    private static func findDialog(_ rows: [String], _ last: Int) -> (top: Int, question: String, options: [AgentDialogOption])? {
        // The hint line closes the dialog and sits within the last few rows.
        var end = -1
        var i = last
        while i >= max(0, last - 3) {
            if DIALOG_END.test(rows[i]) { end = i; break }
            i -= 1
        }
        if end < 0 { return nil }
        var options: [AgentDialogOption] = []
        i = end - 1
        while i >= 0 && (isBlank(rows[i]) || (DIALOG_END.test(rows[i]) && end - i <= 3)) { i -= 1 }
        // Walk up over option rows: numbered, or selection-glyph / same-indent rows.
        var optionRows: [Int] = []
        while i >= 0 && optionRows.count < 9 {
            let row = rows[i]
            if isBlank(row) { break }
            if NUMBERED.test(row) || SELECTED.test(row) || INDENTED.test(row) { optionRows.insert(i, at: 0) }
            else { break }
            i -= 1
        }
        // Wrapped continuation rows of a long option are indented deeper; keep option heads only.
        let heads = optionRows.filter { NUMBERED.test(rows[$0]) || SELECTED.test(rows[$0]) || optionRows.count <= 4 }
        if heads.count < 2 { return nil }
        for (index, r) in heads.enumerated() {
            let row = rows[r]
            let num = NUMBERED.exec(row)
            options.append(AgentDialogOption(
                key: num?[1] ?? "",
                label: jsTrim(num?[2] ?? SELECTED.replaceFirst(row, with: "")),
                selected: SELECTED.test(row),
                index: index
            ))
        }
        // The question: a row ending in '?' just above the options (else the nearest row).
        var q = optionRows[0] - 1
        while q >= 0 && isBlank(rows[q]) { q -= 1 }
        var asked = -1
        var r = q
        while r >= max(0, optionRows[0] - 6) {
            if QUESTION_END.test(rows[r]) { asked = r; break }
            r -= 1
        }
        let qRow = asked >= 0 ? asked : q
        let question = qRow >= 0 ? jsTrim(rows[qRow]) : ""
        // Hide from the question only when it sits right above the options; else just the options.
        let top = asked >= 0 && optionRows[0] - asked <= 2 ? asked : optionRows[0]
        return (top, question, options)
    }

    /// Returns the input area's first row, or -1. Rows after the box beyond 3 footer rows are a menu: -2.
    private static func findInputTop(_ rows: [String], _ last: Int) -> Int {
        let floor = max(0, last - 16)
        let belowBox = { (bottom: Int) in slice(rows, bottom + 1, last + 1).filter { !isBlank($0) }.count }
        // Bordered box: last bottom border, a prompt row above it, a top border above that.
        var b2 = last
        while b2 >= floor {
            defer { b2 -= 1 }
            if !(BORDER.test(rows[b2]) || BOX_BOTTOM.test(rows[b2])) { continue }
            var b1 = b2 - 1
            while b1 >= max(0, b2 - 6) {
                if BORDER.test(rows[b1]) || BOX_TOP.test(rows[b1]) {
                    let inner = slice(rows, b1 + 1, b2)
                    if inner.contains(where: { PROMPT.test($0) }) {
                        if belowBox(b2) > 3 { return -2 }
                        // Claude draws a right-aligned status row (`● high · /effort`) just above.
                        let above = b1 - 1
                        if above >= 0 && EFFORT_ROW.test(rows[above]) { return above }
                        return b1
                    }
                    break
                }
                b1 -= 1
            }
        }
        // opencode: a ┃ block closed by ╹▀▀▀.
        var b = last
        while b >= floor {
            defer { b -= 1 }
            if !BLOCK_BOTTOM.test(rows[b]) { continue }
            var top = b - 1
            while top - 1 >= 0 && BLOCK_PROMPT.test(rows[top - 1]) { top -= 1 }
            // JS reads rows[-1] as undefined (no match); Swift must not index it.
            if top >= 0 && top < b && BLOCK_PROMPT.test(rows[top]) { return top }
        }
        // Codex: a borderless `›` prompt near the bottom with only footer rows below.
        var p = last
        while p >= max(0, last - 5) {
            if CODEX_PROMPT.test(rows[p]) { return p }
            p -= 1
        }
        return -1
    }

    /// Parses the visible rows (top to bottom, one string per row). `title` is the
    /// terminal title, used to name the agent when the banner has scrolled away.
    static func parse(_ rows: [String], title: String = "") -> AgentFrame? {
        let last = lastNonEmpty(rows)
        if last < 2 { return nil }
        let tail = slice(rows, max(0, last - 14), last + 1)
        let agent = agentName(from: title) ?? agentName(from: rows.joined(separator: "\n"))

        if let dialog = findDialog(rows, last) {
            return AgentFrame(
                agent: agent,
                hideFrom: dialog.top,
                status: .needsYou,
                dialog: AgentDialog(question: dialog.question, options: dialog.options),
                mode: modeFrom(tail)
            )
        }

        let top = findInputTop(rows, last)
        if top == -2 {
            // A menu (slash commands, models) is open under the box: show the whole screen.
            return AgentFrame(agent: agent, hideFrom: rows.count, status: .idle, dialog: nil, mode: nil)
        }
        if top < 0 { return nil }
        let footer = slice(rows, top, last + 1)
        // The spinner row ("✻ Brewing… (21s · esc to interrupt)") sits just above the box; it stays visible.
        let near = slice(rows, max(0, top - 3), last + 1).joined(separator: "\n")
        return AgentFrame(
            agent: agent,
            hideFrom: top,
            status: WORKING.test(near) ? .working : .idle,
            dialog: nil,
            mode: modeFrom(footer)
        )
    }
}

// MARK: - Keys (agent-frame.ts)

enum AgentKeys {
    static let escape = "\u{1b}"
    /// shift+tab: cycles the agent's permission mode.
    static let cycleMode = "\u{1b}[Z"

    /// Bracketed paste + Enter: how a prompt is submitted to a TUI agent
    /// (send the paste, then Enter ~60 ms later, as CodeTerminalCanvas does).
    static func promptInput(_ text: String) -> (paste: String, enter: String) {
        ("\u{1b}[200~\(text)\u{1b}[201~", "\r")
    }

    /// Keys that answer a dialog option: its digit, or arrows from the selected row then Enter.
    static func dialogKeys(_ options: [AgentDialogOption], pick: AgentDialogOption) -> String {
        if !pick.key.isEmpty { return pick.key }
        let from = options.first(where: \.selected)?.index ?? 0
        let delta = pick.index - from
        let arrow = delta > 0 ? "\u{1b}[B" : "\u{1b}[A"
        return String(repeating: arrow, count: abs(delta)) + "\r"
    }
}

// MARK: - Composer text (agent-composer.tsx)

enum AgentComposerText {
    /// The composer's placeholder for the agent's state.
    static func placeholder(_ frame: AgentFrame) -> String {
        let name = frame.agent ?? "the agent"
        switch frame.status {
        case .working: return "Queue a message for \(name)…"
        case .needsYou: return "Or tell \(name) what to do instead…"
        case .idle: return "Message \(name)…"
        }
    }

    static func status(_ status: AgentFrame.Status) -> String {
        switch status {
        case .idle: return "Idle"
        case .working: return "Working"
        case .needsYou: return "Needs you"
        }
    }

    /// "Yes, and switch to accept edits (auto-approve …) for this session (shift+tab)" → "Yes, and switch to accept edits".
    static func shortLabel(_ label: String) -> String {
        let cut = jsTrim(shortLabelTail.replaceFirst(label, with: ""))
        return cut.isEmpty ? label : cut
    }
    private static let shortLabelTail = JSRegex(#"\s*\(.*$"#)

    /// "Bypass permissions" → "Bypass"; "Accept edits" stays.
    static func shortMode(_ mode: String) -> String {
        let m = modeTail.replaceFirst(mode, with: "")
        guard let first = m.first else { return m }
        return first.uppercased() + m.dropFirst()
    }
    private static let modeTail = JSRegex(#"\s+(permissions|mode)$"#, ignoreCase: true)

    /// A permission prompt (lock icon, "Permission required") rather than a plain choice.
    static func isPermission(_ question: String) -> Bool {
        permissionWords.test(question)
    }
    private static let permissionWords = JSRegex(#"\b(allow|permission|want to|proceed|approve|trust)\b"#, ignoreCase: true)
}
