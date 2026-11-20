import SwiftUI
import UIKit

// The ☰ mode fan and the bots shelf host (contract C8, D4, D11; web
// PhoneModeFan.tsx, ai #1149). All gesture rules live in the pure
// `reduceMenuHold` (MenuHoldMachine.swift); `MenuHoldController` runs its
// effects (timers, haptics, drawer, mode switch, opening a bot) and
// `MenuOverlayHost` draws the fan / shelf at shell level, so the header's
// clipping can never cut the arc.
//
// Motion (plan section 6): cards spring(0.26, 0.8) out of ☰ with a 25 ms
// stagger, glass; the card under the finger grows to 1.12; release folds
// them back into ☰. Reduced motion: cards appear in place (fade), no growth,
// a ring marks the card under the finger.

// MARK: - Controller

@MainActor
final class MenuHoldController: ObservableObject {
    static let shared = MenuHoldController()

    @Published private(set) var state = MenuHoldState.initial
    /// The pressed (or newest) ☰'s frame in global coordinates, measured
    /// only while the drawer is fully closed: an open or half-dragged drawer
    /// pushes and scales the page, and a frame from then put the fan mid-screen
    /// when the drawer's mode icon opened it (TestFlight 48).
    @Published private(set) var anchor = CGRect(x: 6, y: 59, width: 44, height: 44)
    /// The shelf's Create a bot item opens the new bot sheet.
    @Published var isCreateBotPresented = false
    /// The overlay canvas size (the window), for clamping the arc.
    var canvasWidth: CGFloat = 390

    var visible: MenuHoldOverlay { menuOverlayVisible(state) }

    // Installed by the shell (MainWorkspaceView).
    private weak var modeStore: AppModeStore?
    private var isDrawerOpen: () -> Bool = { false }
    /// Closed with no drag in progress: the page (and ☰) sits at rest.
    private var isDrawerAtRest: () -> Bool = { true }
    private var openDrawerAction: () -> Void = {}
    private var closeDrawerAction: () -> Void = {}

    /// The pressed ☰'s own open action (each screen opens the drawer its way).
    private var pressOpen: (() -> Void)?
    private var holdTask: Task<Void, Never>?
    private var botsTask: Task<Void, Never>?

    private init() {}

    func install(
        modeStore: AppModeStore,
        isDrawerOpen: @escaping () -> Bool,
        isDrawerAtRest: @escaping () -> Bool,
        openDrawer: @escaping () -> Void,
        closeDrawer: @escaping () -> Void
    ) {
        self.modeStore = modeStore
        self.isDrawerOpen = isDrawerOpen
        self.isDrawerAtRest = isDrawerAtRest
        self.openDrawerAction = openDrawer
        self.closeDrawerAction = closeDrawer
    }

    /// Every ☰ / fan / shelf / drawer event goes through here.
    func dispatch(_ event: MenuHoldEvent) {
        var current = state
        current.drawerOpen = isDrawerOpen()
        let result = reduceMenuHold(current, event)
        if result.state != state { state = result.state }
        run(result.effects)
    }

    // MARK: ☰ gesture

    /// A ☰ reports where it is while nothing is open (D4 opens the fan from the newest one).
    func noteAnchor(_ rect: CGRect) {
        guard rect.width > 0, isDrawerAtRest(), state.overlay == .none, !state.pressing, rect != anchor else { return }
        anchor = rect
    }

    func press(anchor rect: CGRect, onOpen: @escaping () -> Void) {
        if rect.width > 0, isDrawerAtRest() { anchor = rect }
        pressOpen = onOpen
        Haptics.prepare()
        dispatch(.press)
    }

    /// The finger moved (global coordinates). Before the hold, travel past the
    /// slop cancels (a scroll, not a hold); after it, the target follows the finger.
    func drag(location: CGPoint, translation: CGSize) {
        guard state.pressing else { return }
        if !state.held {
            if hypot(translation.width, translation.height) > MenuHold.slop { dispatch(.cancel) }
            return
        }
        guard visible != .none else { return }
        dispatch(.move(target(at: location)))
    }

    func endPress() {
        dispatch(.release)
        pressOpen = nil
    }

    /// The fan card or shelf bot under a global point.
    func target(at point: CGPoint) -> MenuHoldTarget? {
        switch visible {
        case .fan:
            let frames = ModeFanLayout.cardFrames(anchor: anchor, width: canvasWidth)
            for (index, frame) in frames.enumerated() where frame.insetBy(dx: -6, dy: -6).contains(point) {
                return .mode(MenuHold.fanModes[index])
            }
            return nil
        case .shelf:
            let targets = shelfTargets
            let layout = BotShelfLayout(anchor: anchor, width: canvasWidth, count: targets.count)
            guard let column = shelfColumn(at: point, shelf: layout.frame, inset: BotShelfLayout.inset, count: targets.count) else { return nil }
            return targets[column]
        case .none:
            return nil
        }
    }

