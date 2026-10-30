import SwiftUI
import UIKit

/// The game window opened from a message box while its bot works (web
/// waiting-games.store.ts). Only the Play button that opened it reports the
/// work, so another chat's composer can't mark this reply ready. `work` is
/// nil once that composer is gone.
@MainActor
final class WaitingGamesStore: ObservableObject {
    enum Work { case working, done }

    static let shared = WaitingGamesStore()
    /// Settings → Appearance → Waiting games (`appearance.waitingGames`).
    static let enabledKey = "appearance.waitingGames"
    private static let gameKey = "allternit.waitingGames.game"

    @Published private(set) var open = false
    @Published var expanded = false
    @Published private(set) var game: WaitingGameID
    @Published private(set) var work: Work? = nil
    private(set) var owner: String? = nil

    private init() {
        game = UserDefaults.standard.string(forKey: Self.gameKey).flatMap(WaitingGameID.init(rawValue:)) ?? .fish
    }

    func openFor(_ owner: String) {
        self.owner = owner
        expanded = false
        work = .working
        open = true
    }

    func close() {
        open = false
        expanded = false
        owner = nil
        work = nil
    }

    func setGame(_ game: WaitingGameID) {
        UserDefaults.standard.set(game.rawValue, forKey: Self.gameKey)
        self.game = game
    }

    /// A Play button's bot is working (true), finished (false) or the button
    /// is gone (nil).
    func report(_ owner: String, busy: Bool?) {
        guard open, self.owner == owner else { return }
        let next: Work?
        switch busy {
        case nil: next = nil
        case true?: next = .working
        case false?: next = work == .working ? .done : work
        }
        if next != work { work = next }
    }
}

// MARK: - Play button

/// Play, beside Stop in a message box while its bot works (web
/// WaitingGamesPlayButton). Keep it mounted whether or not the bot is busy:
/// it tells the open game when the reply is ready.
struct WaitingGamesPlayButton: View {
    let busy: Bool
    /// Work shorter than this never shows the button, so quick replies stay calm.
    private static let showAfter: UInt64 = 5_000_000_000

    @ObservedObject private var store = WaitingGamesStore.shared
    @AppStorage(WaitingGamesStore.enabledKey) private var enabled = true
    @State private var owner = UUID().uuidString
    @State private var shown = false

    private var playing: Bool { store.open && store.owner == owner }

    var body: some View {
        Group {
            if enabled && busy && shown && !playing {
                Button { store.openFor(owner) } label: {
                    Image(systemName: "gamecontroller.fill")
                        .font(.system(size: 14))
                        .foregroundColor(Color("AccentPrimary"))
                        .frame(width: 32, height: 32)
                        .background(Color("AccentPrimary").opacity(0.10), in: Circle())
                        .overlay(Circle().stroke(Color("AccentPrimary").opacity(0.55), lineWidth: 1))
                        .frame(width: 44, height: 44)
                        .contentShape(Circle())
                }
                .buttonStyle(.plain)
                .transition(.scale(scale: 0.9).combined(with: .opacity))
                .accessibilityLabel("Play a game while you wait")
            }
        }
        .task(id: busy) {
            shown = false
            store.report(owner, busy: busy)
            guard busy else { return }
            try? await Task.sleep(nanoseconds: Self.showAfter)
            if !Task.isCancelled { withAnimation(.easeOut(duration: 0.2)) { shown = true } }
        }
        .onDisappear { store.report(owner, busy: nil) }
    }
}

// MARK: - Window

/// Mounted once at the app root (WorkspaceShell). Shows the window while a
/// game is open.
struct WaitingGamesLayer: View {
    @ObservedObject private var store = WaitingGamesStore.shared

    var body: some View {
        ZStack {
            if store.open {
                WaitingGamesWindow()
                    .transition(.scale(scale: 0.95).combined(with: .opacity))
            }
        }
        .animation(.easeOut(duration: 0.2), value: store.open)
        #if DEBUG
        // `-open-waiting-game <id or name>` opens the window (screenshots).
        .task {
            guard let raw = UserDefaults.standard.string(forKey: "open-waiting-game") else { return }
            UserDefaults.standard.removeObject(forKey: "open-waiting-game")
            if let id = WaitingGameID.allCases.first(where: { $0.rawValue == raw.lowercased() || $0.name.lowercased() == raw.lowercased() }) {
                store.setGame(id)
            }
            store.openFor("debug")
        }
        #endif
    }
}

private struct WaitingGamesWindow: View {
    @ObservedObject private var store = WaitingGamesStore.shared
    @StateObject private var engine: WaitingGamesEngine
    @State private var readyDismissed = false
    @State private var touching = false
    @State private var typed = WaitingGamesWindow.sentinel
    @FocusState private var typing: Bool
    @FocusState private var keys: Bool

