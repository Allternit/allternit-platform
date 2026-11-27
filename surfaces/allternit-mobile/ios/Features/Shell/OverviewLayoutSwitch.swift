import SwiftUI
import UIKit

/// List · Grid · Deck (SessionOverview OVERVIEW_LAYOUT_OPTIONS), saved per
/// screen like the web's `useOverviewLayout(scope)`. Every overview uses it:
/// the Bots home roster, bot Threads and Tasks, Projects, Artifacts and the
/// mode session overviews.
enum OverviewLayout: String, CaseIterable, Identifiable {
    case list, grid, deck
    var id: String { rawValue }

    /// Reads a stored value; older builds saved "List" / "Grid" / "Deck".
    init?(stored: String) {
        self.init(rawValue: stored.lowercased())
    }

    var label: String {
        switch self {
        case .list: return "List"
        case .grid: return "Grid"
        case .deck: return "Deck"
        }
    }

    /// A face on a row or card grows with the layout: small on a list row,
    /// bigger on a grid card, biggest on a tall deck card.
    var faceSize: CGFloat {
        switch self {
        case .list: return 32
        case .grid: return 36
        case .deck: return 56
        }
    }

    /// Phosphor List / SquaresFour / Columns.
    var icon: String {
        switch self {
        case .list: return "list.bullet"
        case .grid: return "square.grid.2x2"
        case .deck: return "rectangle.split.2x1"
        }
    }
}

/// The one layout control on every overview: a single icon button that shows
/// the NEXT view (list -> grid -> deck -> list) and cycles on tap, like the
/// web's overview switch. `options` is the set of layouts the screen supports
/// (two or three); the order is the cycle order.
struct OverviewLayoutSwitch: View {
    @Binding var value: OverviewLayout
    var options: [OverviewLayout] = OverviewLayout.allCases

    private var next: OverviewLayout {
        guard let i = options.firstIndex(of: value) else { return options.first ?? value }
        return options[(i + 1) % options.count]
    }

    var body: some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            withAnimation(.easeOut(duration: 0.16)) { value = next }
        } label: {
            Image(systemName: next.icon)
                .font(.system(size: 15, weight: .regular))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 44, height: 44)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Show as \(next.label.lowercased())")
        .accessibilityValue(value.label)
    }
}

extension OverviewLayoutSwitch {
    /// Binds straight to a screen's stored @AppStorage string.
    init(raw: Binding<String>, fallback: OverviewLayout, options: [OverviewLayout] = OverviewLayout.allCases) {
        self.init(value: Binding(
            get: { OverviewLayout(stored: raw.wrappedValue) ?? fallback },
            set: { raw.wrappedValue = $0.rawValue }
        ), options: options)
    }
}

// MARK: - Cards (SessionOverview OverviewCard)

/// One card on an overview whose rows aren't app sessions (a bot's Threads
/// and Tasks): what SessionOverview's OverviewItem carries.
struct OverviewCardItem: Identifiable {
    enum Tone { case working, waiting, suggested, done, idle }

    let id: String
    let title: String
    /// Project, repo or bot it belongs to (top-left of a card).
    var group: String? = nil
    var status: String? = nil
    var tone: Tone = .idle
    var preview: String? = nil
    /// "5m", "2h": top-right of a card, end of a row.
    var meta: String? = nil
    /// Bottom line of a card ("12 messages", "2/5").
    var footer: String? = nil
}

/// Deck cards are tall portrait cards in a sideways row, as on the web
/// (SessionOverview SIZE.deck.phone: 226 wide, the full height).
enum OverviewDeck {
    static let width: CGFloat = 226
    static var height: CGFloat { min(540, max(360, UIScreen.main.bounds.height * 0.56)) }
}

/// The same three layouts every overview has: one-line rows, a two-column
/// grid of cards, or a sideways deck of tall cards.
struct OverviewCards<Leading: View>: View {
    let items: [OverviewCardItem]
    let layout: OverviewLayout
    /// The working accent (a bot's colour; coral otherwise).
    var accent: Color = Theme.accentChat
    /// The face or icon before a row's / card's title.
    @ViewBuilder var leading: (OverviewCardItem) -> Leading
    let onOpen: (OverviewCardItem) -> Void