    /// The shelf's items in order: your bots, then Create a bot.
    var shelfTargets: [MenuHoldTarget] {
        BotModeStore.shared.bots.map { .bot($0.id) } + [.bot(MenuHold.createBotId)]
    }

    // MARK: Direct actions (VoiceOver)

    /// The ☰ VoiceOver action "Switch to <Mode>".
    func switchDirectly(to mode: ShellMode) {
        Haptics.light()
        dispatch(.outsideTap)
        guard let modeStore, modeStore.shellMode != mode else { return }
        modeStore.setShellMode(mode)
    }

    // MARK: Effects

    private func run(_ effects: [MenuHoldEffect]) {
        for effect in effects {
            switch effect {
            case .clearTimers:
                holdTask?.cancel(); holdTask = nil
                botsTask?.cancel(); botsTask = nil
            case .clearBotsHoldTimer:
                botsTask?.cancel(); botsTask = nil
            case .startHoldTimer:
                holdTask?.cancel()
                holdTask = Task { [weak self] in
                    try? await Task.sleep(for: .seconds(MenuHold.holdSeconds))
                    guard !Task.isCancelled else { return }
                    self?.holdTask = nil
                    self?.dispatch(.holdTimer)
                }
            case .startBotsHoldTimer:
                botsTask?.cancel()
                botsTask = Task { [weak self] in
                    try? await Task.sleep(for: .seconds(MenuHold.botsHoldSeconds))
                    guard !Task.isCancelled else { return }
                    self?.botsTask = nil
                    self?.dispatch(.botsHoldTimer)
                }
            case .haptic(.light): Haptics.light()
            case .haptic(.medium): Haptics.medium()
            case .haptic(.selection): Haptics.selection()
            case .openDrawer:
                (pressOpen ?? openDrawerAction)()
            case .closeDrawer:
                closeDrawerAction()
            case .switchMode(let mode):
                if let modeStore, modeStore.shellMode != mode { modeStore.setShellMode(mode) }
            case .openBot(let id):
                if id == MenuHold.createBotId {
                    isCreateBotPresented = true
                } else {
                    // The same path as the old ☰ bot strip and the Bots rail.
                    BotModeStore.shared.pendingOpenBotId = id
                    modeStore?.selectBarItem(.agents)
                }
            }
        }
    }

