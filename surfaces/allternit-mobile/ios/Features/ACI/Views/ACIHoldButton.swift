import SwiftUI

/// Press and hold for one second to confirm (the iOS form of Arc HoldToConfirm).
/// VoiceOver users get a plain activate action instead of the hold.
struct ACIHoldButton: View {
    let title: String
    var danger = false
    var disabled = false
    let action: () -> Void
    @State private var progress: CGFloat = 0

    var body: some View {
        ZStack {
            GeometryReader { geo in
                Rectangle()
                    .fill((danger ? Theme.statusError : Color("TextPrimary")).opacity(0.18))
                    .frame(width: geo.size.width * progress)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(danger ? Theme.statusError : Color("TextPrimary"))
        }
        .frame(maxWidth: .infinity, minHeight: 48)
        .background(Color("FillNeutral"))
        .clipShape(Capsule())
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .opacity(disabled ? 0.45 : 1)
        .allowsHitTesting(!disabled)
        .onLongPressGesture(minimumDuration: 1.0, maximumDistance: 40, perform: {
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            action()
        }, onPressingChanged: { pressing in
            if pressing { withAnimation(.linear(duration: 1.0)) { progress = 1 } }
            else { withAnimation(.easeOut(duration: 0.2)) { progress = 0 } }
        })
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityHint("Press and hold to confirm")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { if !disabled { action() } }
    }
}
