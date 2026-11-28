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
    /// A mode's landing: the mode's pixel wordmark (A://TERNIT, BOTS) sits
    /// centred in the bar in place of the text title.
    var wordmark: ShellMode? = nil
    @ViewBuilder var leading: () -> Leading
    @ViewBuilder var trailing: () -> Trailing
    /// A bot's colour on the Bots home: the bar shows its slice of the wash.
    @Environment(\.botTint) private var botTint

    var body: some View {
        let side: CGFloat = wideSides ? 96 : (sectionStyle ? 44 : 52)
        HStack(spacing: 0) {
            HStack(spacing: 0) { leading() }
                .frame(width: side, alignment: .leading)
            VStack(spacing: 0) {
                if let wordmark {
                    ShellWordmark(mode: wordmark)
                        .accessibilityAddTraits(.isHeader)
                } else if let title, !title.isEmpty {
                    Text(title)
                        .font(.system(size: sectionStyle ? 17 : 16, weight: .semibold))
                        .tracking(-0.2)
                        .foregroundColor(Color("TextPrimary"))
                        .lineLimit(1)
                        .truncationMode(.tail)
                        .accessibilityAddTraits(.isHeader)
                }
                if let subtitle, !subtitle.isEmpty, wordmark == nil {
                    Text(subtitle)
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity)
            HStack(spacing: 0) {
                if Trailing.self == EmptyView.self {
                    trailing()
                } else {
                    // The right-hand actions share one glass pill (D12).
                    HStack(spacing: 2) { trailing() }
                        .padding(.horizontal, 2)
                        .polishGlass(cornerRadius: 22)
                }
            }
            .frame(width: side, alignment: .trailing)
        }
        .frame(height: PhoneHeaderMetrics.barHeight)
        .padding(.horizontal, sectionStyle ? 12 : 6)
        .background {
            if botTint != nil {
                BotWashedBackground().ignoresSafeArea(edges: .top)
            } else {
                Color("BgPrimary")
            }
        }
    }
}

extension PhoneHeader where Leading == PhoneMenuButton {
    /// The usual header: ☰ leads (tap opens the drawer).
    init(
        title: String?,
        subtitle: String? = nil,
        wideSides: Bool = false,
        sectionStyle: Bool = false,
        wordmark: ShellMode? = nil,
        onMenu: @escaping () -> Void,
        @ViewBuilder trailing: @escaping () -> Trailing
    ) {
        self.title = title
        self.subtitle = subtitle
        self.wideSides = wideSides
        self.sectionStyle = sectionStyle
        self.wordmark = wordmark
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
            Haptics.light()
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

/// The hamburger has one action: open the drawer, by touch or keyboard.
struct PhoneMenuButton: View {
    var round: Bool = false
    var tint: Color? = nil
    var glyphSize: CGFloat? = nil
    let onOpen: () -> Void
    @EnvironmentObject private var modeStore: AppModeStore

    var body: some View {
        // Glass circle with no mode dot on its corner (D1). The mode stays
        // in the accessibility value.
        Button {
            Haptics.light()
            onOpen()
        } label: {
            Image(systemName: "line.3.horizontal")
                .font(.system(size: glyphSize ?? 17, weight: .semibold))
                .foregroundStyle(tint ?? PolishTokens.ink)
                .frame(width: 40, height: 40)
                .polishGlass(cornerRadius: 20)
                .frame(width: 44, height: 44)
                .contentShape(Circle())
        }
        .buttonStyle(PhoneMenuPressStyle())
        .accessibilityLabel("Open menu")
        .accessibilityValue("\(modeStore.shellMode.label) mode")
        .accessibilityIdentifier("phoneMenuButton")
    }
}

/// Press scale for the glass ☰ (polish press spring, off under Reduce Motion).
private struct PhoneMenuPressStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? PolishTokens.pressButton : 1)
            .animation(reduceMotion ? nil : PolishTokens.press, value: configuration.isPressed)
    }
}

/// A bot's avatar with its live status dot, read from the bot stores
/// (QuickBotStrip BotStatusAvatar over the shared `BotStatusAvatar`).
struct LiveBotStatusAvatar: View {
    let bot: AgentRecord
    let size: CGFloat
    var tiled = false
    @ObservedObject private var botStore = BotModeStore.shared
    @ObservedObject private var botStatus = BotStatusStore.shared

    var body: some View {
        let waiting = botStore.waitingCount(for: bot.id) > 0 || botStatus.needsAttention(for: bot.id)
        let working = !waiting && (botStore.workingCount(for: bot.id) > 0 || botStatus.isWorking(for: bot.id))
        BotStatusAvatar(bot: bot, size: size, working: working, waiting: waiting, tiled: tiled)
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
            Haptics.light()
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

/// PhoneHeader's bar height without its generic parameters, for callers
/// that need to size around it.
enum PhoneHeaderMetrics {
    static let barHeight: CGFloat = 52
}
