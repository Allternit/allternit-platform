import CoreGraphics
import Foundation

// ☰ menu hold (contract C8, decisions D4 and D11): one pure state machine for
// the phone menu button's tap, hold, mode fan and bots shelf. Port of
// allternit-ai `src/fabric-session/phone/menu-hold.ts` (web step 8, ai #1149):
// same events, effects, constants and rules, so both surfaces behave alike.
//
//  - Tap ☰ opens the drawer. Opening the drawer closes the fan / shelf.
//  - Hold ☰ for `MenuHold.holdSeconds` opens the fan (Home · Bots · Code ·
//    ACI). A hold does nothing while the drawer is open.
//  - Slide over the cards: the card under the finger is the target. Release
//    on a card switches mode. Release on nothing keeps the fan open for a tap.
//  - Stay on the Bots card for `MenuHold.botsHoldSeconds` more: the bots
//    shelf replaces the fan. Release on a bot opens its main chat.
//  - lockedMode (a Bots-only host): a hold goes straight to the shelf.
//  - The drawer's mode icon (D4) sends `openFan`: the drawer closes and the
//    fan opens. Never both.
//  - iOS only: `openShelf` (the ☰ VoiceOver action "Open your bots") opens
//    the shelf directly. The web reaches the shelf from the keyboard through
//    the fan's Bots card instead.
//
// Views only dispatch events and run the returned effects (timers, haptics,
// navigation): `MenuHoldController` in ModeDialView.swift.

enum MenuHold {
    /// Hold on ☰ before the fan opens (web HOLD_MS 350).
    static let holdSeconds: Double = 0.35
    /// Further hold on the Bots card before the shelf opens (web BOTS_HOLD_MS 400).
    static let botsHoldSeconds: Double = 0.4
    /// Finger travel before the hold fires that cancels it (web HOLD_SLOP_PX 12).
    static let slop: CGFloat = 12
    /// The shelf's Create a bot item (web `'__create'`).
    static let createBotId = "__create"
    /// The modes on the fan, in order (desktop mode wheel order).
    static let fanModes: [ShellMode] = [.home, .bots, .code, .aci]
}

enum MenuHoldOverlay: String, Equatable, Sendable {
    case none, fan, shelf
}

/// A fan card or a shelf bot. `.bot(MenuHold.createBotId)` is Create a bot.
enum MenuHoldTarget: Hashable, Sendable {
    case mode(ShellMode)
    case bot(String)

    var isBotsCard: Bool { self == .mode(.bots) }
}

struct MenuHoldState: Equatable, Sendable {
    var overlay: MenuHoldOverlay = .none
    /// A finger is down on ☰.
    var pressing = false
    /// The current press became a hold (so its release is not a tap).
    var held = false
    /// The card or bot under the finger.
    var target: MenuHoldTarget? = nil
    /// Mirrors the shell's drawer (synced before every dispatch).
    var drawerOpen = false
    /// A Bots-only host: holds open the shelf.
    var lockedMode = false
    /// Opened by an action (drawer icon, VoiceOver): focus moves into it.
    var openedByAction = false

    static let initial = MenuHoldState()
}

enum MenuHoldEvent: Equatable, Sendable {
    case press
    case release
    /// The press stopped being a tap or hold (the finger moved before the hold).
    case cancel
    case holdTimer
    case botsHoldTimer
    case move(MenuHoldTarget?)
    /// A tap on a card or bot while the fan / shelf stays open.
    case pick(MenuHoldTarget)
    /// D4 drawer mode icon.
    case openFan
    /// iOS: the ☰ VoiceOver action "Open your bots".
    case openShelf
    case drawerOpened
    case drawerClosed
    /// A tap outside the fan / shelf, or the VoiceOver escape gesture.
    case outsideTap
    case setLocked(Bool)
}

enum MenuHoldHaptic: String, Equatable, Sendable {
    case light, medium, selection
}

enum MenuHoldEffect: Equatable, Sendable {
    case startHoldTimer
    case startBotsHoldTimer
    case clearBotsHoldTimer
    case clearTimers
    case openDrawer
    case closeDrawer
    case switchMode(ShellMode)
    case openBot(String)
    case haptic(MenuHoldHaptic)
}

struct MenuHoldResult: Equatable, Sendable {
    var state: MenuHoldState
    var effects: [MenuHoldEffect]
}

