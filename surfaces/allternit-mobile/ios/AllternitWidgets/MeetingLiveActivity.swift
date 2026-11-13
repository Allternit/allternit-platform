import ActivityKit
import WidgetKit
import SwiftUI

/// Lock Screen banner and Dynamic Island for a meeting Gizzi is in, driven by
/// `MeetingLiveActivityManager` in the app. Shows the state chip, the elapsed
/// time once live, and "Gizzi is listening". Tapping opens the live page
/// (`allternit://meetings/<id>`).
struct MeetingLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: MeetingActivityAttributes.self) { context in
            MeetingLockScreenView(context: context)
                .activityBackgroundTint(Color("BgPanel"))
                .activitySystemActionForegroundColor(Color("TextPrimary"))
                .widgetURL(Self.url(context))
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    MeetingMascot(size: 36)
                }
                DynamicIslandExpandedRegion(.trailing) {
                    MeetingElapsed(state: context.state)
                        .font(.system(.callout, design: .rounded).monospacedDigit())
                        .foregroundColor(.white)
                }
                DynamicIslandExpandedRegion(.center) {
                    Text(context.attributes.title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(.white)
                        .lineLimit(1)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    HStack(spacing: 8) {
                        MeetingStateChip(state: context.state, onDark: true)
                        Text(context.state.detail)
                            .font(.caption)
                            .foregroundColor(.white.opacity(0.75))
                            .lineLimit(1)
                        Spacer(minLength: 0)
                    }
                }
            } compactLeading: {
                MeetingMascot(size: 20)
            } compactTrailing: {
                MeetingElapsed(state: context.state)
                    .font(.caption2.monospacedDigit())
                    .foregroundColor(MeetingStateChip.tint(context.state.gizzi))
                    .frame(maxWidth: 52)
            } minimal: {
                MeetingMascot(size: 18)
            }
            .widgetURL(Self.url(context))
        }
        .smallActivityFamilyIfAvailable()
    }

    static func url(_ context: ActivityViewContext<MeetingActivityAttributes>) -> URL? {
        URL(string: "allternit://meetings/\(context.attributes.meetingId)")
    }
}

private struct MeetingMascot: View {
    let size: CGFloat

    var body: some View {
        Image("GizziMascot")
            .resizable()
            .interpolation(.high)
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityLabel("Gizzi")
    }
}

/// Elapsed time once live; the state word before that.
private struct MeetingElapsed: View {
    let state: MeetingActivityAttributes.ContentState

    var body: some View {
        if let since = state.liveSinceDate, state.gizzi == "live" || state.gizzi == "paused" {
            Text(since, style: .timer)
                .multilineTextAlignment(.trailing)
        } else {
            Text(state.gizzi == "waiting_admit" ? "Lobby" : state.stateLabel)
        }
    }
}

private struct MeetingStateChip: View {
    let state: MeetingActivityAttributes.ContentState
    var onDark = false

    static func tint(_ gizzi: String) -> Color {
        switch gizzi {
        case "live": return Color(red: 0.13, green: 0.77, blue: 0.37)
        case "waiting_admit", "launching", "joining": return Color(red: 0.98, green: 0.75, blue: 0.14)
        case "not_admitted": return Color(red: 0.97, green: 0.44, blue: 0.44)
        default: return .gray
        }
    }

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(Self.tint(state.gizzi)).frame(width: 6, height: 6)
            Text(state.stateLabel)
                .font(.caption.weight(.semibold))
        }
        .foregroundColor(onDark ? .white : Color("TextPrimary"))
        .padding(.horizontal, 8)
        .padding(.vertical, 3)
        .background(Capsule().fill(Self.tint(state.gizzi).opacity(onDark ? 0.28 : 0.16)))
    }
}

private struct MeetingLockScreenView: View {
    let context: ActivityViewContext<MeetingActivityAttributes>

    var body: some View {
        HStack(spacing: 12) {
            MeetingMascot(size: 44)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(context.attributes.title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    MeetingElapsed(state: context.state)
                        .font(.subheadline.monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: 70, alignment: .trailing)
                }
                HStack(spacing: 6) {
                    MeetingStateChip(state: context.state)
                    Text(context.state.gizzi == "waiting_admit" && context.attributes.platformLabel == "Zoom"
                         ? "Join the Zoom call so Gizzi can come in"
                         : "\(context.state.detail) · \(context.attributes.platformLabel)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
        }
        .padding(16)
    }
}
