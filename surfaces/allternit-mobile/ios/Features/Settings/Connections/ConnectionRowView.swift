import SwiftUI

/// The vendor's logo on a white tile, or an SF Symbol on a neutral tile
/// (web connect-kit `ConnLogo`).
struct ConnLogoView: View {
    let logo: ConnLogo
    let name: String
    var size: CGFloat = 36

    var body: some View {
        let radius = max(6, (size / 4).rounded())
        Group {
            switch logo {
            case .asset(let asset):
                Image(asset)
                    .resizable()
                    .scaledToFit()
                    .frame(width: size * 0.6, height: size * 0.6)
                    .frame(width: size, height: size)
                    .background(Color.white)
                    .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous).stroke(Color.black.opacity(0.06), lineWidth: 1))
            case .symbol(let symbol):
                Image(systemName: symbol)
                    .font(.system(size: size * 0.45, weight: .regular))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: size, height: size)
                    .background(Color("TextPrimary").opacity(0.07))
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
        .accessibilityHidden(true)
    }
}

/// The status dot before the words (web `StatusDot`).
struct ConnStatusDot: View {
    let state: ConnState
    @ReducedMotion private var reduceMotion
    @State private var dim = false

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: 7, height: 7)
            .opacity(state == .connecting && dim ? 0.35 : 1)
            .onAppear {
                guard state == .connecting, !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 0.7).repeatForever(autoreverses: true)) { dim = true }
            }
            .accessibilityHidden(true)
    }

    private var color: Color {
        switch state {
        case .on: return Theme.statusSuccess
        case .needs: return Color(hex: "#E8A33A")
        case .connecting: return Theme.statusInfo
        case .off: return .clear
        }
    }
}

/// What the row's right side does.
enum ConnRowAction {
    case connect, fix, disconnect, open, fallback(String)
}

/// The one row: logo, name, one status line, one button (web `ConnectionRow`).
/// An unavailable row shows its reason and up to two fallbacks, never a
/// Connect button that fails.
struct ConnectionRowView: View {
    let entry: ConnEntry
    @ObservedObject var store: ConnectionsStore
    /// Shown after the name ("Your brain").
    var tag: String? = nil
    /// Rows that open a settings page show a chevron instead of Connect/Disconnect.
    var opensPage = false
    /// Replaces the right side (setup: "Use this").
    var customAction: AnyView? = nil
    var compact = false
    /// A ChatGPT account role (title, badge, "Powers:" line, model count). Nil for every other row.
    var accountExtras: AccountPair.Extras? = nil
    /// Draw the role's own mark in place of the vendor logo (the grouped card).
    var accountMark = false
    let onAction: (ConnRowAction) -> Void