func sameTarget(_ a: MenuHoldTarget?, _ b: MenuHoldTarget?) -> Bool { a == b }

/// Keeps only targets the open overlay can hold: modes in the fan, bots on the shelf.
private func fits(_ overlay: MenuHoldOverlay, _ target: MenuHoldTarget?) -> MenuHoldTarget? {
    guard let target else { return nil }
    switch (overlay, target) {
    case (.fan, .mode), (.shelf, .bot): return target
    default: return nil
    }
}

/// The choice a release or tap on `target` makes.
private func choose(_ state: MenuHoldState, _ target: MenuHoldTarget) -> MenuHoldResult {
    var closed = state
    closed.overlay = .none
    closed.target = nil
    closed.held = false
    closed.pressing = false
    closed.openedByAction = false
    let done: [MenuHoldEffect] = [.clearTimers, .haptic(.light)]
    switch target {
    case .mode(let mode): return MenuHoldResult(state: closed, effects: done + [.switchMode(mode)])
    case .bot(let id): return MenuHoldResult(state: closed, effects: done + [.openBot(id)])
    }
}

private func same(_ state: MenuHoldState) -> MenuHoldResult { MenuHoldResult(state: state, effects: []) }

func reduceMenuHold(_ state: MenuHoldState, _ event: MenuHoldEvent) -> MenuHoldResult {
    var next = state
    switch event {
    case .press:
        next.pressing = true
        next.held = false
        // Hold does nothing while the drawer is open: no timer, the release closes the drawer.
        if state.drawerOpen { return same(next) }
        return MenuHoldResult(state: next, effects: [.clearTimers, .startHoldTimer])

    case .holdTimer:
        if !state.pressing || state.held || state.drawerOpen { return same(state) }
        next.held = true
        next.overlay = state.lockedMode ? .shelf : .fan
        next.target = nil
        next.openedByAction = false
        return MenuHoldResult(state: next, effects: [.haptic(.medium)])

    case .move(let raw):
        if state.overlay == .none { return same(state) }
        let target = fits(state.overlay, raw)
        if sameTarget(target, state.target) { return same(state) }
        var effects: [MenuHoldEffect] = []
        if target != nil { effects.append(.haptic(.selection)) }
        if state.overlay == .fan {
            if state.target?.isBotsCard == true { effects.append(.clearBotsHoldTimer) }
            if target?.isBotsCard == true, state.pressing, state.held, !state.lockedMode {
                effects.append(.startBotsHoldTimer)
            }
        }
        next.target = target
        return MenuHoldResult(state: next, effects: effects)

    case .botsHoldTimer:
        if state.overlay != .fan || !state.pressing || state.target?.isBotsCard != true { return same(state) }
        next.overlay = .shelf
        next.target = nil
        return MenuHoldResult(state: next, effects: [.haptic(.medium)])

    case .release:
        if !state.pressing { return same(state) }
        if !state.held {
            // A tap. With the drawer open it closes the drawer; otherwise it opens it (and closes the fan).
            next.pressing = false
            if state.drawerOpen {
                return MenuHoldResult(state: next, effects: [.clearTimers, .closeDrawer])
            }
            next.overlay = .none
            next.target = nil
            next.drawerOpen = true
            next.openedByAction = false
            return MenuHoldResult(state: next, effects: [.clearTimers, .openDrawer])
        }
        if let target = fits(state.overlay, state.target) { return choose(state, target) }
        // Let go on nothing: the fan / shelf stays open for a tap.
        next.pressing = false
        next.held = false
        next.target = nil
        return MenuHoldResult(state: next, effects: [.clearTimers])

    case .cancel:
        if !state.pressing { return same(state) }
        next.pressing = false
        next.held = false
        if state.held {
            next.overlay = .none
            next.target = nil
        }
        return MenuHoldResult(state: next, effects: [.clearTimers])

    case .pick(let raw):
        guard let target = fits(state.overlay, raw) else { return same(state) }
        return choose(state, target)

    case .openFan, .openShelf:
        next.overlay = (event == .openShelf || state.lockedMode) ? .shelf : .fan
        next.target = nil
        next.pressing = false
        next.held = false
        next.drawerOpen = false
        next.openedByAction = true
        var effects: [MenuHoldEffect] = [.clearTimers]
        if state.drawerOpen { effects.append(.closeDrawer) }
        effects.append(.haptic(.medium))
        return MenuHoldResult(state: next, effects: effects)

    case .drawerOpened:
        next.drawerOpen = true
        next.overlay = .none
        next.target = nil
        next.pressing = false
        next.held = false
        next.openedByAction = false
        return MenuHoldResult(state: next, effects: state.pressing ? [.clearTimers] : [])

    case .drawerClosed:
        next.drawerOpen = false
        return same(next)

    case .outsideTap:
        if state.overlay == .none { return same(state) }
        next.overlay = .none
        next.target = nil
        next.pressing = false
        next.held = false
        next.openedByAction = false
        return MenuHoldResult(state: next, effects: [.clearTimers])

    case .setLocked(let locked):
        next.lockedMode = locked
        if locked && state.overlay == .fan {
            next.overlay = .none
            next.target = nil
        }
        return same(next)
    }
}

