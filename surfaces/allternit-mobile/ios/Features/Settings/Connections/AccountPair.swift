import SwiftUI

/// The ChatGPT account shown as two separate connections (contract §9.9, D5;
/// design pick: Connections, option B grouped). "ChatGPT subscription" is the
/// device sign-in (chat models and pictures); "Codex CLI" is its own sign-in
/// (coding runs). They can use the same ChatGPT account, but each one signs in
/// and reports on its own. Mirrors allternit-ai `accountPair.tsx`.
enum AccountPair {
    static let chatgptId = "chatgpt"
    static let codexId = "codex"
    static let pairTitle = "ChatGPT account · two separate connections"
    static let pairNote = "They can use the same ChatGPT account, but each one signs in and reports on its own."

    /// Both rows are in the list, so they group. One alone (or neither) does not.
    static func pairsUp(_ ids: [String]) -> Bool {
        ids.contains(chatgptId) && ids.contains(codexId)
    }

    /// What a row says about itself once it carries a role.
    struct Extras: Equatable {
        var title: String
        var badge: String
        var powers: String
        var connectLabel: String?
        var disconnectLabel: String?
        var statusOn: String
        var statusOff: String
        var modelCount: Int?
    }

    /// The extras for `id`; nil for any other row. `paired` shortens nothing but
    /// adds the "signing out of one does not sign out the other" line to Codex.
    static func extras(id: String, row: ConnectionWire, paired: Bool) -> Extras? {
        switch id {
        case chatgptId:
            let count = row.models.count
            return Extras(
                title: "ChatGPT subscription",
                badge: "Device sign-in",
                powers: "Powers: chat models and pictures. Signed in once on this iPhone.",
                statusOn: "Signed in on this iPhone",
                statusOff: "Not signed in on this iPhone",
                modelCount: count > 0 ? count : nil
            )
        case codexId:
            return Extras(
                title: "Codex CLI",
                badge: "Own sign-in",
                powers: paired ? "Powers: coding runs. Signing out of one does not sign out the other." : "Powers: coding runs.",
                connectLabel: "Set up",
                disconnectLabel: "Sign out",
                statusOn: row.found.map { "Signed in · \($0)" } ?? "Signed in",
                statusOff: "Not set up",
                modelCount: nil
            )
        default:
            return nil
        }
    }

    /// The one status line for a role row. Needs-you and connecting keep the
    /// store's own wording (`fallback`), which carries the real reason.
    static func statusLine(_ extras: Extras, state: ConnState, fallback: String) -> String {
        switch state {
        case .on: return extras.statusOn + (extras.modelCount.map { " · \($0) model\($0 == 1 ? "" : "s")" } ?? "")
        case .off: return extras.statusOff
        case .needs, .connecting: return fallback
        }
    }
}

/// The one card for both rows, with the heading above and the note under.
struct AccountPairCard<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(AccountPair.pairTitle.uppercased())
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.6)
                .foregroundColor(Color("TextTertiary"))
                .padding(.horizontal, 16)
                .padding(.vertical, 6)
                .background(Color("TextPrimary").opacity(0.03))
                .accessibilityAddTraits(.isHeader)
            content
            PhoneCardDivider()
            Text(AccountPair.pairNote)
                .font(.system(size: 12))
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(AccountPair.pairTitle)
        .accessibilityIdentifier("conn-account-pair")
    }
}

/// Round chat bubble (subscription) vs outlined terminal (CLI): two shapes,
/// not only two words, so the rows read as different things.
struct AccountMark: View {
    let id: String

    var body: some View {
        Group {
            if id == AccountPair.codexId {
                Image(systemName: "terminal")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(width: 36, height: 36)
                    .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).stroke(Color("TextPrimary").opacity(0.35), lineWidth: 1.5))
            } else {
                Image(systemName: "bubble.left.fill")
                    .font(.system(size: 15))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 36, height: 36)
                    .background(Circle().fill(Color("TextPrimary").opacity(0.09)))
            }
        }
        .accessibilityHidden(true)
    }
}

/// The small outlined tag after a role row's name.
struct AccountBadge: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 7)
            .padding(.vertical, 1.5)
            .overlay(Capsule().stroke(Color("TextPrimary").opacity(0.18), lineWidth: 1))
    }
}
