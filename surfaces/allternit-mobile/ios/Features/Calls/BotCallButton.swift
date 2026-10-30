import SwiftUI

/// "Call" for a bot: rings someone who joined through this bot's invite,
/// over the free in-app voice path (the web's Contacts app Call button —
/// `ringContact` + `startOutgoingCall`). Shown only when the bot has at
/// least one such person (`userId` + `reach.app`, the web's `inApp` rule);
/// the parent loads them with `InappCallStore.loadCallableContacts`.
/// A tap opens the list of people to call; the call screen takes over from
/// there ("Calling…" with Cancel).
struct BotCallButton: View {
    enum Style { case toolbar, capsule, round }

    let botId: String
    var style: Style = .toolbar

    @ObservedObject private var store = InappCallStore.shared
    @State private var choosing = false

    private var contacts: [InappCallsClient.Contact] { store.callableContacts[botId] ?? [] }

    var body: some View {
        if !contacts.isEmpty {
            Button {
                choosing = true
            } label: {
                switch style {
                case .toolbar:
                    Image(systemName: "phone")
                case .round:
                    Image(systemName: "phone")
                        .font(.system(size: 15, weight: .semibold))
                        .modifier(ThreadRoundButton())
                case .capsule:
                    Label("Call", systemImage: "phone")
                        .font(.subheadline.weight(.medium))
                        .padding(.horizontal, 14)
                        .frame(minHeight: 36)
                        .background(Color("BgSecondary"))
                        .clipShape(Capsule())
                }
            }
            .disabled(store.active != nil)
            .accessibilityLabel("Call")
            .accessibilityHint("Free Allternit call with someone who joined through this bot's invite")
            .confirmationDialog("Call on Allternit", isPresented: $choosing, titleVisibility: .visible) {
                ForEach(contacts) { contact in
                    Button(contact.label) {
                        Task { await store.call(contact, botId: botId) }
                    }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Free calls with people who joined through this bot's invite.")
            }
        }
    }
}