/// The overlay the view may show: never the fan or shelf while the drawer is open.
func menuOverlayVisible(overlay: MenuHoldOverlay, drawerOpen: Bool) -> MenuHoldOverlay {
    drawerOpen ? .none : overlay
}

func menuOverlayVisible(_ state: MenuHoldState) -> MenuHoldOverlay {
    menuOverlayVisible(overlay: state.overlay, drawerOpen: state.drawerOpen)
}

/// `mode:<m>` / `bot:<id>` (web `targetKey`), for accessibility identifiers.
func targetKey(_ target: MenuHoldTarget) -> String {
    switch target {
    case .mode(let mode): return "mode:\(mode.rawValue)"
    case .bot(let id): return "bot:\(id)"
    }
}

func parseTargetKey(_ key: String?) -> MenuHoldTarget? {
    guard let key, let at = key.firstIndex(of: ":") else { return nil }
    let kind = key[key.startIndex..<at]
    let value = String(key[key.index(after: at)...])
    if kind == "mode" { return ShellMode(rawValue: value).map(MenuHoldTarget.mode) }
    if kind == "bot", !value.isEmpty { return .bot(value) }
    return nil
}

// MARK: - Shelf and badge helpers (web PhoneBotShelf / PhoneModeFan)

/// Unread badge text: "9+" past nine (web `badgeText`).
func menuBadgeText(_ count: Int) -> String { count > 9 ? "9+" : "\(count)" }

/// Dock magnify for a shelf bot `distance` columns from the one under the
/// finger: 1.55 under it, 1.15 beside it, lifted 14 / 4 pt. Reduced motion
/// or no bot under the finger: no magnify (a ring marks the bot instead).
func shelfMagnify(distance: Int?, reduced: Bool) -> (scale: CGFloat, lift: CGFloat) {
    guard !reduced, let distance else { return (1, 0) }
    switch distance {
    case 0: return (1.55, -14)
    case 1: return (1.15, -4)
    default: return (1, 0)
    }
}

/// "Drop · 2 of 6" under the shelf.
func shelfPositionLabel(name: String, index: Int, count: Int) -> String {
    "\(name) · \(index + 1) of \(count)"
}

/// One calm status line for a bot, no codes (web `botStatusLine`).
func botShelfStatusLine(waiting: Bool, working: Bool, activityLabel: String?, offline: Bool, isMain: Bool) -> String {
    let line: String
    if waiting {
        line = "Waiting for you"
    } else if working {
        if let activityLabel, !activityLabel.isEmpty { line = "Working · \(activityLabel)" } else { line = "Working" }
    } else if offline {
        line = "Offline"
    } else {
        line = "Idle"
    }
    return isMain ? "Main bot · \(line)" : line
}

/// The shelf column under `x` (web `menuHoldTargetAt`, shelf branch): read by
/// column because magnified avatars move under the finger, with room around
/// the glass so a slide that drifts off it still scrubs. nil outside.
func shelfColumn(at point: CGPoint, shelf: CGRect, inset: CGFloat, count: Int) -> Int? {
    guard count > 0 else { return nil }
    let zone = CGRect(x: shelf.minX - 12, y: shelf.minY - 80, width: shelf.width + 24, height: shelf.height + 240)
    guard zone.contains(point) else { return nil }
    let inner = max(1, shelf.width - inset * 2)
    let column = Int(((point.x - shelf.minX - inset) / (inner / CGFloat(count))).rounded(.down))
    return min(max(column, 0), count - 1)
}
