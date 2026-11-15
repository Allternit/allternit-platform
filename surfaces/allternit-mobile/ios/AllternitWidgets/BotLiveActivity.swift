import ActivityKit
import WidgetKit
import SwiftUI

/// Lock Screen banner + Dynamic Island presentation, driven by
/// `BotLiveActivityManager` in the main app target and by cloud-api pushes.
/// Option A, bot first: the focus bot (name, step, progress, "+N others");
/// when it needs the owner, the question, with Approve / Deny only for an
/// inline question and Open otherwise. Final layout comes with plan I10.
struct BotLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: BotActivityAttributes.self) { context in
            LockScreenView(context: context)
                .activityBackgroundTint(Color("BgPanel"))
                .activitySystemActionForegroundColor(Color("TextPrimary"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    statusIcon(context: context, size: 34)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(countText(context: context))
                        .font(.caption)
                        .foregroundColor(.white)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    if context.state.question?.isEmpty == false {
                        QuestionView(state: context.state, onDark: true)
                    } else if let displayName = context.state.displayName, !displayName.isEmpty {
                        Text(Self.botLine(context.state, name: displayName))
                            .font(.caption)
                            .foregroundColor(.white.opacity(0.8))
                            .lineLimit(1)
                    } else {
                        Text(detailText(context: context))
                            .font(.system(.caption, design: .monospaced))
                            .foregroundColor(.white.opacity(0.8))
                            .lineLimit(1)
                    }
                }
            } compactLeading: {
                statusIcon(context: context, size: 18)
            } compactTrailing: {
                Text(countText(context: context))
                    .font(.caption2)
                    .foregroundColor(.white)
            } minimal: {
                statusIcon(context: context, size: 16)
            }
        }
        .smallActivityFamilyIfAvailable()
    }

    private func countText(context: ActivityViewContext<BotActivityAttributes>) -> String {
        if context.state.attentionBotsCount > 0 {
            return "\(context.state.attentionBotsCount)"
        }
        return "\(context.state.activeBotsCount)"
    }

    private func detailText(context: ActivityViewContext<BotActivityAttributes>) -> String {
        if let label = context.state.activityLabel, !label.isEmpty {
            return label
        }
        return Self.statusLabel(context.state.status)
    }

    @ViewBuilder
    private func statusIcon(context: ActivityViewContext<BotActivityAttributes>, size: CGFloat) -> some View {
        let color = Self.statusColor(context.state.status)
        Image(systemName: Self.statusIconName(context.state.status))
            .font(.system(size: size * 0.55, weight: .semibold))
            .foregroundColor(color)
            .frame(width: size, height: size)
            .background(color.opacity(0.15))
            .clipShape(Circle())
    }

    /// "Raven · Drafting the reply · +2 others"
    fileprivate static func botLine(_ state: BotActivityAttributes.ContentState, name: String) -> String {
        var parts = [name]
        if let step = state.step, !step.isEmpty { parts.append(step) }
        if let others = othersText(state) { parts.append(others) }
        return parts.joined(separator: " · ")
    }

    fileprivate static func othersText(_ state: BotActivityAttributes.ContentState) -> String? {
        guard let others = state.othersCount, others > 0 else { return nil }
        return others == 1 ? "+1 other" : "+\(others) others"
    }

    // MARK: - Status mapping (mirrors BotOperationalStatus in the app target)

    fileprivate static func statusLabel(_ status: String) -> String {
        switch status {
        case "idle": return "Idle"
        case "working": return "Working"
        case "waiting_input": return "Needs input"
        case "waiting_approval": return "Needs approval"
        case "blocked": return "Blocked"
        case "offline": return "Offline"
        case "degraded": return "Degraded"
        case "failed": return "Failed"
        case "completed": return "Completed"
        default: return status.capitalized
        }
    }

    fileprivate static func statusColor(_ status: String) -> Color {
        switch status {
        case "working": return .blue
        case "completed", "idle": return .green
        case "waiting_input", "waiting_approval", "blocked", "degraded": return .orange
        case "failed": return .red
        case "offline": return .gray
        default: return .gray
        }
    }

    fileprivate static func statusIconName(_ status: String) -> String {
        switch status {
        case "working": return "cpu"
        case "waiting_input": return "text.bubble"
        case "waiting_approval": return "hand.raised"
        case "blocked": return "exclamationmark.triangle"
        case "failed": return "xmark.octagon"
        case "completed": return "checkmark.circle"
        case "degraded": return "exclamationmark.triangle"
        case "offline": return "wifi.slash"
        default: return "cpu"
        }
    }
}