    /// The hidden field keeps one invisible character so a backspace on an
    /// "empty" field still arrives.
    private static let sentinel = "\u{200B}"
    private static let fallbackRacers = ["Scout", "Pixel", "Byte", "Nova"]

    init() {
        // The racers are read once, when the window opens: the first 4 bots.
        let bots = BotModeStore.shared.bots.prefix(4).map {
            WaitingGameRacer(name: String($0.botDisplayName.prefix(8)), color: $0.botAccentColor)
        }
        let racers = bots.isEmpty ? Self.fallbackRacers.map { WaitingGameRacer(name: $0) } : Array(bots)
        _engine = StateObject(wrappedValue: WaitingGamesEngine(racers: racers))
    }

    private var ready: Bool { store.work == .done && !readyDismissed }

    var body: some View {
        GeometryReader { proxy in
            let expanded = store.expanded
            window
                .frame(height: expanded ? nil : proxy.size.height * 0.46)
                .frame(maxHeight: expanded ? .infinity : nil)
                .clipShape(RoundedRectangle(cornerRadius: expanded ? 0 : 16, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: expanded ? 0 : 16, style: .continuous)
                    .stroke(Color.white.opacity(expanded ? 0 : 0.10), lineWidth: 1))
                .shadow(color: .black.opacity(expanded ? 0 : 0.35), radius: 24, y: 10)
                .padding(.horizontal, expanded ? 0 : 8)
                // Docked above the message box on a phone.
                .padding(.bottom, expanded ? 0 : 168)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        }
        .ignoresSafeArea(store.expanded ? .all : [], edges: .all)
        .onAppear {
            engine.pick(store.game)
            keys = true
        }
        .onChange(of: store.game) { _, game in
            engine.pick(game)
            if engine.wantsKeyboard { typing = true } else { keys = true }
        }
        .onChange(of: ready) { _, ready in engine.setPlaying(!ready) }
        .onChange(of: store.work) { _, work in if work == .working { readyDismissed = false } }
        .onDisappear { engine.setPlaying(false) }
    }

    private var window: some View {
        VStack(spacing: 0) {
            ZStack(alignment: .top) {
                canvas
                overlays
                topBar
                if engine.wantsKeyboard { hiddenField }
            }
            if !engine.hint.isEmpty {
                Text(engine.hint)
                    .font(.system(size: 11.5))
                    .foregroundColor(.white.opacity(0.6))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
            }
        }
        .padding(.top, store.expanded ? safeTop : 0)
        .background(Color(hex: "#0b1220"))
        .environment(\.colorScheme, .dark)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Waiting games")
        // Hardware keys (Snake's arrows, Stack's space, Race's letters).
        .focusable()
        .focused($keys)
        .focusEffectDisabled()
        .onKeyPress(phases: .down) { press in
            let name: String
            switch press.key {
            case .upArrow: name = "ArrowUp"
            case .downArrow: name = "ArrowDown"
            case .leftArrow: name = "ArrowLeft"
            case .rightArrow: name = "ArrowRight"
            case .return: name = "Enter"
            case .space: name = " "
            case .delete: name = "Backspace"
            case .escape:
                if store.expanded { store.expanded = false } else { store.close() }
                return .handled
            default: name = press.characters
            }
            return engine.key(name) ? .handled : .ignored
        }
    }

    private var safeTop: CGFloat {
        (UIApplication.shared.connectedScenes.first as? UIWindowScene)?.keyWindow?.safeAreaInsets.top ?? 0
    }

    private var canvas: some View {
        let engine = engine
        return TimelineView(.animation(minimumInterval: nil, paused: !engine.playing)) { timeline in
            Canvas { ctx, size in
                MainActor.assumeIsolated {
                    engine.render(&ctx, size: size, date: timeline.date)
                }
            }
        }
        .contentShape(Rectangle())
        .gesture(
            DragGesture(minimumDistance: 0)
                .onChanged { value in
                    let x = Double(value.location.x), y = Double(value.location.y)
                    engine.pointer(x, y)
                    if !touching {
                        touching = true
                        engine.click(x, y)
                    }
                }
                .onEnded { _ in
                    touching = false
                    if engine.wantsKeyboard { typing = true } else { keys = true }
                }
        )
    }

