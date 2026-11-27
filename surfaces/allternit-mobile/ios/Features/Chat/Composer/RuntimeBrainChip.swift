import SwiftUI

/// State of the runtime + brain pair shown on the chip's dot.
enum RuntimeBrainChipState {
    case ready      // green: runtime and brain are set
    case needsStep  // amber: a runtime or brain step is missing
    case running    // coral: a turn is running

    var label: String {
        switch self {
        case .ready: return "ready"
        case .needsStep: return "needs a step"
        case .running: return "running"
        }
    }

    var color: Color {
        switch self {
        case .ready: return Color(red: 0.48, green: 0.77, blue: 0.48)
        case .needsStep: return Color(red: 0.90, green: 0.68, blue: 0.25)
        case .running: return Color(red: 0.91, green: 0.51, blue: 0.42)
        }
    }
}

/// One chip for "runtime | brain": brand mark, runtime name, divider, brain,
/// chevron, state dot. No box: it sits on a line and the host leaves the
/// line's gap (the composer card's border, Code's prompt rule). Pure view:
/// the caller owns state and the sheet it opens.
struct RuntimeBrainChip: View {
    let runtimeId: String
    let runtimeName: String
    let brainName: String
    var state: RuntimeBrainChipState = .ready
    var warning: String? = nil
    /// Vendor name when the brain is fixed by the runtime ("Run by <vendor>").
    var lockedVendor: String? = nil
    /// Start anywhere: the picked computer, shown as a leading segment
    /// (icon + name, truncated) only when a computer is picked.
    var computerName: String? = nil
    var computerIcon: String = "desktopcomputer"
    var action: () -> Void = {}

    private var brainText: String {
        if let v = lockedVendor { return "Run by \(v)" }
        return brainName
    }

    private var accessibilitySummary: String {
        var parts = computerName.map { ["Computer \($0)"] } ?? []
        parts += ["Runtime \(runtimeName)", lockedVendor.map { "run by \($0)" } ?? "brain \(brainName)", state.label]
        if let w = warning, !w.isEmpty { parts.append(w) }
        return parts.joined(separator: ", ")
    }

    var body: some View {
        Button(action: action) {
            // With a computer segment the brain text goes first when the
            // chip runs out of room on a compact width.
            ViewThatFits(in: .horizontal) {
                content(showsBrain: true)
                if computerName != nil { content(showsBrain: false) }
            }
            .padding(.horizontal, 8)
            .frame(height: 24)
            .frame(minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilitySummary)
        .accessibilityHint("Opens runtime and brain")
        .accessibilityAddTraits(.isButton)
        .accessibilityIdentifier("composerChip-runtimeBrain")
    }

    @ViewBuilder
    private func content(showsBrain: Bool) -> some View {
        HStack(spacing: 6) {
            if let computerName {
                Image(systemName: computerIcon)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                Text(computerName)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: 90, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                Text("·")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
            ModelProviderMark(providerId: runtimeId, size: 16)
            Text(runtimeName)
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            if showsBrain {
                Rectangle()
                    .fill(Color("BorderSubtle"))
                    .frame(width: 1, height: 12)
                Text(brainText)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            Image(systemName: "chevron.down")
                .font(.system(size: 9, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
            if warning != nil {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 10))
                    .foregroundColor(RuntimeBrainChipState.needsStep.color)
            }
            Circle()
                .fill(state.color)
                .frame(width: 6, height: 6)
        }
    }
}

#Preview("Runtime brain chip") {
    VStack(alignment: .leading, spacing: 8) {
        RuntimeBrainChip(runtimeId: "allternit", runtimeName: "Allternit", brainName: "claude/opus")
        RuntimeBrainChip(runtimeId: "allternit", runtimeName: "Gizzi", brainName: "Opus", computerName: "MacBook Pro")
        RuntimeBrainChip(runtimeId: "allternit", runtimeName: "Gizzi", brainName: "Opus", computerName: "Mac mini", computerIcon: "moon")
            .frame(width: 200)
        RuntimeBrainChip(runtimeId: "claude-code", runtimeName: "Claude Code", brainName: "Opus", lockedVendor: "Anthropic")
        RuntimeBrainChip(runtimeId: "codex", runtimeName: "Codex", brainName: "gpt-5", state: .running)
        RuntimeBrainChip(runtimeId: "qwen-cli", runtimeName: "Qwen Code", brainName: "Pick a brain", state: .needsStep, warning: "No brain")
        RuntimeBrainChip(runtimeId: "codebuddy", runtimeName: "CodeBuddy", brainName: "Any brain")
        RuntimeBrainChip(runtimeId: "unknown-x", runtimeName: "Other", brainName: "Any brain")
    }
    .padding()
    .background(Color("BgPrimary"))
}
