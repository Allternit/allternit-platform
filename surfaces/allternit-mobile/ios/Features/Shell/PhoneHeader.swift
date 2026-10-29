import SwiftUI
import UIKit

/// Phone top bar (allternit-ai src/fabric-session/phone/PhoneHeader.tsx):
/// menu on the left, a centered title (and optional subtitle), and the
/// trailing actions. No separators — content scrolls under it. 52pt tall,
/// 52pt sides (96pt when the trailing side holds two actions).
struct PhoneHeader<Leading: View, Trailing: View>: View {
    var title: String?
    var subtitle: String?
    /// Two trailing actions widen both sides so the title stays centered.
    var wideSides: Bool = false
    /// Section screens (PhoneSectionHeader) use 17pt titles and 12pt insets.
    var sectionStyle: Bool = false
    @ViewBuilder var leading: () -> Leading
    @ViewBuilder var trailing: () -> Trailing

    var body: some View {
        let side: CGFloat = wideSides ? 96 : (sectionStyle ? 44 : 52)
        HStack(spacing: 0) {
            HStack(spacing: 0) { leading() }
                .frame(width: side, alignment: .leading)
            VStack(spacing: 0) {
                if let title, !title.isEmpty {
                    Text(title)
                        .font(.system(size: sectionStyle ? 17 : 16, weight: .semibold))
                        .tracking(-0.2)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .accessibilityAddTraits(.isHeader)
                }
                if let subtitle, !subtitle.isEmpty {
                    Text(subtitle)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity)
            HStack(spacing: 2) { trailing() }
                .frame(width: side, alignment: .trailing)
        }
        .frame(height: 52)
        .padding(.horizontal, sectionStyle ? 12 : 6)
        .background(Color("BgPrimary"))
    }
}

extension PhoneHeader where Leading == PhoneMenuButton {
    /// The usual header: ☰ leads (tap opens the drawer, hold shows your bots).
    init(
        title: String?,
        subtitle: String? = nil,
        wideSides: Bool = false,
        sectionStyle: Bool = false,
        onMenu: @escaping () -> Void,
        @ViewBuilder trailing: @escaping () -> Trailing
    ) {
        self.title = title
        self.subtitle = subtitle
        self.wideSides = wideSides
        self.sectionStyle = sectionStyle
        self.leading = { PhoneMenuButton(round: sectionStyle, onOpen: onMenu) }
        self.trailing = trailing
    }
}

/// A 44pt round header button (`phone-icon-btn`): 22pt glyph, no fill at
/// rest. `round` gives it the section header's neutral fill (`phone-round-btn`).
struct PhoneIconButton: View {
    let systemName: String
    let label: String
    var round: Bool = false
    var showsDot: Bool = false
    let action: () -> Void

    var body: some View {
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        } label: {
            Image(systemName: systemName)
                .font(.system(size: round ? 18 : 20, weight: .regular))
                .foregroundColor(Color("TextPrimary"))
                .frame(width: 44, height: 44)
                .background(Circle().fill(round ? Color("FillNeutral") : Color.clear))
                .overlay(alignment: .topTrailing) {
                    if showsDot { PhoneRoundDot().offset(x: -9, y: 9) }
                }
                .contentShape(Circle())
        }
        .buttonStyle(PhonePressStyle())
        .accessibilityLabel(label)
    }
}

/// The 7pt accent dot on a round button (`phone-round-btn-dot`).
struct PhoneRoundDot: View {
    var body: some View {
        Circle()
            .fill(Color("AccentPrimary"))
            .frame(width: 7, height: 7)
            .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
            .accessibilityHidden(true)
    }
}

/// `:active` feedback for phone buttons: the neutral fill under the press.
struct PhonePressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(Circle().fill(configuration.isPressed ? Color("FillNeutral") : Color.clear))
    }
}