    @ViewBuilder
    private var overlays: some View {
        VStack(spacing: 0) {
            if !engine.hud.isEmpty {
                HStack(spacing: 8) {
                    ForEach(Array(engine.hud.enumerated()), id: \.offset) { item in
                        Text(item.element)
                            .font(.system(size: 12, weight: .semibold, design: .monospaced))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(.black.opacity(0.3), in: Capsule())
                        if item.offset < engine.hud.count - 1 { Spacer(minLength: 0) }
                    }
                }
                .foregroundColor(.white)
                .padding(.horizontal, 10)
                .padding(.top, 48)
                .allowsHitTesting(false)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)

        if ready {
            VStack(spacing: 10) {
                Text("Your reply is ready").font(.system(size: 18, weight: .bold))
                Text("The game is paused where you left it.").foregroundColor(.white.opacity(0.8))
                HStack(spacing: 8) {
                    Button("Back to chat") { store.close() }
                        .font(.system(size: 14, weight: .medium))
                        .foregroundColor(Color(hex: "#18181b"))
                        .padding(.horizontal, 14).padding(.vertical, 7)
                        .background(.white, in: Capsule())
                    Button("Keep playing") { readyDismissed = true }
                        .font(.system(size: 14))
                        .foregroundColor(.white)
                        .padding(.horizontal, 14).padding(.vertical, 7)
                        .overlay(Capsule().stroke(.white.opacity(0.4), lineWidth: 1))
                }
                .buttonStyle(.plain)
            }
            .font(.system(size: 13))
            .foregroundColor(.white)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 20).padding(.vertical, 16)
            .background(.black.opacity(0.5), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .padding(.horizontal, 24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let banner = engine.banner {
            VStack(spacing: 4) {
                Text(banner.title).font(.system(size: 20, weight: .bold))
                if !banner.sub.isEmpty { Text(banner.sub) }
            }
            .font(.system(size: 13))
            .foregroundColor(.white)
            .multilineTextAlignment(.center)
            .padding(.horizontal, 20).padding(.vertical, 14)
            .background(.black.opacity(0.4), in: RoundedRectangle(cornerRadius: 16, style: .continuous))
            .padding(.horizontal, 24)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .allowsHitTesting(false)
        }
    }

    private var topBar: some View {
        HStack(spacing: 6) {
            if let work = store.work {
                HStack(spacing: 6) {
                    Circle()
                        .fill(work == .done ? Color(hex: "#34d399") : Color(hex: "#D97757"))
                        .frame(width: 7, height: 7)
                        .phaseAnimator(work == .done ? [1.0] : [1.0, 0.35]) { dot, a in dot.opacity(a) } animation: { _ in .easeInOut(duration: 0.8) }
                    Text(work == .done ? "Reply ready" : "Working…")
                        .font(.system(size: 12))
                        .lineLimit(1)
                }
                .padding(.horizontal, 10).padding(.vertical, 4)
                .background(.black.opacity(0.3), in: Capsule())
                .accessibilityElement(children: .combine)
            }
            Spacer(minLength: 0)
            Menu {
                ForEach(WaitingGameID.allCases) { g in
                    Button { store.setGame(g) } label: {
                        if g == store.game { Label(g.name, systemImage: "checkmark") } else { Text(g.name) }
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Text(store.game.name).font(.system(size: 12))
                    Image(systemName: "chevron.down").font(.system(size: 9, weight: .semibold))
                }
                .padding(.horizontal, 10).padding(.vertical, 5)
                .background(.black.opacity(0.3), in: Capsule())
                .frame(minHeight: 36)
                .contentShape(Rectangle())
            }
            .accessibilityLabel("Game: \(store.game.name)")
            Button { store.expanded.toggle() } label: {
                Image(systemName: store.expanded ? "arrow.down.right.and.arrow.up.left" : "arrow.up.left.and.arrow.down.right")
                    .font(.system(size: 12, weight: .semibold))
                    .frame(width: 30, height: 26)
                    .background(.black.opacity(0.3), in: Capsule())
                    .frame(width: 36, height: 36)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel(store.expanded ? "Make the game small" : "Full screen")
            Button { store.close() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 12, weight: .bold))
                    .foregroundColor(Color(hex: "#18181b"))
                    .frame(width: 30, height: 26)
                    .background(.white.opacity(0.9), in: Capsule())
                    .frame(width: 36, height: 36)
                    .contentShape(Rectangle())
            }
            .accessibilityLabel("Close the game")
        }
        .buttonStyle(.plain)
        .foregroundColor(.white)
        .padding(.horizontal, 8)
        .padding(.top, 6)
    }

    /// Typed games (Bot Race) read this field so the phone shows a keyboard.
    private var hiddenField: some View {
        TextField("", text: $typed)
            .focused($typing)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .keyboardType(.asciiCapable)
            .submitLabel(.go)
            .onSubmit {
                engine.key("Enter")
                typing = true
            }
            .onChange(of: typed) { _, value in
                guard value != Self.sentinel else { return }
                if !value.hasPrefix(Self.sentinel) {
                    engine.key("Backspace")
                } else {
                    for ch in value.dropFirst(Self.sentinel.count) { engine.key(String(ch)) }
                }
                typed = Self.sentinel
            }
            .frame(width: 1, height: 1)
            .opacity(0.01)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
            .accessibilityLabel("Type the word")
    }
}
