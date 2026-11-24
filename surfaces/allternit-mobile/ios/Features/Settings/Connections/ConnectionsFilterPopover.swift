import SwiftUI

/// The status filter of Settings › Connections (the sliders popover).
enum ConnStatusFilter: String, CaseIterable, Identifiable {
    case all, connected, needs, off
    var id: String { rawValue }

    var label: String {
        switch self {
        case .all: return "All"
        case .connected: return "Connected"
        case .needs: return "Needs you"
        case .off: return "Not connected"
        }
    }

    /// Does a row in this state pass the filter?
    func matches(_ state: ConnState) -> Bool {
        switch self {
        case .all: return true
        case .connected: return state == .on
        case .needs: return state == .needs
        case .off: return state == .off
        }
    }
}

/// How many listed connections sit in each status.
struct ConnStatusCounts {
    var all = 0
    var connected = 0
    var needs = 0
    var off = 0

    init(states: [ConnState]) {
        all = states.count
        connected = states.filter { $0 == .on }.count
        needs = states.filter { $0 == .needs }.count
        off = states.filter { $0 == .off }.count
    }

    subscript(filter: ConnStatusFilter) -> Int {
        switch filter {
        case .all: return all
        case .connected: return connected
        case .needs: return needs
        case .off: return off
        }
    }
}

/// The sliders popover: the layout switch on top, Status with counts, then
/// "Find more connections". Modelled on `CodeSessionsFilterPopover`.
struct ConnectionsFilterPopover: View {
    @Binding var layoutRaw: String
    @Binding var status: ConnStatusFilter
    let counts: ConnStatusCounts
    let onFindMore: () -> Void

    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                OverviewLayoutSwitch(raw: $layoutRaw, fallback: .list)
                    .padding(.bottom, 6)
                label("Status")
                ForEach(ConnStatusFilter.allCases) { option in
                    check(option.label, count: counts[option], on: status == option) {
                        Haptics.selection()
                        status = option
                    }
                }
                label("Show")
                check("Find more connections", count: nil, on: false) {
                    dismiss()
                    onFindMore()
                }
            }
            .padding(16)
        }
        .frame(minWidth: 260, maxHeight: 460)
    }

    private func label(_ text: String) -> some View {
        Text(text).font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary")).padding(.top, 6)
    }

    private func check(_ title: String, count: Int?, on: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack {
                Text(title).font(.system(size: 15))
                Spacer(minLength: 12)
                if let count {
                    Text("\(count)").font(.system(size: 13)).monospacedDigit().foregroundColor(Color("TextSecondary"))
                }
                if on { Image(systemName: "checkmark").font(.system(size: 13, weight: .semibold)) }
            }
            .foregroundColor(Color("TextPrimary"))
            .frame(minHeight: 40)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
