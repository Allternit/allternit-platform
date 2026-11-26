import ActivityKit
import WidgetKit
import SwiftUI

/// Lock Screen banner + Dynamic Island for one bar job, driven by
/// `BotLiveActivityManager.syncBarJob`. Same layout and tokens as
/// `BotLiveActivityWidget`: a tinted status circle (15 percent), a mono title,
/// a caption line, and for the owner the question with Approve / Deny
/// (reusing `ApproveFromLiveActivityIntent` / `DenyFromLiveActivityIntent`)
/// or Open when the question cannot be answered inline.
struct TaskLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: TaskActivityAttributes.self) { context in
            TaskLockScreenView(context: context)
                .activityBackgroundTint(Color("BgPanel"))
                .activitySystemActionForegroundColor(Color("TextPrimary"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    TaskStatusIcon(state: context.state.state, size: 34)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(context.state.amount ?? context.state.shortState)
                        .font(.caption)
                        .foregroundColor(.white)
                        .lineLimit(1)
                }
                DynamicIslandExpandedRegion(.center) {
                    Text(context.attributes.title)
                        .font(.system(.subheadline, design: .monospaced))
                        .foregroundColor(.white)
                        .lineLimit(1)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 6) {
                        if context.state.showsInlineDecision || (context.state.isOwner && context.state.question?.isEmpty == false) {
                            TaskQuestionView(state: context.state, onDark: true)
                        } else {
                            Text(taskLine(context))
                                .font(.caption)
                                .foregroundColor(.white.opacity(0.8))
                                .lineLimit(1)
                        }
                        TaskStepLine(reached: context.state.stepsReached,
                                     tint: TaskStatusIcon.color(for: context.state.state), onDark: true)
                    }
                }
            } compactLeading: {
                TaskStatusIcon(state: context.state.state, size: 18)
            } compactTrailing: {
                Text(context.state.amount ?? context.state.shortState)
                    .font(.caption2)
                    .foregroundColor(.white)
                    .lineLimit(1)
                    .frame(maxWidth: 56)
            } minimal: {
                TaskStatusIcon(state: context.state.state, size: 16)
            }
        }
        .smallActivityFamilyIfAvailable()
    }

    /// "Stripe · Waiting on Noah"
    private func taskLine(_ context: ActivityViewContext<TaskActivityAttributes>) -> String {
        [context.attributes.appName, context.state.stateLine].filter { !$0.isEmpty }.joined(separator: " · ")
    }
}

/// Tinted circle with a state symbol (15 percent tint, as `BotLiveActivityWidget.statusIcon`).
private struct TaskStatusIcon: View {
    let state: String
    let size: CGFloat

    var body: some View {
        let color = Self.color(for: state)
        Image(systemName: Self.symbol(for: state))
            .font(.system(size: size * 0.55, weight: .semibold))
            .foregroundColor(color)
            .frame(width: size, height: size)
            .background(color.opacity(0.15))
            .clipShape(Circle())
    }

    static func color(for state: String) -> Color {
        switch state {
        case "working": return .blue
        case "completed": return .green
        case "waiting_approval", "input_required", "review": return .orange
        case "failed": return .red
        default: return .gray
        }
    }

    static func symbol(for state: String) -> String {
        switch state {
        case "working": return "cpu"
        case "waiting_approval": return "hand.raised"
        case "input_required": return "text.bubble"
        case "review": return "eye"
        case "completed": return "checkmark.circle"
        case "failed": return "xmark.octagon"
        case "cancelled": return "slash.circle"
        default: return "cpu"
        }
    }
}

private struct TaskLockScreenView: View {
    let context: ActivityViewContext<TaskActivityAttributes>

    var body: some View {
        HStack(spacing: 12) {
            TaskStatusIcon(state: context.state.state, size: 40)

            VStack(alignment: .leading, spacing: 2) {
                Text(titleLine)
                    .font(.system(.subheadline, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                if context.state.isOwner, context.state.question?.isEmpty == false {
                    TaskQuestionView(state: context.state, onDark: false)
                } else {
                    Text(context.state.stateLine)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                }
                TaskStepLine(reached: context.state.stepsReached,
                             tint: TaskStatusIcon.color(for: context.state.state), onDark: false)
                    .padding(.top, 4)
            }

            Spacer(minLength: 0)
        }
        .padding(16)
    }

    /// "Refund payment · Stripe", or the bot's name first when known.
    private var titleLine: String {
        var parts = [context.attributes.title]
        if !context.attributes.appName.isEmpty { parts.append(context.attributes.appName) }
        return parts.joined(separator: " · ")
    }
}

/// Owner only. Approve / Deny exist only with an inline question (the same
/// rule as `BotLiveActivityWidget`'s question view); otherwise Open.
private struct TaskQuestionView: View {
    let state: TaskActivityAttributes.ContentState
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

/// Four steps (Asked, Approval, Running, Done): reached steps fill, the rest stay faint.
private struct TaskStepLine: View {
    let reached: Int
    let tint: Color
    let onDark: Bool

    var body: some View {
        VStack(spacing: 2) {
            HStack(spacing: 3) {
                ForEach(0..<TaskActivityAttributes.stepLabels.count, id: \.self) { index in
                    Capsule()
                        .fill(index < reached ? tint : tint.opacity(0.25))
                        .frame(height: 3)
                }
            }
            HStack {
                Text(TaskActivityAttributes.stepLabels.first ?? "")
                Spacer(minLength: 4)
                Text(TaskActivityAttributes.stepLabels.last ?? "")
            }
            .font(.system(size: 10))
            .foregroundColor(onDark ? .white.opacity(0.6) : Color("TextSecondary"))
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Step \(min(max(reached, 0), 4)) of 4")
    }
}
