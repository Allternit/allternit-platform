import SwiftUI

/// Follow-up suggestions under the newest reply. Real ones only, the same rule as
/// web `lib/chat/follow-ups.ts`: the answer's own closing "Next step:" line. No canned
/// prompts: a reply without one shows nothing. (iOS does not receive the stream's
/// `follow-ups` part, so the text line is the only source here.)
enum FollowUps {
    static let maxCount = 3
    private static let minChars = 3
    private static let maxChars = 200

    /// "Next step:" / "**Next steps:**" / "> Next step - ...", as a bullet or heading too.
    private static let nextStep = try! NSRegularExpression(
        pattern: #"^\s*(?:[-*>]\s+|#{1,6}\s+)?(?:\*\*|__)?\s*next\s+steps?\s*(?:\*\*|__)?\s*(?::|—|–|-)?\s*(?:\*\*|__)?\s*(.*)$"#,
        options: [.caseInsensitive]
    )

    static func plainLine(_ line: String) -> String {
        var s = line.replacingOccurrences(of: #"^\s*(?:[-*+]\s+|\d+[.)]\s+)"#, with: "", options: .regularExpression)
        s = s.replacingOccurrences(of: #"\[([^\]]+)\]\([^)]*\)"#, with: "$1", options: .regularExpression)
        s = s.replacingOccurrences(of: #"(\*\*|__|\*|_|`)"#, with: "", options: .regularExpression)
        s = s.replacingOccurrences(of: #"\s+"#, with: " ", options: .regularExpression)
        return s.trimmingCharacters(in: .whitespaces)
    }

    /// The closing "Next step:" of an answer, as one follow-up.
    static func fromText(_ text: String) -> [String] {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
            .map { String($0).trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        let tail = Array(lines.suffix(6))
        for i in stride(from: tail.count - 1, through: 0, by: -1) {
            let line = tail[i]
            let range = NSRange(line.startIndex..., in: line)
            guard let m = nextStep.firstMatch(in: line, range: range) else { continue }
            let captured = Range(m.range(at: 1), in: line).map { String(line[$0]) } ?? ""
            let raw = captured.isEmpty ? (i + 1 < tail.count ? tail[i + 1] : "") : captured
            let body = plainLine(raw)
            return (body.count >= minChars && body.count <= maxChars) ? [body] : []
        }
        return []
    }
}

/// A right-aligned pill stack under the reply: pills arrive one by one (about 180 ms
/// apart); a pick flies up and stays as the user's message while the rest fade.
/// Reduced motion: no stagger, no fly, the pick just sends.
struct FollowUpChips: View {
    let prompts: [String]
    let onPick: (String) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var shown = 0
    @State private var picked: String? = nil

    var body: some View {
        VStack(alignment: .trailing, spacing: 8) {
            ForEach(Array(prompts.prefix(FollowUps.maxCount).enumerated()), id: \.offset) { index, prompt in
                let isPicked = picked == prompt
                let visible = reduceMotion || index < shown
                Button {
                    pick(prompt)
                } label: {
                    Text(prompt)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .multilineTextAlignment(.trailing)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 9)
                        .background(Capsule(style: .continuous).fill(Color("BgSecondary")))
                        .overlay(Capsule(style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .disabled(picked != nil)
                .accessibilityLabel("Follow-up: \(prompt)")
                .opacity(picked == nil ? (visible ? 1 : 0) : (isPicked ? 1 : 0))
                .offset(y: isPicked ? -24 : (visible || reduceMotion ? 0 : 10))
                .scaleEffect(isPicked ? 1.03 : 1, anchor: .trailing)
            }
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.top, 4)
        .task(id: prompts) {
            guard !reduceMotion else { shown = prompts.count; return }
            shown = 0
            for i in 0..<min(prompts.count, FollowUps.maxCount) {
                try? await Task.sleep(nanoseconds: 180_000_000)
                if Task.isCancelled { return }
                withAnimation(.spring(response: 0.35, dampingFraction: 0.85)) { shown = i + 1 }
            }
        }
    }

    private func pick(_ prompt: String) {
        guard picked == nil else { return }
        Haptics.success()
        if reduceMotion {
            picked = prompt
            onPick(prompt)
            return
        }
        withAnimation(.spring(response: 0.4, dampingFraction: 0.82)) { picked = prompt }
        Task {
            try? await Task.sleep(nanoseconds: 260_000_000)
            onPick(prompt)
        }
    }
}
