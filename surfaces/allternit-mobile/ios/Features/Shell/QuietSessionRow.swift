import SwiftUI

/// The one quiet list row (Claude polish i5): a leading glyph or avatar, a
/// one-line title, an optional status mark, and the last-used time at the
/// right of the title line ("2m", "3h", "Nov 17"). Touch has no hover, so the
/// time is always visible. Used for recents, bot and group rows, and the
/// drawer's section rows, so every list reads the same.
///
/// The status mark is passed in by the caller from real data only (a running
/// spinner, an offline glyph, a warning). Remote-control state
/// (`rc.state` connected / reconnecting / offline / off) is not read by the
/// iOS client yet, so rows show their type glyph until it is.
struct QuietSessionRow<Leading: View, Status: View>: View {
    enum Metrics {
        /// A session, bot or group row.
        case session
        /// A drawer section row (Projects, Artifacts, ...): larger, no time.
        case nav

        var height: CGFloat { self == .nav ? 50 : 44 }
        var titleSize: CGFloat { self == .nav ? 17 : 16.5 }
        var gap: CGFloat { self == .nav ? 14 : 12 }
    }

    let title: String
    var time: String = ""
    var isSelected: Bool = false
    /// Unread / needs-attention rows read a little heavier.
    var emphasized: Bool = false
    var metrics: Metrics = .session
    @ViewBuilder var leading: () -> Leading
    @ViewBuilder var status: () -> Status

    var body: some View {
        HStack(spacing: metrics.gap) {
            leading()
            Text(title)
                .font(.system(size: metrics.titleSize, weight: emphasized ? .semibold : .regular))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            status()
            if !time.isEmpty {
                Text(time)
                    .font(.system(size: 12))
                    .monospacedDigit()
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
        }
        .padding(.horizontal, 12)
        .frame(minHeight: metrics.height)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(isSelected ? Color("BorderStrong").opacity(0.65) : Color.clear)
        )
        .contentShape(Rectangle())
    }
}

extension QuietSessionRow where Leading == QuietSessionGlyph {
    /// Row with a type glyph (SF Symbol) as its leading mark.
    init(title: String, glyph: String, time: String = "", isSelected: Bool = false,
         emphasized: Bool = false, metrics: Metrics = .session,
         @ViewBuilder status: @escaping () -> Status) {
        self.init(title: title, time: time, isSelected: isSelected, emphasized: emphasized,
                  metrics: metrics, leading: { QuietSessionGlyph(name: glyph, metrics: metrics) },
                  status: status)
    }
}

/// The type glyph at the row's left.
struct QuietSessionGlyph: View {
    let name: String
    var metrics: QuietSessionRow<EmptyView, EmptyView>.Metrics = .session

    var body: some View {
        Image(systemName: name)
            .font(.system(size: metrics == .nav ? 19 : 16))
            .foregroundColor(metrics == .nav ? Color("TextPrimary") : Color("TextSecondary"))
            .frame(width: metrics == .nav ? 24 : 20)
    }
}
