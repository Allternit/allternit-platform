import SwiftUI
import UIKit

/// TV-style captions for live mode (allternit-ai VoiceCaptions.tsx): a
/// frosted dark band, the speaker's chip in their colour (or "You" with a
/// live dot), the bot's words appearing at a speaking pace, and past two
/// lines the text rolls up with the older line fading at the top. Between
/// turns a quiet "is thinking" line keeps the band from going blank.
/// VoiceOver reads the whole caption once (the band is one element).
struct VoiceCaptionsView<Avatar: View>: View {
    let you: Bool
    let name: String
    let color: Color?
    let text: String
    var quiet: String?
    var size: CaptionSize = .medium
    @ViewBuilder var avatar: () -> Avatar

    @State private var shown = 0
    @State private var lastText = ""
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var words: [Substring] { text.split(whereSeparator: \.isWhitespace) }
    private var visibleText: String {
        let n = you || reduceMotion ? words.count : min(shown, words.count)
        return words.prefix(n).joined(separator: " ")
    }
    private var lineHeight: CGFloat { size.points * 1.38 }
    private var chipColor: Color { color ?? Color("AccentPrimary") }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            chip
            lines
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, 12)
        .frame(maxWidth: 640, alignment: .leading)
        .background(band)
        .environment(\.colorScheme, .dark)
        .opacity(isEmpty ? 0 : 1)
        .animation(.easeOut(duration: 0.35), value: isEmpty)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spokenLabel)
        .onChange(of: text, initial: true) { _, new in
            if !new.hasPrefix(lastText) { shown = 0 }
            lastText = new
        }
        .task(id: "\(text)|\(shown)") {
            guard !you, shown < words.count else { return }
            try? await Task.sleep(nanoseconds: 170_000_000)
            if !Task.isCancelled { shown += 1 }
        }
    }

    private var isEmpty: Bool { text.isEmpty && quiet == nil }
    private var spokenLabel: String { text.isEmpty ? (quiet ?? "") : "\(you ? "You" : name): \(text)" }

    private var chip: some View {
        HStack(spacing: 6) {
            chipMark
            Text((you ? "You" : name).uppercased())
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.9)
        }
        .padding(.leading, you ? 8 : 3)
        .padding(.trailing, 8)
        .frame(height: 22)
        .foregroundColor(you ? Color.white.opacity(0.85) : chipColor.mix(with: .white, by: 0.45))
        .background(Capsule().fill(you ? Color.white.opacity(0.12) : chipColor.opacity(0.22)))
        .id(you ? "you" : name)
        .transition(.opacity.combined(with: .offset(y: 4)))
    }

    @ViewBuilder
    private var chipMark: some View {
        if you {
            Circle().fill(Color(hex: "#ff5a4e")).frame(width: 6, height: 6)
                .shadow(color: Color(hex: "#ff5a4e").opacity(0.5), radius: 3)
        } else {
            avatar()
                .frame(width: 18, height: 18)
                .background(Circle().fill(Color.white.opacity(0.92)))
                .clipShape(Circle())
        }
    }

    private var lines: some View {
        linesContent
            .frame(height: lineHeight * 2, alignment: .bottom)
            .clipped()
            .mask(topFade)
    }

    @ViewBuilder
    private var linesContent: some View {
        if text.isEmpty, let quiet {
            Text("\(quiet) …")
                .italic()
                .font(.system(size: size.points * 0.88, weight: .medium))
                .foregroundColor(Color.white.opacity(0.55))
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            // Newest lines at the bottom; older ones are clipped off the top.
            Text(visibleText)
                .font(.system(size: size.points, weight: .medium))
                .lineSpacing(size.points * 0.38 - 4)
                .foregroundColor(you ? Color.white.opacity(0.78) : Color.white)
                .shadow(color: Color.black.opacity(0.25), radius: 0.5, y: 1)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(.easeOut(duration: 0.28), value: visibleText)
        }
    }

    @ViewBuilder
    private var topFade: some View {
        if wordsOverflow {
            LinearGradient(stops: [.init(color: Color.black.opacity(0), location: 0), .init(color: Color.black, location: 0.34)],
                           startPoint: .top, endPoint: .bottom)
        } else {
            Color.black
        }
    }

    private var band: some View {
        let shape = RoundedRectangle(cornerRadius: 16, style: .continuous)
        let top = Color(red: 0.11, green: 0.11, blue: 0.12).opacity(0.80)
        let bottom = Color(red: 0.07, green: 0.07, blue: 0.08).opacity(0.88)
        return shape
            .fill(LinearGradient(colors: [top, bottom], startPoint: .top, endPoint: .bottom))
            .background(.ultraThinMaterial, in: shape)
            .overlay(shape.strokeBorder(Color.white.opacity(0.08)))
            .shadow(color: Color.black.opacity(0.18), radius: 15, y: 10)
    }

    /// Rough check for more than two lines, for the top fade only.
    private var wordsOverflow: Bool {
        let charsPerLine = max(18, Int(300 / (size.points * 0.55)))
        return visibleText.count > charsPerLine * 2
    }
}

private extension Color {
    /// A blend toward `other` (0 = self, 1 = other).
    func mix(with other: Color, by amount: Double) -> Color {
        let a = UIColor(self), b = UIColor(other)
        var r1: CGFloat = 0, g1: CGFloat = 0, b1: CGFloat = 0, a1: CGFloat = 0
        var r2: CGFloat = 0, g2: CGFloat = 0, b2: CGFloat = 0, a2: CGFloat = 0
        a.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        b.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        let k = CGFloat(amount)
        return Color(red: r1 + (r2 - r1) * k, green: g1 + (g2 - g1) * k, blue: b1 + (b2 - b1) * k)
    }
}
