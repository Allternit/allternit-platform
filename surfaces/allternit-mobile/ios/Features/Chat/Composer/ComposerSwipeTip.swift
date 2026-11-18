import SwiftUI

/// The one-time card above the composer that says the deck is a swipe away
/// (web `PhoneNudges` swipe card). Shown once per install, dismissable.
struct ComposerSwipeTip: View {
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "chevron.up")
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))
            Text(ComposerSwipeTipState.text)
                .font(.system(size: 13))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
            Spacer(minLength: 4)
            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 44, height: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss tip")
        }
        .padding(.leading, 12)
        .padding(.trailing, 0)
        .frame(minHeight: 44)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .padding(.bottom, 6)
        .accessibilityElement(children: .contain)
    }
}
