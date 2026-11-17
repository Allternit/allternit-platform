import Combine
import SwiftUI

// The computer chip above the composer (phone calm pass, contract C4): the
// native twin of the PWA's `PhoneComputerChip` (allternit-ai
// src/fabric-session/phone/PhoneNoComputer.tsx). It replaces the top strip
// the shell used to show over every screen. One line in Chat, Bots and Code:
// "No computer connected · Connect", "Can't reach your computer · Fix",
// "Waking your computer…", or a compute gate notice. ✕ folds it into a dot
// on the composer's +; a different state brings it back. Screens with no
// composer say it in their empty state instead (FriendlyStateView).

// MARK: - State (pure)

enum ComputerChipKind: String, Sendable, CaseIterable {
    case none, connect, fix, waking, starting, gate, suspended, runway
}

/// What the chip's button opens: the connect kit (the computer picker) or
/// the compute gate sheet.
enum ComputerChipAction: String, Sendable, Identifiable {
    case connect, fix, gate
    var id: String { rawValue }
}

struct ComputerChipState: Equatable, Sendable {
    let kind: ComputerChipKind
    let text: String
    /// Button label and what it opens; nil = no button (waking, starting).
    let actionLabel: String?
    let action: ComputerChipAction?

    static let hidden = ComputerChipState(kind: .none, text: "", actionLabel: nil, action: nil)

    /// What the ✕ dismissal is keyed to (web `useComputerChip` key).
    var key: String { kind == .none ? "none" : "\(kind.rawValue):\(text)" }

    /// A spinner instead of a button.
    var showsProgress: Bool { kind == .waking || kind == .starting }
}

struct ComputerChipInput: Equatable, Sendable {
    /// Not signed in (or the DEBUG skip-auth bypass): no chip at all.
    var signedOut = false
    /// `compute-status` says agents are gated (no computer on the account).
    var gate = false
    /// Credits ran out and the hosted computer stopped.
    var suspended = false
    /// The hosted computer is being made or started.
    var starting = false
    /// Low credits runway line, when there is one.
    var runway: String? = nil
    /// A computer answers (relay available or still unknown, or a direct
    /// `gizzi serve --tunnel` instance is online).
    var runtimeAvailable = true
    /// A sleeping cloud computer is waking for a request.
    var waking = false
    /// The account has at least one paired computer (online or not).
    var paired = false
    /// A read answered a compute-gate refusal (`ComputeReadState.refused`).
    var readRefused = false
}

/// One chip state, most urgent first. Pure: the order is the contract
/// (web `computerChipState`). iOS has no full-screen gate, so the gate shows
/// here too, and its button opens the gate sheet.
func computerChipState(_ input: ComputerChipInput) -> ComputerChipState {
    if input.signedOut { return .hidden }
    if input.suspended {
        return ComputerChipState(kind: .suspended, text: "Your credits ran out",
                                 actionLabel: ComputeGateCopy.cardCButton, action: .gate)
    }
    if input.gate {
        return ComputerChipState(kind: .gate, text: CalmCopy.noComputer, actionLabel: "Connect", action: .gate)
    }
    if input.starting {
        return ComputerChipState(kind: .starting, text: ComputeGateCopy.provisioning, actionLabel: nil, action: nil)
    }
    if input.waking {
        return ComputerChipState(kind: .waking, text: CalmCopy.waking, actionLabel: nil, action: nil)
    }
    if input.readRefused && !input.paired {
        return ComputerChipState(kind: .connect, text: CalmCopy.noComputer, actionLabel: "Connect", action: .connect)
    }
    if !input.runtimeAvailable {
        return input.paired
            ? ComputerChipState(kind: .fix, text: CalmCopy.cantReach, actionLabel: "Fix", action: .fix)
            : ComputerChipState(kind: .connect, text: CalmCopy.noComputer, actionLabel: "Connect", action: .connect)
    }
    if let runway = input.runway {
        return ComputerChipState(kind: .runway, text: runway, actionLabel: ComputeGateCopy.cardCButton, action: .gate)
    }
    return .hidden
}

/// The chip shows as a dot on + while the state on screen is the one the
/// person closed (web `chipShowsAsDot`).
func chipShowsAsDot(key: String, dismissed: String?) -> Bool {
    key != "none" && dismissed == key
}

