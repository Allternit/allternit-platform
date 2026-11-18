import SwiftUI

/// Where the shelf sits: 56 pt under ☰, 12 pt from the left edge, one slot
/// per item (your bots, then Create a bot). Slots shrink so every bot fits
/// on one row; the dock magnify makes the one under the finger readable.
struct BotShelfLayout {
    static let inset: CGFloat = 12
    static let height: CGFloat = 64
    static let edge: CGFloat = 12

    let frame: CGRect
    let slot: CGFloat
    let avatar: CGFloat

    init(anchor: CGRect, width: CGFloat, count: Int) {
        let n = CGFloat(max(1, count))
        let room = max(0, width - Self.edge * 2 - Self.inset * 2)
        slot = min(52, room / n)
        avatar = max(20, min(38, slot - 10))
        frame = CGRect(x: Self.edge, y: anchor.maxY + 56, width: slot * n + Self.inset * 2, height: Self.height)
    }
}

/// The bots shelf (C8, brief item 2, option 1; web PhoneBotShelf.tsx): hold
/// ☰, slide to Bots and keep holding 0.4 s. Your bots sit on one horizontal
/// glass shelf with a dock magnify: the bot under the finger grows to 1.55,
/// its neighbours to 1.15. Under the shelf: "Name · i of n" and the bot's
/// status line. Each bot carries its unread count. Release on a bot (or tap
/// one) opens its main chat; the last item creates a bot.
///
/// It is a flat bar that slides out of ☰ sideways, so it never reads as the
/// half-moon. Reduced motion: no magnify and no slide; a ring marks the bot
/// under the finger and the shelf fades.
struct BotShelfView: View {
    let open: Bool
    let anchor: CGRect
    let width: CGFloat
    let reduced: Bool
    @ObservedObject private var hold = MenuHoldController.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @ObservedObject private var botStatus = BotStatusStore.shared

