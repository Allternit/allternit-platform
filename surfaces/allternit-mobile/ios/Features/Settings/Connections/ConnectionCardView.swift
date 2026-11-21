import SwiftUI

/// One connection as a card for the Grid and Deck layouts. It reuses the row's
/// logo, status lines and action, so connect, disconnect, fallback and fix
/// behave exactly as on a row (one set of logic, no fork).
struct ConnectionCardView: View {
    let entry: ConnEntry
    @ObservedObject var store: ConnectionsStore
    var opensPage = false
    let onAction: (ConnRowAction) -> Void

    var body: some View {
        let row = store.row(entry.id)
        let blocked = !row.available && row.state != .on
        VStack(alignment: .leading, spacing: 10) {
            ConnLogoView(logo: entry.logo, name: entry.name, size: 40)
                .opacity(blocked ? 0.5 : 1)
                .saturation(blocked ? 0 : 1)
            Text(entry.name)
                .font(.system(size: 16, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            VStack(alignment: .leading, spacing: 3) {
                ConnStatusLines(entry: entry, store: store, row: row, blocked: blocked, lineLimit: 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
            ConnRowTrailing(entry: entry, row: row, blocked: blocked, opensPage: opensPage, compact: true, fill: true, onAction: onAction)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(14)
        .frame(maxWidth: .infinity, minHeight: 168, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-card-\(entry.id)")
    }
}