    #if DEBUG
    /// `-calm-fan` / `-calm-shelf`: open with a card or bot under a pretend
    /// finger, so the screenshot shows the growth / magnify and the label.
    func debugOpen(_ overlay: MenuHoldOverlay) {
        dispatch(overlay == .shelf ? .openShelf : .openFan)
        if overlay == .fan {
            state.target = .mode(.code)
            return
        }
        Task { [weak self] in
            // The sample workspace's bots arrive with the agent list.
            for _ in 0..<30 {
                guard let self, self.state.overlay == .shelf else { return }
                let bots = BotModeStore.shared.bots
                if !bots.isEmpty {
                    self.state.target = .bot(bots[min(1, bots.count - 1)].id)
                    return
                }
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }
    #endif
}

// MARK: - Fan layout

/// Card spots from ☰'s centre (web FAN_SPOTS): a quarter arc of radius 148
/// from straight down (Home) to the right (ACI), nudged 16 down, 60 pt cards
/// kept 8 pt inside the screen edges.
enum ModeFanLayout {
    static let radius: CGFloat = 148
    static let card: CGFloat = 60

    static func cardFrames(anchor: CGRect, width: CGFloat) -> [CGRect] {
        let count = MenuHold.fanModes.count
        return (0..<count).map { index in
            let angle = (Double.pi / 2) * (1 - Double(index) / Double(max(1, count - 1)))
            let x = (cos(angle) * radius).rounded()
            let y = (sin(angle) * radius).rounded() + 16
            let left = min(max(8, anchor.midX + x - card / 2), max(8, width - card - 8))
            return CGRect(x: left, y: anchor.midY + y - card / 2, width: card, height: card)
        }
    }
}

// MARK: - Overlay host

/// Draws the scrim, the fan and the shelf over the whole shell (above the
/// drawer and the page). Hit-testing is off while nothing is open.
struct MenuOverlayHost: View {
    @ObservedObject private var hold = MenuHoldController.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @EnvironmentObject private var modeStore: AppModeStore
    @ReducedMotion private var reduced

    var body: some View {
        GeometryReader { proxy in
            let origin = proxy.frame(in: .global).origin
            let visible = hold.visible
            ZStack(alignment: .topLeading) {
                if visible != .none {
                    Color.black.opacity(0.08)
                        .contentShape(Rectangle())
                        .onTapGesture { hold.dispatch(.outsideTap) }
                        .transition(.opacity.animation(.easeOut(duration: 0.16)))
                        .accessibilityHidden(true)
                }
                // Both stay mounted so each card / the shelf transitions in and out.
                Group {
                    ModeFanView(open: visible == .fan, anchor: hold.anchor, width: proxy.size.width, reduced: reduced)
                    BotShelfView(open: visible == .shelf, anchor: hold.anchor, width: proxy.size.width, reduced: reduced)
                }
                // Layouts are in global coordinates; this canvas may not start at 0,0.
                .offset(x: -origin.x, y: -origin.y)
            }
            .frame(width: proxy.size.width, height: proxy.size.height, alignment: .topLeading)
            .onAppear { hold.canvasWidth = proxy.size.width }
            .onChange(of: proxy.size.width) { _, width in hold.canvasWidth = width }
        }
        .ignoresSafeArea()
        .allowsHitTesting(hold.visible != .none)
        .accessibilityHidden(hold.visible == .none)
        .accessibilityAddTraits(hold.visible == .none ? [] : .isModal)
        .accessibilityAction(.escape) { hold.dispatch(.outsideTap) }
        .onChange(of: hold.state.openedByAction) { _, byAction in
            // Opened from the drawer icon or VoiceOver: move VoiceOver into it.
            if byAction { UIAccessibility.post(notification: .screenChanged, argument: nil) }
        }
        .sheet(isPresented: $hold.isCreateBotPresented) {
            NewBotSheet { outcome in
                // A new bot opens straight into its first thread (useCreateBotSubmit).
                Task {
                    await botStore.refreshAll()
                    if let botId = outcome.botIds.first {
                        botStore.pendingOpenBotId = botId
                        modeStore.selectBarItem(.agents)
                    }
                }
            }
        }
        .onAppear {
            #if DEBUG
            let fan = launchArgumentEnabled(LaunchArgument.calmFan) || launchArgumentEnabled("open-mode-dial")
            let shelf = launchArgumentEnabled(LaunchArgument.calmShelf)
            guard fan || shelf else { return }
            if shelf { AgentHubStore.shared.fetchAgentsIfNeeded() }
            Task {
                // Let the header's ☰ report where it is first.
                try? await Task.sleep(for: .milliseconds(700))
                hold.debugOpen(shelf ? .shelf : .fan)
            }
            #endif
        }
    }
}

// MARK: - Fan

/// Home · Bots · Code · ACI as glass cards on an arc out of ☰.
struct ModeFanView: View {
    let open: Bool
    let anchor: CGRect
    let width: CGFloat
    let reduced: Bool
    @ObservedObject private var hold = MenuHoldController.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @EnvironmentObject private var modeStore: AppModeStore

    var body: some View {
        let frames = ModeFanLayout.cardFrames(anchor: anchor, width: width)
        ZStack(alignment: .topLeading) {
            if open {
                ForEach(Array(MenuHold.fanModes.enumerated()), id: \.element) { index, mode in
                    let frame = frames[index]
                    card(mode)
                        .position(x: frame.midX, y: frame.midY)
                        .transition(transition(index: index, from: frame))
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Switch mode")
    }

    private func transition(index: Int, from frame: CGRect) -> AnyTransition {
        if reduced { return AnyTransition.opacity.animation(Motion.fade) }
        let fold = FanFold(dx: anchor.midX - frame.midX, dy: anchor.midY - frame.midY, folded: true)
        let open = FanFold(dx: 0, dy: 0, folded: false)
        return AnyTransition.modifier(active: fold, identity: open)
            .animation(Motion.fan.delay(Double(index) * Motion.fanStagger))
    }

    private func card(_ mode: ShellMode) -> some View {
        let current = mode == modeStore.shellMode
        let hot = hold.state.target == .mode(mode)
        let unread = mode == .bots ? botStore.waitingCount : 0
        return Button {
            hold.dispatch(.pick(.mode(mode)))
        } label: {
            VStack(spacing: 3) {
                Image(systemName: mode.icon)
                    .font(.system(size: 14, weight: current ? .bold : .regular))
                    .foregroundColor(current ? Color("BgPrimary") : Color("TextPrimary"))
                    .frame(width: 30, height: 30)
                    .background(Circle().fill(current ? Color("TextPrimary") : Color("FillNeutral")))
                Text(mode.label)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
            }
            .frame(width: ModeFanLayout.card, height: ModeFanLayout.card)
            .glassSurface(cornerRadius: 18)
            .overlay {
                if reduced && hot {
                    RoundedRectangle(cornerRadius: 18, style: .continuous)
                        .stroke(Color("AccentPrimary"), lineWidth: 2)
                }
            }
            .shadow(color: .black.opacity(0.12), radius: 12, x: 0, y: 6)
            .overlay(alignment: .topTrailing) {
                if unread > 0 { ModeDialBadge(count: unread).offset(x: 4, y: -4) }
            }
            .scaleEffect(hot && !reduced ? 1.12 : 1)
            .animation(reduced ? nil : Motion.fan, value: hot)
            .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(mode == .bots && unread > 0 ? "Bots, \(unread) waiting" : mode.label)
        .accessibilityAddTraits(current ? [.isSelected] : [])
        .accessibilityIdentifier("modeFanCard\(mode.rawValue)")
        .modifier(BotsCardShelfAction(enabled: mode == .bots))
    }
}

/// VoiceOver can't hold: the Bots card carries "Open your bots".
private struct BotsCardShelfAction: ViewModifier {
    let enabled: Bool
    func body(content: Content) -> some View {
        if enabled {
            content.accessibilityAction(named: "Open your bots") { MenuHoldController.shared.dispatch(.openShelf) }
        } else {
            content
        }
    }
}

/// A fan card folded into ☰: half size, transparent, at ☰'s centre.
private struct FanFold: ViewModifier {
    let dx: CGFloat
    let dy: CGFloat
    let folded: Bool

    func body(content: Content) -> some View {
        content
            .scaleEffect(folded ? 0.5 : 1)
            .offset(x: folded ? dx : 0, y: folded ? dy : 0)
            .opacity(folded ? 0 : 1)
    }
}

// MARK: - Drawer mode icon (D4)

/// The drawer's mode icon (web PhoneModeWheel after ai #1149): the active
/// mode as a round filled icon. Tap closes the drawer, then opens the fan out
/// of ☰ (D4), so hold is never the only way to switch mode. The fan and the
/// drawer are never open together.
struct ModeDialButton: View {
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var botStore = BotModeStore.shared

    var body: some View {
        let mode = modeStore.shellMode
        let unread = botStore.waitingCount
        Button {
            // The machine plays the medium haptic and closes the drawer.
            MenuHoldController.shared.dispatch(.openFan)
        } label: {
            Image(systemName: mode.icon)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("BgPrimary"))
                .frame(width: 34, height: 34)
                .background(Circle().fill(Color("TextPrimary")))
                .frame(width: 44, height: 44)
                // Bots' unread, as on the desktop dial (`phone-mode-badge`).
                .overlay(alignment: .topTrailing) {
                    if unread > 0 { ModeDialBadge(count: unread).offset(x: -1, y: 1) }
                }
                .contentShape(Circle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(mode.label) mode. Switch mode\(unread > 0 ? ", \(unread) waiting in Bots" : "")")
        .accessibilityHint("Closes the menu and shows the modes")
        .accessibilityIdentifier("modeDial")
    }
}

/// The red unread count on the dial, ☰'s Bots card and the shelf
/// (`phone-mode-badge`): 16pt, "9+" past nine.
struct ModeDialBadge: View {
    let count: Int

    var body: some View {
        Text(menuBadgeText(count))
            .font(.system(size: 10, weight: .bold))
            .foregroundColor(.white)
            .padding(.horizontal, 4)
            .frame(minWidth: 16, minHeight: 16)
            .background(Capsule().fill(Theme.statusError))
            .accessibilityHidden(true)
    }
}

/// The mode's wordmark beside the dial (PhoneDrawer `phone-wordmark`):
/// A://TERNIT, A://TERNIT BOTS, A://TERNIT ACI, or the gizzi code mark.
struct ShellWordmark: View {
    let mode: ShellMode

    var body: some View {
        Group {
            if mode == .code {
                // The pixel GIZZI CODE lockup the web drawer shows.
                GizziCodeWordmark(height: 15)
            } else if let suffix = mode.wordmarkSuffix {
                // Suffixed marks run smaller so they fit beside the dial and search.
                PixelWordmark(height: 12, suffix: suffix)
            } else {
                PixelWordmark(height: 16)
            }
        }
        .lineLimit(1)
        .accessibilityIdentifier("shellWordmark\(mode.rawValue)")
    }
}