    var body: some View {
        let row = store.row(entry.id)
        let blocked = !row.available && row.state != .on
        HStack(alignment: .center, spacing: 12) {
            if accountMark, accountExtras != nil {
                AccountMark(id: entry.id)
            } else {
                ConnLogoView(logo: entry.logo, name: entry.name, size: compact ? 30 : 36)
                    .opacity(blocked ? 0.5 : 1)
                    .saturation(blocked ? 0 : 1)
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(accountExtras?.title ?? entry.name)
                        .font(.system(size: compact ? 15 : 16, weight: .medium))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    if entry.isNew && row.state == .off && !blocked {
                        Text("New")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(Capsule().fill(Color("TextPrimary").opacity(0.07)))
                    }
                    if let badge = accountExtras?.badge { AccountBadge(text: badge) }
                    if let tag {
                        Text("· \(tag)")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                ConnStatusLines(
                    entry: entry, store: store, row: row, blocked: blocked,
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
            if let customAction {
                customAction
            } else {
                ConnRowTrailing(entry: entry, row: row, blocked: blocked, opensPage: opensPage, compact: compact, accountExtras: accountExtras, onAction: onAction)
            }
        }
        .padding(.horizontal, compact ? 12 : 16)
        .padding(.vertical, compact ? 9 : 11)
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-row-\(entry.id)")
    }
}

/// The status under a row's or card's name: the unavailable reason, or the
/// dot and the one status line (plus the Claude organization note).
struct ConnStatusLines: View {
    let entry: ConnEntry
    @ObservedObject var store: ConnectionsStore
    let row: ConnectionWire
    let blocked: Bool
    var lineLimit = 2
    /// A role row's own status words (see `AccountPair.statusLine`).
    var statusOverride: String? = nil

    var body: some View {
        if blocked {
            Text(store.unavailableReason(entry.id))
                .font(.system(size: 13))
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(lineLimit + 1)
                .fixedSize(horizontal: false, vertical: true)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if row.state != .off { ConnStatusDot(state: row.state) }
                Text(statusOverride ?? store.statusLine(entry))
                    .font(.system(size: 13))
                    .foregroundColor(row.state == .off ? Color("TextTertiary") : Color("TextSecondary"))
                    .lineLimit(lineLimit)
            }
            if entry.id == "claude-org" && row.state == .off {
                Text(ConnectionsCatalog.claudeOrgNote)
                    .font(.system(size: 12.5))
                    .foregroundColor(Color("TextSecondary"))
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// The one action of a row or card: fallbacks when unavailable, Included,
/// Fix, open (chevron on a row, "Open" on a card), Disconnect or Connect.
/// `fill` stretches the buttons to the card's width.
struct ConnRowTrailing: View {
    let entry: ConnEntry
    let row: ConnectionWire
    let blocked: Bool
    var opensPage = false
    var compact = false
    var fill = false
    var accountExtras: AccountPair.Extras? = nil
    let onAction: (ConnRowAction) -> Void

    var body: some View {
        if blocked {
            let fallbacks = Array(row.fallbacks.prefix(compact || fill ? 1 : 2))
            VStack(alignment: .trailing, spacing: 6) {
                ForEach(fallbacks, id: \.self) { f in
                    Button(ConnectionsCatalog.fallbackLabel(f)) { onAction(.fallback(f)) }
                        .buttonStyle(ConnQuietButtonStyle(fill: fill))
                }
            }
        } else if entry.mainWay == .included && !(entry.id == "cloud-computer" && row.state != .on) {
            Text("Included")
                .font(.system(size: 13))
                .foregroundColor(Color("TextTertiary"))
        } else if entry.id == "phone" {
            EmptyView()
        } else if row.state == .connecting {
            ProgressView().controlSize(.small)
        } else if row.state == .needs {
            Button(entry.id == "claude-org" && row.reasonCode != "spend_limit_reached" ? "Link again" : "Fix") { onAction(.fix) }
                .buttonStyle(ConnPrimaryButtonStyle(fill: fill))
        } else if opensPage {
            if fill {
                Button("Open") { onAction(.open) }
                    .buttonStyle(ConnQuietButtonStyle(fill: true))
                    .accessibilityLabel("Open \(entry.name)")
            } else {
                Button { onAction(.open) } label: { PhoneRowChevron().padding(8) }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Open \(entry.name)")
            }
        } else if row.state == .on {
            Button(accountExtras?.disconnectLabel ?? (entry.id == "claude-org" ? "Unlink" : "Disconnect")) { onAction(.disconnect) }
                .buttonStyle(ConnQuietButtonStyle(fill: fill))
                .accessibilityLabel(entry.id == "claude-org" ? "Unlink your Claude organization" : "\(accountExtras?.disconnectLabel ?? "Disconnect") \(accountExtras?.title ?? entry.name)")
        } else {
            Button(accountExtras?.connectLabel ?? (entry.id == "cloud-computer" ? "See plans" : entry.id == "claude-org" ? "Link" : "Connect")) { onAction(.connect) }
                .buttonStyle(ConnQuietButtonStyle(fill: fill))
                .accessibilityLabel(entry.id == "claude-org" ? "Link your Claude organization" : "\(accountExtras?.connectLabel ?? "Connect") \(accountExtras?.title ?? entry.name)")
        }
    }
}

/// The kit's quiet pill button (`quietButton`).
struct ConnQuietButtonStyle: ButtonStyle {
    /// Full width (Grid and Deck cards).
    var fill = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13.5, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .lineLimit(1)
            .padding(.horizontal, 12)
            .frame(maxWidth: fill ? .infinity : nil, minHeight: 32)
            .background(Capsule().fill(Color("TextPrimary").opacity(configuration.isPressed ? 0.14 : 0.07)))
    }
}

/// The kit's filled pill button (`primaryButton`).
struct ConnPrimaryButtonStyle: ButtonStyle {
    /// Full width (Grid and Deck cards).
    var fill = false
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13.5, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .lineLimit(1)
            .padding(.horizontal, 14)
            .frame(maxWidth: fill ? .infinity : nil, minHeight: 32)
            .background(Capsule().fill(Color("TextPrimary").opacity(isEnabled ? (configuration.isPressed ? 0.8 : 1) : 0.4)))
    }
}
