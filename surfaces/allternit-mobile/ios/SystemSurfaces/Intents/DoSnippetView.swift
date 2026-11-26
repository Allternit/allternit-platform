import SwiftUI

// Snippets for "Do with Allternit" (DoIntents.swift): the review card shown
// in the confirmation, then the result card. Drawn by Siri, Shortcuts and
// Spotlight / iOS 27 "Search or Ask", so system colors only (they follow the
// system's light or dark mode) and no app assets. Layout follows the
// "Search or Ask" card in the island-bar mockups (B-ios-island.jsx), whose
// system chrome is assumed, not published by Apple.

/// The review: what will run, the values, and the owner rule when one applies.
struct DoReviewSnippet: View {
    var appName: String
    var toolTitle: String
    var level: String
    var review: BarReviewInfo

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            DoSnippetHeader(title: "\(toolTitle) on \(appName)",
                            subtitle: ConnectionWording.level(level.isEmpty ? review.level : level))
            if !review.lines.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(review.lines, id: \.self) { line in
                        HStack(alignment: .firstTextBaseline) {
                            Text(line.label).foregroundStyle(.secondary)
                            Spacer(minLength: 12)
                            Text(line.value).fontWeight(.medium).multilineTextAlignment(.trailing)
                        }
                        .font(.subheadline)
                    }
                }
            }
            if let note = DoWording.ruleNote(review) {
                HStack(alignment: .top, spacing: 7) {
                    Image(systemName: "checkmark.shield")
                        .font(.footnote)
                        .accessibilityHidden(true)
                    Text(note).font(.footnote)
                }
                .padding(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            }
        }
        .padding()
        .accessibilityElement(children: .combine)
    }
}

/// The result: done (with the receipt), sent to someone to approve, still
/// working, or failed.
struct DoResultSnippet: View {
    var appName: String
    var toolTitle: String
    var outcome: BarRunOutcome

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            DoSnippetHeader(title: "\(toolTitle) on \(appName)", subtitle: nil)
            HStack(spacing: 8) {
                Image(systemName: symbol)
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
                Text(DoWording.outcome(outcome))
                    .font(.subheadline.weight(.semibold))
            }
            if case .waitingApproval = outcome {
                Text("You can follow it on the Live Activity or in Allternit.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        .padding()
        .accessibilityElement(children: .combine)
    }

    private var symbol: String {
        switch outcome {
        case .completed: return "checkmark.circle.fill"
        case .working: return "cpu"
        case .waitingApproval: return "hand.raised.fill"
        case .needsInput: return "questionmark.circle.fill"
        case .failed: return "exclamationmark.triangle.fill"
        case .cancelled: return "xmark.circle.fill"
        }
    }

    private var tint: Color {
        switch outcome {
        case .completed: return .green
        case .working: return .blue
        case .waitingApproval, .needsInput: return .orange
        case .failed: return .red
        case .cancelled: return .secondary
        }
    }
}

private struct DoSnippetHeader: View {
    var title: String
    var subtitle: String?

    var body: some View {
        HStack(spacing: 9) {
            Text("A://")
                .font(.system(size: 10, weight: .heavy, design: .monospaced))
                .foregroundStyle(Color(uiColor: .systemBackground))
                .frame(width: 34, height: 34)
                .background(Color.primary, in: RoundedRectangle(cornerRadius: 9, style: .continuous))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(title).font(.headline).lineLimit(2)
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }
}
