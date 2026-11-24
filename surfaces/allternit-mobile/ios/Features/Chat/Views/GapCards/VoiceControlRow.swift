import SwiftUI

/// Voice card: shows what the voice_control tool changed.
struct VoiceControlRow: View {
    let gap: GapToolMetadata

    /// What changed, in words: "Language en", "Speed 1.2x", "Replies as text", "Call ended".
    private var line: String {
        var parts: [String] = []
        if let language = gap.language, !language.isEmpty { parts.append("Language \(language)") }
        if let speed = gap.speed { parts.append("Speed " + String(format: "%.1fx", speed)) }
        if let mode = gap.replyMode { parts.append(mode == "text" ? "Replies as text" : mode == "both" ? "Replies spoken and as text" : "Replies spoken") }
        if gap.action == "end_call" { parts.append("Call ends after this reply") }
        if parts.isEmpty, let action = gap.action { parts.append(action) }
        return parts.joined(separator: " \u{00B7} ")
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "waveform")
                    .foregroundColor(Color("AccentPrimary"))
                Text("Voice")
                    .foregroundColor(Color("TextSecondary"))
            }
            .font(.caption.weight(.semibold))
            if !line.isEmpty {
                Text(line)
                    .font(.footnote)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .accessibilityElement(children: .combine)
    }
}