/// The states `-calm-chip=<name>` forces (DEBUG screenshots), built through
/// `computerChipState` so they read exactly like the live ones.
func computerChipFixture(_ name: String) -> ComputerChipState? {
    switch name.lowercased() {
    case "none": return ComputerChipState.hidden
    case "nocomputer", "connect": return computerChipState(ComputerChipInput(runtimeAvailable: false))
    case "cantreach", "fix": return computerChipState(ComputerChipInput(runtimeAvailable: false, paired: true))
    case "waking": return computerChipState(ComputerChipInput(waking: true, paired: true))
    case "gate": return computerChipState(ComputerChipInput(gate: true, runtimeAvailable: false))
    case "starting": return computerChipState(ComputerChipInput(starting: true))
    case "suspended": return computerChipState(ComputerChipInput(suspended: true))
    case "runway": return computerChipState(ComputerChipInput(runway: ComputeGateCopy.lowRunway24, paired: true))
    default: return nil
    }
}

// MARK: - Live store

/// The live chip state, shared by every composer so a ✕ in one chat holds in
/// the next. The dismissal lives in memory only: a relaunch shows the chip.
@MainActor
final class ComputerChipStore: ObservableObject {
    static let shared = ComputerChipStore()

    @Published private(set) var state: ComputerChipState = .hidden
    /// The key the person closed with ✕ (nil = not closed).
    @Published private(set) var dismissedKey: String?

    private let forced: ComputerChipState?
    private var cancellables = Set<AnyCancellable>()
    private var wasAvailable = false

    private init() {
        forced = Self.forcedState()
        let sources: [AnyPublisher<Void, Never>] = [
            RuntimeRelayStore.shared.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
            InstanceStore.shared.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
            ComputeStatusStore.shared.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
            ComputeReadState.shared.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
            AuthManager.shared.objectWillChange.map { _ in () }.eraseToAnyPublisher(),
        ]
        // objectWillChange fires before the value moves: read on the next turn.
        Publishers.MergeMany(sources)
            .receive(on: DispatchQueue.main)
            .sink { [weak self] _ in
                MainActor.assumeIsolated { self?.recompute() }
            }
            .store(in: &cancellables)
        recompute()
    }

    /// The chip is folded into the dot on +.
    var showsDot: Bool { chipShowsAsDot(key: state.key, dismissed: dismissedKey) }

    /// ✕: fold the chip into the dot until the state changes.
    func dismiss() {
        guard state.kind != .none else { return }
        dismissedKey = state.key
    }

    /// Brings the sources up to date (each call is cheap when fresh).
    func refreshSources() async {
        guard forced == nil, AuthManager.shared.isSignedIn else { return }
        await ComputeStatusStore.shared.refreshIfNeeded()
        // Refetches the device list only when it is older than 30 s.
        _ = await RuntimeRelayStore.shared.resolveActiveRuntimeId()
        await InstanceStore.shared.refreshIfNeeded()
    }

    private func recompute() {
        let relay = RuntimeRelayStore.shared
        let compute = ComputeStatusStore.shared
        let directOnline = Self.directInstanceOnline
        let available = directOnline || relay.availability == .available

        // A computer paired or came back: an earlier read refusal is stale,
        // and a gate read before pairing is worth checking again.
        if available {
            ComputeReadState.shared.clear()
            if !wasAvailable, compute.status?.gate == true {
                Task { await compute.refresh() }
            }
        }
        wasAvailable = available

        let next: ComputerChipState
        if let forced {
            next = forced
        } else {
            let status = compute.status
            next = computerChipState(ComputerChipInput(
                signedOut: !AuthManager.shared.isSignedIn,
                gate: status?.gate ?? false,
                suspended: status?.isSuspended ?? false,
                starting: status.map(ComputeGateNotice.isStarting) ?? false,
                runway: status.flatMap(ComputeGateNotice.runway),
                runtimeAvailable: directOnline || relay.availability == .available || relay.availability == .unknown,
                waking: relay.isWaking,
                paired: !relay.devices.isEmpty,
                readRefused: ComputeReadState.shared.refused
            ))
        }
        guard next != state else { return }
        state = next
        // A different state clears the dismissal (web setComputerChipKey).
        if dismissedKey != next.key { dismissedKey = nil }
    }

