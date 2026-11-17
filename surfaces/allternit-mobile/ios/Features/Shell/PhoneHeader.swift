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
    /// A bot's colour on the Bots home: the bar shows its slice of the wash.
    @Environment(\.botTint) private var botTint

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
    /// The usual header: ☰ leads (tap opens the drawer, hold fans the modes out).
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

/// The ☰ button (PhoneHeader MenuButton, contract C8). Tap opens the
/// drawer. Hold 0.35 s fans the modes out of it (Home · Bots · Code · ACI);
/// slide over the cards and let go on one to switch, or keep holding on Bots
/// for the bots shelf. A short hold and release keeps the fan open for a tap.
/// A hold does nothing while the drawer is open. The rules are
/// `reduceMenuHold`; `MenuHoldController` runs them and `MenuOverlayHost`
/// draws the fan at shell level. A small badge shows the current mode.
///
/// VoiceOver: activate opens the menu; one named action per mode
/// ("Switch to Code", …) and "Open your bots".
struct PhoneMenuButton: View {
    var round: Bool = false
    /// Glyph colour where the header is not the usual ink (Code's terminal chrome).
    var tint: Color? = nil
    var glyphSize: CGFloat? = nil
    let onOpen: () -> Void

    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.menuHoldEnabled) private var holdEnabled
    @State private var buttonFrame: CGRect = .zero
    @State private var pressing = false
    @GestureState private var fingerDown = false

    var body: some View {
        let mode = modeStore.shellMode
        let ink = tint ?? Color("TextPrimary")
        Image(systemName: "line.3.horizontal")
            .font(.system(size: glyphSize ?? (round ? 18 : 20), weight: .regular))
            .foregroundColor(ink)
            .frame(width: 44, height: 44)
            .background(Circle().fill(round ? Color("FillNeutral") : Color.clear))
            .overlay(alignment: .bottomTrailing) {
                MenuModeBadge(mode: mode, fill: ink).offset(x: -4, y: -4)
            }
            .contentShape(Circle())
            .background(
                GeometryReader { proxy in
                    Color.clear
                        .onAppear { remember(proxy.frame(in: .global)) }
                        .onChange(of: proxy.frame(in: .global)) { _, rect in remember(rect) }
                }
            )
            .gesture(pressGesture)
            .onChange(of: fingerDown) { _, down in
                guard !down else { return }
                // The system took the touch (no onEnded): treat it as a cancel.
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(60))
                    if pressing {
                        pressing = false
                        if holdEnabled { MenuHoldController.shared.dispatch(.cancel) }
                    }
                }
            }
            .accessibilityElement()
            .accessibilityLabel("Open menu")
            .accessibilityValue("\(mode.label) mode")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { open() }
            .accessibilityActions {
                if holdEnabled {
                    ForEach(MenuHold.fanModes, id: \.self) { option in
                        Button("Switch to \(option.label)") { MenuHoldController.shared.switchDirectly(to: option) }
                    }
                    Button("Open your bots") { MenuHoldController.shared.dispatch(.openShelf) }
                }
            }
            .accessibilityIdentifier("phoneMenuButton")
    }

    private var pressGesture: some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .global)
            .updating($fingerDown) { _, down, _ in down = true }
            .onChanged { gesture in
                let hold = MenuHoldController.shared
                if !pressing {
                    pressing = true
                    if holdEnabled { hold.press(anchor: buttonFrame, onOpen: onOpen) }
                    return
                }
                if holdEnabled { hold.drag(location: gesture.location, translation: gesture.translation) }
            }
            .onEnded { gesture in
                guard pressing else { return }
                pressing = false
                if holdEnabled {
                    Haptics.prepare()
                    MenuHoldController.shared.endPress()
                } else if hypot(gesture.translation.width, gesture.translation.height) <= MenuHold.slop {
                    open()
                }
            }
    }

    /// The shell plays the drawer's light haptic when it opens.
    private func open() {
        onOpen()
    }

    private func remember(_ rect: CGRect) {
        buttonFrame = rect
        if holdEnabled { MenuHoldController.shared.noteAnchor(rect) }
    }
}

/// The current mode's icon in a 15 pt dot on ☰'s corner. The glyph is
/// knocked out, so it reads on any header colour.
struct MenuModeBadge: View {
    let mode: ShellMode
    var fill: Color = Color("TextPrimary")

    var body: some View {
        ZStack {
            Circle().fill(fill)
            Image(systemName: mode.icon)
                .font(.system(size: 7.5, weight: .bold))
                .blendMode(.destinationOut)
        }
        .compositingGroup()
        .frame(width: 15, height: 15)
        .accessibilityHidden(true)
        .accessibilityIdentifier("phoneMenuModeBadge\(mode.rawValue)")
    }
}

private struct MenuHoldEnabledKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    /// False where the shell's fan can't draw over the screen (a full-screen
    /// cover such as the session overview): ☰ is then tap only.
    var menuHoldEnabled: Bool {
        get { self[MenuHoldEnabledKey.self] }
        set { self[MenuHoldEnabledKey.self] = newValue }
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
