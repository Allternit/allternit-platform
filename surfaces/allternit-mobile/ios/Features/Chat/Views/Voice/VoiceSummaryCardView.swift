import SwiftUI

/// Local-only record of a finished voice-mode session (Phase 7b) — the
/// "Voice chat ended · Ns" card filed into the text thread when the voice
/// takeover closes. The conversation itself went through the normal
/// sendMessage stream path, so the card is just a duration marker with
/// feedback affordances.
///
/// LOCAL-ONLY: the backend has no summary-card message type (like the
/// message action bar's 👍/👎, verified against cmd/allternit-api routes,
/// 2026-07), so the row never leaves the device and a history reload drops
/// it.
struct VoiceSummary: Equatable, Sendable {
    var durationSeconds: Int
    var endedAt: Date = Date()

    /// "Voice chat ended · 41s" / "· 2m 5s" duration label.
    var formattedDuration: String {
        let minutes = durationSeconds / 60
        let seconds = durationSeconds % 60
        if minutes > 0 {
            return seconds > 0 ? "\(minutes)m \(seconds)s" : "\(minutes)m"
        }
        return "\(seconds)s"
    }
}

/// Inline feed card for a VoiceSummary (Claude iOS parity): waveform glyph,
/// "Voice chat ended · Ns", and local-only 👍/👎 feedback for the session.
struct VoiceSummaryCardView: View {
    let summary: VoiceSummary

    /// Session feedback. LOCAL-ONLY: no backend feedback endpoint exists —
    /// the choice is visual acknowledgement until one ships (same pattern
    /// as MessageActionBar).
    @State private var feedback: Int = 0 // -1 down, 0 none, +1 up

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "waveform")
                .font(.system(size: 14, weight: .semibold))
                .foregroundColor(Color("AccentPrimary"))
                .frame(width: 36, height: 36)
                .background(Color("AccentPrimary").opacity(0.14))
                .clipShape(Circle())

            VStack(alignment: .leading, spacing: 2) {
                Text("Voice chat ended · \(summary.formattedDuration)")
                    .font(.subheadline)
                    .fontWeight(.semibold)
                    .foregroundColor(Color("TextPrimary"))

                Text("The conversation continues below")
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer()

            feedbackButton(feedback == 1 ? "hand.thumbsup.fill" : "hand.thumbsup", label: "Good session") {
                feedback = feedback == 1 ? 0 : 1
            }
            feedbackButton(feedback == -1 ? "hand.thumbsdown.fill" : "hand.thumbsdown", label: "Bad session") {
                feedback = feedback == -1 ? 0 : -1
            }
        }
        .padding(12)
        .frame(maxWidth: 360)
        .background(Color("BgPrimary"))
        .cornerRadius(12)
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
    }

    private func feedbackButton(_ systemName: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: {
            Haptics.selection()
            action()
        }) {
            Image(systemName: systemName)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 30, height: 30)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}