    /// A registered `gizzi serve --tunnel` instance that is online is a
    /// direct target of its own (InstanceConnection.resolve).
    private static var directInstanceOnline: Bool {
        #if DEBUG
        if UserDefaults.standard.string(forKey: "gizzi-url") != nil { return true }
        #endif
        return InstanceStore.shared.instances.contains(where: \.isOnline)
    }

    private static func forcedState() -> ComputerChipState? {
        #if DEBUG
        guard let raw = launchArgumentValue(LaunchArgument.calmChip) else { return nil }
        return computerChipFixture(raw)
        #else
        return nil
        #endif
    }
}

// MARK: - Views

/// The chip's place above a composer. Always in the tree (so its sheets stay
/// up while the state moves); it draws nothing when there is nothing to say.
/// `dismissible` is false where the composer has no + to fold into.
struct ComputerChipSlot: View {
    var dismissible = true

    @ObservedObject private var store = ComputerChipStore.shared
    @ReducedMotion private var reduceMotion
    @State private var sheet: ComputerChipAction?

    private var visible: Bool {
        store.state.kind != .none && !(dismissible && store.showsDot)
    }

    private var animation: Animation { Motion.pick(Motion.deck, reduced: reduceMotion) }

    /// ✕ shrinks the chip toward + (bottom leading); reduced motion fades.
    private var chipTransition: AnyTransition {
        reduceMotion
            ? .opacity
            : .asymmetric(
                insertion: .opacity.combined(with: .scale(scale: 0.96, anchor: .bottom)),
                removal: .scale(scale: 0.05, anchor: .bottomLeading).combined(with: .opacity)
            )
    }

    var body: some View {
        VStack(spacing: 0) {
            if visible {
                ComputerChipView(
                    state: store.state,
                    dismissible: dismissible,
                    onAction: { sheet = $0 },
                    onDismiss: {
                        Haptics.soft()
                        withAnimation(animation) { store.dismiss() }
                    }
                )
                .padding(.bottom, 8)
                .transition(chipTransition)
            }
        }
        .frame(maxWidth: .infinity)
        .animation(animation, value: store.state)
        .task { await store.refreshSources() }
        .sheet(item: $sheet) { action in
            switch action {
            case .connect, .fix: ConnectComputerSheet()
            case .gate: ComputeGateSheet()
            }
        }
    }
}

/// One glass line: desktop icon, the text, the action (or a spinner), ✕.
struct ComputerChipView: View {
    let state: ComputerChipState
    var dismissible = true
    let onAction: (ComputerChipAction) -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 14, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .accessibilityHidden(true)
            Text(state.text)
                .font(.system(size: 13))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
            if state.showsProgress {
                ProgressView().controlSize(.small)
                    .padding(.horizontal, 6)
                    .accessibilityHidden(true)
            } else if let label = state.actionLabel, let action = state.action {
                Button(action: {
                    Haptics.light()
                    onAction(action)
                }) {
                    Text(label)
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 10)
                        .frame(minHeight: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            if dismissible {
                Button(action: onDismiss) {
                    Image(systemName: "xmark")
                        .font(.system(size: 11, weight: .bold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 36, height: 36)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Hide")
            }
        }
        .padding(.leading, 12)
        .padding(.trailing, 2)
        .frame(minHeight: 44)
        .glassSurface(cornerRadius: 16)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(state.text)
    }
}

/// The dot on the composer's + while the chip is folded away. Sits in the
/// +'s top-trailing corner; never takes taps (the + still opens its sheet).
struct ComputerChipDot: View {
    @ObservedObject private var store = ComputerChipStore.shared
    @ReducedMotion private var reduceMotion

    var body: some View {
        ZStack {
            if store.showsDot {
                Circle()
                    .fill(Color("TextPrimary"))
                    .frame(width: 10, height: 10)
                    .overlay(Circle().stroke(Color("BgPanel"), lineWidth: 2))
                    .transition(reduceMotion ? .opacity : .scale(scale: 0.2).combined(with: .opacity))
                    .accessibilityLabel("Computer notice")
            }
        }
        .animation(Motion.pick(Motion.deck, reduced: reduceMotion), value: store.showsDot)
        .allowsHitTesting(false)
    }
}
