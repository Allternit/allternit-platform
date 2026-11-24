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
/// chevron, state dot. Pure view: the caller owns state and the sheet it opens.
struct RuntimeBrainChip: View {
    let runtimeId: String
    let runtimeName: String
    let brainName: String
    var state: RuntimeBrainChipState = .ready
    var warning: String? = nil
    /// Vendor name when the brain is fixed by the runtime ("Run by <vendor>").
    var lockedVendor: String? = nil
    /// The surface the chip sits on, so the chip blends into it.
    var background: Color = Color("BgPrimary")
    var action: () -> Void = {}

    private var brainText: String {
        if let v = lockedVendor { return "Run by \(v)" }
        return brainName
    }

    private var accessibilitySummary: String {
        var parts = ["Runtime \(runtimeName)", lockedVendor.map { "run by \($0)" } ?? "brain \(brainName)", state.label]
        if let w = warning, !w.isEmpty { parts.append(w) }
        return parts.joined(separator: ", ")
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                ModelProviderMark(providerId: runtimeId, size: 16)
                Text(runtimeName)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Rectangle()
                    .fill(Color("BorderSubtle"))
                    .frame(width: 1, height: 12)
                Text(brainText)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                    .truncationMode(.middle)
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
            .padding(.horizontal, 8)
            .frame(height: 24)
            .background(background, in: RoundedRectangle(cornerRadius: 8))
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
}

#Preview("Runtime brain chip") {
    VStack(alignment: .leading, spacing: 8) {
        RuntimeBrainChip(runtimeId: "allternit", runtimeName: "Allternit", brainName: "claude/opus")
        RuntimeBrainChip(runtimeId: "claude-code", runtimeName: "Claude Code", brainName: "Opus", lockedVendor: "Anthropic")
        RuntimeBrainChip(runtimeId: "codex", runtimeName: "Codex", brainName: "gpt-5", state: .running)
        RuntimeBrainChip(runtimeId: "qwen-cli", runtimeName: "Qwen Code", brainName: "Pick a brain", state: .needsStep, warning: "No brain")
        RuntimeBrainChip(runtimeId: "codebuddy", runtimeName: "CodeBuddy", brainName: "Any brain")
        RuntimeBrainChip(runtimeId: "unknown-x", runtimeName: "Other", brainName: "Any brain")
    }
    .padding()
    .background(Color("BgPrimary"))
}
