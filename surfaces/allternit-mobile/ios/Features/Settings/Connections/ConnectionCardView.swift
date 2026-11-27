import SwiftUI

/// One connection as a card for the Grid and Deck layouts. It reuses the row's
/// logo, status lines and action, so connect, disconnect, fallback and fix
/// behave exactly as on a row (one set of logic, no fork).
struct ConnectionCardView: View {
    let entry: ConnEntry
    @ObservedObject var store: ConnectionsStore
    var opensPage = false
    /// A ChatGPT account role (title, badge, "Powers:" line, model count). Nil for every other card; the same
    /// words the List rows use, so the two read the same in every layout.
    var accountExtras: AccountPair.Extras? = nil
    let onAction: (ConnRowAction) -> Void

    var body: some View {
        let row = store.row(entry.id)
        let blocked = !row.available && row.state != .on
        VStack(alignment: .leading, spacing: 10) {
            if accountExtras != nil {
                AccountMark(id: entry.id)
            } else {
                ConnLogoView(logo: entry.logo, name: entry.name, size: 40)
                    .opacity(blocked ? 0.5 : 1)
                    .saturation(blocked ? 0 : 1)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text(accountExtras?.title ?? entry.name)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(accountExtras == nil ? 1 : 2)
                if let badge = accountExtras?.badge { AccountBadge(text: badge) }
            }
            VStack(alignment: .leading, spacing: 3) {
                ConnStatusLines(
                    entry: entry, store: store, row: row, blocked: blocked, lineLimit: 2,
                    statusOverride: accountExtras.map { AccountPair.statusLine($0, state: row.state, fallback: store.statusLine(entry)) }
                )
                if let powers = accountExtras?.powers, !blocked {
                    Text(powers)
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                }
                if entry.group == .apps && row.state == .on { ToolPermissionsButton(entry: entry) }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
            ConnRowTrailing(entry: entry, row: row, blocked: blocked, opensPage: opensPage, compact: true, fill: true, accountExtras: accountExtras, onAction: onAction)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 168, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-card-\(entry.id)")
    }
}