/// The ☰ button (PhoneHeader MenuButton). Tap opens the drawer. Press and
/// hold shows your bots as a floating column under it: tap one to open its
/// main chat; a tap outside closes the strip. With no bots, a hold is a tap.
struct PhoneMenuButton: View {
    var round: Bool = false
    let onOpen: () -> Void

    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var botStore = BotModeStore.shared
    @State private var stripOpen = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Image(systemName: "line.3.horizontal")
            .font(.system(size: round ? 18 : 20, weight: .regular))
            .foregroundColor(Color("TextPrimary"))
            .frame(width: 44, height: 44)
            .background(Circle().fill(round ? Color("FillNeutral") : Color.clear))
            .contentShape(Circle())
            .onTapGesture {
                UIImpactFeedbackGenerator(style: .light).impactOccurred()
                stripOpen = false
                onOpen()
            }
            .onLongPressGesture(minimumDuration: 0.35) {
                guard !botStore.bots.isEmpty else {
                    onOpen()
                    return
                }
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                withAnimation(reduceMotion ? .linear(duration: 0.12) : .easeOut(duration: 0.18)) {
                    stripOpen = true
                }
            }
            .overlay(alignment: .topLeading) {
                if stripOpen { strip }
            }
            .zIndex(stripOpen ? 50 : 0)
            .accessibilityElement()
            .accessibilityLabel("Open menu")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { onOpen() }
            .accessibilityAction(named: "Your bots") { stripOpen = true }
            .accessibilityIdentifier("phoneMenuButton")
    }

    /// QuickBotStrip, phone variant: 40pt avatars with the working (green) /
    /// needs-you (amber) dot, in a rounded column 6pt under the button.
    private var strip: some View {
        ZStack(alignment: .topLeading) {
            // Tap anywhere else closes it.
            Color.black.opacity(0.001)
                .frame(width: 4000, height: 4000)
                .offset(x: -2000, y: -2000)
                .onTapGesture { close() }
                .accessibilityHidden(true)
            ScrollView(.vertical, showsIndicators: false) {
                VStack(spacing: 8) {
                    ForEach(botStore.bots, id: \.id) { bot in
                        Button {
                            close()
                            botStore.pendingOpenBotId = bot.id
                            modeStore.selectBarItem(.agents)
                        } label: {
                            LiveBotStatusAvatar(bot: bot, size: 40)
                                .padding(2)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Open \(bot.botDisplayName)'s main chat\(LiveBotStatusAvatar.stateLabel(bot.id))")
                    }
                }
                .padding(8)
            }
            .frame(width: 64)
            .frame(maxHeight: 520)
            .fixedSize(horizontal: false, vertical: true)
            .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .shadow(color: .black.opacity(0.18), radius: 12, x: 0, y: 8)
            .offset(x: 0, y: 50)
            .transition(reduceMotion ? .opacity : .opacity.combined(with: .scale(scale: 0.97, anchor: .topLeading)))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Your bots")
        }
    }

    private func close() {
        withAnimation(.easeOut(duration: 0.12)) { stripOpen = false }
    }
}

/// A bot's avatar with its live status dot, read from the bot stores
/// (QuickBotStrip BotStatusAvatar over the shared `BotStatusAvatar`).
struct LiveBotStatusAvatar: View {
    let bot: AgentRecord
    let size: CGFloat
    @ObservedObject private var botStore = BotModeStore.shared
    @ObservedObject private var botStatus = BotStatusStore.shared

    var body: some View {
        let waiting = botStore.waitingCount(for: bot.id) > 0 || botStatus.needsAttention(for: bot.id)
        let working = !waiting && (botStore.workingCount(for: bot.id) > 0 || botStatus.isWorking(for: bot.id))
        BotStatusAvatar(bot: bot, size: size, working: working, waiting: waiting)
            .frame(width: size, height: size)
    }

    @MainActor
    static func stateLabel(_ botId: String) -> String {
        let store = BotModeStore.shared
        let status = BotStatusStore.shared
        if store.waitingCount(for: botId) > 0 || status.needsAttention(for: botId) { return ", needs you" }
        if store.workingCount(for: botId) > 0 || status.isWorking(for: botId) { return ", working" }
        return ""
    }
}

/// The header's count button (PhoneOverviewButton): the mode's open
/// sessions in a rounded square, a dot while any is working. Opens the
/// mode's session overview.
struct PhoneOverviewButton: View {
    let mode: ShellMode
    var working: Bool = false
    let action: () -> Void
    @ObservedObject private var store = ShellSessionsStore.shared

    var body: some View {
        let count = store.openSessions(for: mode).count
        let noun = mode == .code ? "code threads" : (mode == .aci ? "agent runs" : "chats")
        Button {
            UIImpactFeedbackGenerator(style: .light).impactOccurred()
            action()
        } label: {
            Text(count > 99 ? "99+" : "\(count)")
                .font(.system(size: 11, weight: .bold))
                .monospacedDigit()
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 4)
                .frame(minWidth: 22, minHeight: 22)
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("TextPrimary"), lineWidth: 1.8))
                .overlay(alignment: .topTrailing) {
                    if working {
                        Circle()
                            .fill(Color("BrandAccent"))
                            .frame(width: 7, height: 7)
                            .overlay(Circle().stroke(Color("BgPrimary"), lineWidth: 2))
                            .offset(x: 4, y: -4)
                    }
                }
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(PhonePressStyle())
        .accessibilityLabel("All \(noun): \(count) open\(working ? ", some working" : "")")
        .accessibilityIdentifier("phoneOverviewButton")
    }
}
