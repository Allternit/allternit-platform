import ActivityKit
import WidgetKit
import SwiftUI

/// Lock Screen banner + Dynamic Island presentation for a running `Loop`,
/// driven by `LoopLiveActivityManager` in the main app target. Mirrors
/// `LoopStaminaRing`'s progress-ring look (can't literally reuse that SwiftUI
/// view — it lives in the app target, not this extension) with the same
/// color convention as `LoopsListView.statusColor(_:)`.
struct LoopLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: LoopActivityAttributes.self) { context in
            LockScreenView(context: context)
                .activityBackgroundTint(Color("BgPanel"))
                .activitySystemActionForegroundColor(Color("TextPrimary"))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    ringView(context: context, diameter: 34)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(Self.iterationText(context: context))
                        .font(.caption)
                        .foregroundColor(.white)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    Text(context.attributes.command)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(.white.opacity(0.8))
                        .lineLimit(1)
                }
            } compactLeading: {
                ringView(context: context, diameter: 18)
            } compactTrailing: {
                Text(Self.iterationText(context: context))
                    .font(.caption2)
                    .foregroundColor(.white)
            } minimal: {
                ringView(context: context, diameter: 16)
            }
        }
        .smallActivityFamilyIfAvailable()
    }

    private static func iterationText(context: ActivityViewContext<LoopActivityAttributes>) -> String {
        "\(context.state.iterationsCompleted)/\(context.state.maxIterations)"
    }

    fileprivate static func statusColor(_ state: String) -> Color {
        switch state {
        case "running": return .blue
        case "succeeded": return .green
        case "max_iterations": return .orange
        default: return .gray
        }
    }

    @ViewBuilder
    private func ringView(context: ActivityViewContext<LoopActivityAttributes>, diameter: CGFloat) -> some View {
        let progress = context.state.maxIterations > 0
            ? min(1, Double(context.state.iterationsCompleted) / Double(context.state.maxIterations))
            : 0
        let color = Self.statusColor(context.state.state)
        ZStack {
            Circle().stroke(color.opacity(0.25), lineWidth: 2.5)
            Circle()
                .trim(from: 0, to: progress)
                .stroke(color, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
                .rotationEffect(.degrees(-90))
        }
        .frame(width: diameter, height: diameter)
    }
}

private struct LockScreenView: View {
    let context: ActivityViewContext<LoopActivityAttributes>

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                Circle().stroke(LoopLiveActivityWidget.statusColor(context.state.state).opacity(0.2), lineWidth: 4)
                Circle()
                    .trim(from: 0, to: progress)
                    .stroke(LoopLiveActivityWidget.statusColor(context.state.state), style: StrokeStyle(lineWidth: 4, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            .frame(width: 40, height: 40)

            VStack(alignment: .leading, spacing: 2) {
                Text(context.attributes.command)
                    .font(.system(.subheadline, design: .monospaced))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text("\(context.state.iterationsCompleted) of \(context.state.maxIterations) iterations · \(context.state.state.capitalized)")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            Spacer(minLength: 0)
        }
        .padding(16)
    }

    private var progress: Double {
        context.state.maxIterations > 0
            ? min(1, Double(context.state.iterationsCompleted) / Double(context.state.maxIterations))
            : 0
    }
}
