import Foundation

/// Splits streamed reply text into speakable sentences (the local engine
/// speaks sentence by sentence as `speak.delta` arrives — the same idea as
/// the server's splitter). Boundaries: `.`/`!`/`?`/`…`/newline. A `.` only
/// ends a sentence when whitespace follows (so `3.5` and `e.g.x` stay whole),
/// and a trailing boundary waits for the next character before it fires.
/// Markdown emphasis, code ticks, headings and bullets are stripped. Text
/// with no boundary for `maxLength` characters is cut at a space.
struct SpeechSentenceChunker: Sendable {
    static let maxLength = 280
    private var buffer = ""

    mutating func push(_ text: String) -> [String] {
        buffer += text
        var sentences: [String] = []
        let chars = Array(buffer)
        var start = 0
        var index = 0
        while index < chars.count {
            let c = chars[index]
            var boundary = false
            if c == "\n" {
                boundary = true
            } else if c == "!" || c == "?" || c == "…" || c == "." {
                guard index + 1 < chars.count else { break }  // wait for the next char
                let next = chars[index + 1]
                if c == "." {
                    boundary = next.isWhitespace
                } else {
                    boundary = next.isWhitespace || next == "\"" || next == "'" || next == ")"
                }
            } else if index - start >= Self.maxLength, c == " " {
                boundary = true
            }
            if boundary {
                if let cleaned = Self.clean(String(chars[start...index])) { sentences.append(cleaned) }
                start = index + 1
            }
            index += 1
        }
        buffer = String(chars[start...])
        return sentences
    }

    /// The unfinished tail at `speak.done`.
    mutating func flush() -> String? {
        defer { buffer = "" }
        return Self.clean(buffer)
    }

    /// Strips markdown noise; nil when nothing speakable remains.
    static func clean(_ raw: String) -> String? {
        var text = raw
        for token in ["**", "__", "`", "```"] { text = text.replacingOccurrences(of: token, with: "") }
        text = text.replacingOccurrences(of: "*", with: "")
        var lines: [String] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: true) {
            var l = line.trimmingCharacters(in: .whitespaces)
            while l.hasPrefix("#") { l.removeFirst() }
            if l.hasPrefix("- ") || l.hasPrefix("• ") { l.removeFirst(2) }
            l = l.trimmingCharacters(in: .whitespaces)
            if !l.isEmpty { lines.append(l) }
        }
        let joined = lines.joined(separator: " ")
        // Needs at least one letter or digit to be worth synthesizing.
        return joined.contains(where: { $0.isLetter || $0.isNumber }) ? joined : nil
    }
}