    var body: some View {
        switch layout {
        case .list:
            LazyVStack(spacing: 0) {
                ForEach(items) { item in
                    Button { onOpen(item) } label: { row(item) }
                        .buttonStyle(.plain)
                }
            }
        case .grid:
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 12), GridItem(.flexible(), spacing: 12)], spacing: 12) {
                ForEach(items) { item in
                    Button { onOpen(item) } label: { card(item, tall: false) }
                        .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16)
        case .deck:
            ScrollView(.horizontal, showsIndicators: false) {
                LazyHStack(spacing: 12) {
                    ForEach(items) { item in
                        Button { onOpen(item) } label: { card(item, tall: true) }
                            .buttonStyle(.plain)
                            .frame(width: OverviewDeck.width, height: OverviewDeck.height)
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
                .scrollTargetLayout()
            }
            .scrollTargetBehavior(.viewAligned)
        }
    }

    private func row(_ item: OverviewCardItem) -> some View {
        HStack(spacing: 12) {
            leading(item).frame(minWidth: 24)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text(item.title)
                        .font(.system(size: 15))
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                    Spacer(minLength: 0)
                    if let meta = item.meta {
                        Text(meta)
                            .font(.system(size: 12).monospacedDigit())
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                if let preview = item.preview, !preview.isEmpty {
                    Text(preview)
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                pill(item)
            }
        }
        .padding(.horizontal, 20)
        .frame(minHeight: 58)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
    }

    private func card(_ item: OverviewCardItem, tall: Bool) -> some View {
        let working = item.tone == .working
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                leading(item)
                if let group = item.group {
                    Text(group)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                if let meta = item.meta {
                    Text(meta)
                        .font(.system(size: 11).monospacedDigit())
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            Text(item.title)
                .font(.system(size: tall ? 15.5 : 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(2)
                .multilineTextAlignment(.leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            pill(item)
            if let preview = item.preview, !preview.isEmpty {
                Text(preview)
                    .font(.system(size: 13))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(tall ? 12 : 4)
                    .multilineTextAlignment(.leading)
            }
            Spacer(minLength: 0)
            if let footer = item.footer {
                Text(footer)
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, minHeight: tall ? nil : 150, maxHeight: tall ? .infinity : nil, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(working ? accent : Color("BorderSubtle"), lineWidth: 1)
        )
        // A working card gets a soft halo (3px at 14%).
        .background(
            RoundedRectangle(cornerRadius: 19, style: .continuous)
                .stroke(accent.opacity(working && !tall ? 0.14 : 0), lineWidth: 3)
                .padding(-1.5)
        )
        .shadow(color: .black.opacity(tall ? 0.10 : 0), radius: 15, y: 10)
        .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(.isButton)
    }

    private func tint(_ tone: OverviewCardItem.Tone) -> Color {
        switch tone {
        case .working: return accent
        case .waiting: return Theme.statusWarning
        case .suggested: return Theme.statusInfo
        case .done, .idle: return Color("TextTertiary")
        }
    }

    /// OverviewStatusPill: Working pulses in the accent, Needs you is amber.
    @ViewBuilder
    private func pill(_ item: OverviewCardItem) -> some View {
        if let word = item.status {
            let color = tint(item.tone)
            HStack(spacing: 4) {
                if item.tone == .working {
                    Circle().fill(color).frame(width: 6, height: 6)
                        .phaseAnimator([1.0, 0.35]) { dot, phase in dot.opacity(phase) } animation: { _ in .easeInOut(duration: 0.9) }
                }
                if item.tone == .waiting {
                    Image(systemName: "hand.raised").font(.system(size: 10))
                }
                Text(word).lineLimit(1)
            }
            .font(.system(size: 10.5, weight: .semibold))
            .foregroundColor(color)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(color.opacity(0.13)))
        }
    }
}
