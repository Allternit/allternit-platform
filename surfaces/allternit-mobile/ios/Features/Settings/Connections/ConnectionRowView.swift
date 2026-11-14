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
private struct ConnStatusDot: View {
    let state: ConnState
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
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
    let onAction: (ConnRowAction) -> Void

    var body: some View {
        let row = store.row(entry.id)
        let blocked = !row.available && row.state != .on
        HStack(alignment: .center, spacing: 12) {
            ConnLogoView(logo: entry.logo, name: entry.name, size: compact ? 30 : 36)
                .opacity(blocked ? 0.5 : 1)
                .saturation(blocked ? 0 : 1)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(entry.name)
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
                    if let tag {
                        Text("· \(tag)")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                }
                if blocked {
                    Text(store.unavailableReason(entry.id))
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    HStack(spacing: 6) {
                        if row.state != .off { ConnStatusDot(state: row.state) }
                        Text(store.statusLine(entry))
                            .font(.system(size: 13))
                            .foregroundColor(row.state == .off ? Color("TextTertiary") : Color("TextSecondary"))
                            .lineLimit(2)
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            trailing(row: row, blocked: blocked)
        }
        .padding(.horizontal, compact ? 12 : 16)
        .padding(.vertical, compact ? 9 : 11)
        .contentShape(Rectangle())
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("conn-row-\(entry.id)")
    }

    @ViewBuilder
    private func trailing(row: ConnectionWire, blocked: Bool) -> some View {
        if let customAction {
            customAction
        } else if blocked {
            let fallbacks = Array(row.fallbacks.prefix(compact ? 1 : 2))
            VStack(alignment: .trailing, spacing: 6) {
                ForEach(fallbacks, id: \.self) { f in
                    Button(ConnectionsCatalog.fallbackLabel(f)) { onAction(.fallback(f)) }
                        .buttonStyle(ConnQuietButtonStyle())
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
            Button("Fix") { onAction(.fix) }
                .buttonStyle(ConnPrimaryButtonStyle())
        } else if opensPage {
            Button { onAction(.open) } label: { PhoneRowChevron().padding(8) }
                .buttonStyle(.plain)
                .accessibilityLabel("Open \(entry.name)")
        } else if row.state == .on {
            Button("Disconnect") { onAction(.disconnect) }
                .buttonStyle(ConnQuietButtonStyle())
                .accessibilityLabel("Disconnect \(entry.name)")
        } else {
            Button(entry.id == "cloud-computer" ? "See plans" : "Connect") { onAction(.connect) }
                .buttonStyle(ConnQuietButtonStyle())
                .accessibilityLabel("Connect \(entry.name)")
        }
    }
}

/// The kit's quiet pill button (`quietButton`).
struct ConnQuietButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13.5, weight: .medium))
            .foregroundColor(Color("TextPrimary"))
            .lineLimit(1)
            .padding(.horizontal, 12)
            .frame(minHeight: 32)
            .background(Capsule().fill(Color("TextPrimary").opacity(configuration.isPressed ? 0.14 : 0.07)))
    }
}

/// The kit's filled pill button (`primaryButton`).
struct ConnPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 13.5, weight: .semibold))
            .foregroundColor(Color("BgPrimary"))
            .lineLimit(1)
            .padding(.horizontal, 14)
            .frame(minHeight: 32)
            .background(Capsule().fill(Color("TextPrimary").opacity(isEnabled ? (configuration.isPressed ? 0.8 : 1) : 0.4)))
    }
}