    var body: some View {
        ZStack(alignment: .topLeading) {
            if open {
                shelf
                    .transition(shelfTransition)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var shelfTransition: AnyTransition {
        if reduced { return AnyTransition.opacity.animation(Motion.fade) }
        return AnyTransition.scale(scale: 0.2, anchor: .leading)
            .combined(with: .opacity)
            .animation(.spring(response: 0.3, dampingFraction: 0.82))
    }

    private var shelf: some View {
        let bots = botStore.bots
        let layout = BotShelfLayout(anchor: anchor, width: width, count: bots.count + 1)
        let hotIndex: Int? = {
            switch hold.state.target {
            case .bot(let id)?:
                if id == MenuHold.createBotId { return bots.count }
                return bots.firstIndex { $0.id == id }
            default:
                return nil
            }
        }()
        return VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 0) {
                ForEach(Array(bots.enumerated()), id: \.element.id) { index, bot in
                    item(bot, index: index, count: bots.count, distance: hotIndex.map { abs($0 - index) }, layout: layout)
                }
                createItem(distance: hotIndex.map { abs($0 - bots.count) }, layout: layout)
            }
            .padding(.horizontal, BotShelfLayout.inset)
            .frame(width: layout.frame.width, height: BotShelfLayout.height)
            .glassSurface(cornerRadius: BotShelfLayout.height / 2)
            .shadow(color: .black.opacity(0.12), radius: 14, x: 0, y: 6)
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Your bots")

            label(bots: bots, hotIndex: hotIndex)
        }
        .fixedSize()
        .offset(x: layout.frame.minX, y: layout.frame.minY)
    }

    // MARK: Items

    private func item(_ bot: AgentRecord, index: Int, count: Int, distance: Int?, layout: BotShelfLayout) -> some View {
        let waiting = botStore.waitingCount(for: bot.id) > 0 || botStatus.needsAttention(for: bot.id)
        let working = !waiting && (botStore.workingCount(for: bot.id) > 0 || botStatus.isWorking(for: bot.id))
        let unread = unreadCount(bot.id)
        let extra = [waiting ? "waiting for you" : (working ? "working" : ""), unread > 0 ? "\(unread) unread" : ""]
            .filter { !$0.isEmpty }
            .joined(separator: ", ")
        return Button {
            hold.dispatch(.pick(.bot(bot.id)))
        } label: {
            LiveBotStatusAvatar(bot: bot, size: layout.avatar)
                .overlay(alignment: .topTrailing) {
                    if unread > 0 { ModeDialBadge(count: unread).offset(x: 7, y: -5) }
                }
                .modifier(ShelfMagnify(distance: distance, reduced: reduced))
                .frame(width: layout.slot, height: BotShelfLayout.height)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .zIndex(distance == 0 ? 2 : (distance == 1 ? 1 : 0))
        .accessibilityLabel("Open \(bot.botDisplayName)'s main chat\(extra.isEmpty ? "" : ", \(extra)")")
        .accessibilityHint(shelfPositionLabel(name: bot.botDisplayName, index: index, count: count))
        .accessibilityIdentifier("botShelf-\(targetKey(.bot(bot.id)))")
    }

    private func createItem(distance: Int?, layout: BotShelfLayout) -> some View {
        Button {
            hold.dispatch(.pick(.bot(MenuHold.createBotId)))
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 15, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: layout.avatar, height: layout.avatar)
                .overlay(Circle().stroke(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [3, 3])))
                .modifier(ShelfMagnify(distance: distance, reduced: reduced))
                .frame(width: layout.slot, height: BotShelfLayout.height)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .zIndex(distance == 0 ? 2 : 0)
        .accessibilityLabel("Create a bot")
        .accessibilityIdentifier("botShelfCreate")
    }

    // MARK: Label

    @ViewBuilder
    private func label(bots: [AgentRecord], hotIndex: Int?) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            if let hotIndex, hotIndex < bots.count {
                let bot = bots[hotIndex]
                Text(shelfPositionLabel(name: bot.botDisplayName, index: hotIndex, count: bots.count))
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                Text(statusLine(bot))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            } else if hotIndex != nil || bots.isEmpty {
                Text("Create a bot")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            } else {
                Text("Your bots · \(bots.count)")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .lineLimit(1)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .glassSurface(cornerRadius: 14)
        .padding(.leading, BotShelfLayout.inset - 4)
        // Each bot's own label already says its name, place and status.
        .accessibilityHidden(true)
    }

    private func statusLine(_ bot: AgentRecord) -> String {
        let waiting = botStore.waitingCount(for: bot.id) > 0 || botStatus.needsAttention(for: bot.id)
        let working = botStore.workingCount(for: bot.id) > 0 || botStatus.isWorking(for: bot.id)
        return botShelfStatusLine(
            waiting: waiting,
            working: working,
            activityLabel: botStatus.entry(for: bot.id)?.state.activityLabel,
            offline: botStatus.status(for: bot.id) == .offline,
            isMain: bot.isMainBot
        )
    }

    /// The bot's unread messages; with none reported, its threads waiting on
    /// you (its share of the Bots card count).
    private func unreadCount(_ botId: String) -> Int {
        let unread = botStatus.entry(for: botId)?.state.unreadMessagesCount ?? 0
        return unread > 0 ? unread : botStore.waitingCount(for: botId)
    }
}

/// Dock magnify from the bottom edge, or a ring under reduced motion.
private struct ShelfMagnify: ViewModifier {
    let distance: Int?
    let reduced: Bool

    func body(content: Content) -> some View {
        let magnify = shelfMagnify(distance: distance, reduced: reduced)
        content
            .overlay {
                if reduced && distance == 0 {
                    Circle().stroke(Color("AccentPrimary"), lineWidth: 2).padding(-4)
                }
            }
            .scaleEffect(magnify.scale, anchor: .bottom)
            .offset(y: magnify.lift)
            .animation(reduced ? nil : .spring(response: 0.22, dampingFraction: 0.72), value: distance)
    }
}