private struct LockScreenView: View {
    let context: ActivityViewContext<BotActivityAttributes>

    var body: some View {
        HStack(spacing: 12) {
            let color = BotLiveActivityWidget.statusColor(context.state.status)
            Image(systemName: BotLiveActivityWidget.statusIconName(context.state.status))
                .font(.system(size: 22, weight: .semibold))
                .foregroundColor(color)
                .frame(width: 40, height: 40)
                .background(color.opacity(0.15))
                .clipShape(Circle())

            VStack(alignment: .leading, spacing: 2) {
                if context.state.question?.isEmpty == false {
                    if let displayName = context.state.displayName, !displayName.isEmpty {
                        Text(displayName)
                            .font(.system(.subheadline, design: .monospaced))
                            .foregroundColor(Color("TextPrimary"))
                            .lineLimit(1)
                    }
                    QuestionView(state: context.state, onDark: false)
                } else if let displayName = context.state.displayName, !displayName.isEmpty {
                    Text(displayName)
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Text(focusSubtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                    if let progress = context.state.progress {
                        ProgressView(value: min(max(progress, 0), 1))
                            .tint(BotLiveActivityWidget.statusColor(context.state.status))
                    }
                } else {
                    Text(BotLiveActivityWidget.statusLabel(context.state.status))
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)

                    let subtitle = subtitleText
                    Text(subtitle)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }

            Spacer(minLength: 0)
        }
        .padding(16)
    }

    /// Focus bot: its step (or status) and "+N others".
    private var focusSubtitle: String {
        var parts: [String] = []
        if let step = context.state.step, !step.isEmpty {
            parts.append(step)
        } else {
            parts.append(BotLiveActivityWidget.statusLabel(context.state.status))
        }
        if let others = BotLiveActivityWidget.othersText(context.state) { parts.append(others) }
        return parts.joined(separator: " · ")
    }

    private var subtitleText: String {
        var parts: [String] = []
        if context.state.activeBotsCount == 1 {
            parts.append("1 bot active")
        } else {
            parts.append("\(context.state.activeBotsCount) bots active")
        }
        if context.state.pendingApprovalsCount > 0 {
            parts.append("\(context.state.pendingApprovalsCount) pending approval")
        }
        if let label = context.state.activityLabel, !label.isEmpty {
            parts.append(label)
        }
        return parts.joined(separator: " · ")
    }
}

/// The focus bot needs the owner. Approve / Deny run in the app process as
/// `LiveActivityIntent`s (they ask for unlock first); without an inline
/// question the generic line and Open (`allternit://inbox/<id>`) show.
private struct QuestionView: View {
    let state: BotActivityAttributes.ContentState
    let onDark: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(state.question ?? "")
                .font(.caption)
                .foregroundColor(onDark ? .white.opacity(0.9) : Color("TextPrimary"))
                .lineLimit(3)
            HStack(spacing: 8) {
                if state.showsInlineDecision, let inboxId = state.inboxId {
                    Button(intent: ApproveFromLiveActivityIntent(
                        inboxId: inboxId, runtimeId: state.runtimeId, contentHash: state.contentHash
                    )) {
                        Text("Approve").font(.caption.weight(.semibold))
                    }
                    .tint(.green)
                    Button(intent: DenyFromLiveActivityIntent(
                        inboxId: inboxId, runtimeId: state.runtimeId, contentHash: state.contentHash
                    )) {
                        Text("Deny").font(.caption.weight(.semibold))
                    }
                    .tint(.red)
                } else if let url = state.inboxURL {
                    Link(destination: url) {
                        Text("Open").font(.caption.weight(.semibold))
                    }
                }
            }
            .buttonStyle(.bordered)
        }
    }
}

extension WidgetConfiguration {
    /// Apple Watch Smart Stack / CarPlay (`.small`) on iOS 18 and later.
    /// SE-0360 lets the two branches return different opaque types.
    func smallActivityFamilyIfAvailable() -> some WidgetConfiguration {
        if #available(iOS 18.0, *) {
            return self.supplementalActivityFamilies([.small])
        } else {
            return self
        }
    }
}
