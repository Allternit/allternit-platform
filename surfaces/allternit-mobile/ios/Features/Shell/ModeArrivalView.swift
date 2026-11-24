import SwiftUI

// MARK: - Arrivals (panel B of the mobile revamp; web port: ModeArrival.tsx)
//
// Team arrival: entering Bots with no bot open, the team's bots fly up from
// the orb into "Your team" (spring 260/24, delay .08 + i * .07) over a soft
// 1.1 s wash in all their colours, each ticking a light haptic as it lands.
// Bot arrival: a bot is picked. A wash in its colour comes in first, the big
// avatar rises out of a blur (spring 200/22), then settles into its roster
// face (matchedGeometryEffect). Timeline 0 / 600 / 1600 / 2300 ms.
// Both wait for any mode intro (`ModeIntroGate`) so an intro always comes first.
// Reduce Motion: the bots fade in place, no flight, no wash.

/// Remembers that Bots was just entered, so the roster plays its team arrival once.
@MainActor
final class ModeArrivalCenter: ObservableObject {
    static let shared = ModeArrivalCenter()
    /// True from "Bots mode was entered" until the Bots home consumes it.
    /// Starts true: the first Bots home of a launch is an entrance too.
    @Published var teamPending = true
    private init() {}

    func consumeTeam() -> Bool {
        guard teamPending else { return false }
        teamPending = false
        return true
    }
}

/// Mounted once on the shell: flags the team arrival whenever Bots is entered.
struct ModeArrivalsHost: ViewModifier {
    @EnvironmentObject private var modeStore: AppModeStore

    func body(content: Content) -> some View {
        content.onChange(of: modeStore.shellMode) { _, mode in
            if mode == .bots { ModeArrivalCenter.shared.teamPending = true }
        }
    }
}

extension View {
    /// Mounts the mode arrivals' trigger over the workspace shell.
    func modeArrivals() -> some View { modifier(ModeArrivalsHost()) }
}

// MARK: - Team arrival

/// Drives the team's flight: `flown` flips once the intro gate is clear.
@MainActor
final class TeamArrivalState: ObservableObject {
    /// False while the bots wait below the orb; true once they have flown in.
    @Published private(set) var flown = !ModeArrivalCenter.shared.teamPending
    @Published private(set) var washOn = false
    private var task: Task<Void, Never>?

    /// Hold every face back, wait for intros, then fly them in.
    func play() {
        task?.cancel()
        flown = false
        washOn = false
        task = Task { [weak self] in
            await ModeIntroGate.shared.waitUntilClear()
            guard !Task.isCancelled, let self else { return }
            self.washOn = true
            self.flown = true
            // The wash is a soft 1.1 s; team landing resets at 1100 ms.
            try? await Task.sleep(nanoseconds: 1_100_000_000)
            guard !Task.isCancelled else { return }
            self.washOn = false
        }
    }
}

/// A roster face that flies up from the orb (or just fades, reduced motion).
private struct TeamFlight: ViewModifier {
    let index: Int
    @ObservedObject var state: TeamArrivalState
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func body(content: Content) -> some View {
        let up = state.flown
        content
            .opacity(up ? 1 : 0)
            .scaleEffect(reduceMotion || up ? 1 : 0.5)
            .offset(y: reduceMotion || up ? 0 : 56)
            .animation(reduceMotion
                       ? .easeOut(duration: 0.25)
                       : PolishTokens.team.delay(PolishTokens.teamDelay(index)),
                       value: up)
            .onChange(of: up) { _, landed in
                guard landed else { return }
                // A light tick as this bot lands.
                let wait = reduceMotion ? 0.1 : PolishTokens.teamDelay(index) + 0.28
                Task { @MainActor in
                    try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000))
                    Haptics.light()
                }
            }
    }
}

extension View {
    func teamFlight(index: Int, state: TeamArrivalState) -> some View {
        modifier(TeamFlight(index: index, state: state))
    }
}

/// A soft wash in all the team's colours behind the roster (1.1 s).
struct TeamArrivalWash: View {
    let hexes: [String]
    let on: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let colors = hexes.isEmpty ? [PolishTokens.coral] : hexes.map { Color(hex: $0) }
        LinearGradient(colors: colors.map { $0.opacity(0.32) }.appendingIfSingle(),
                       startPoint: .leading, endPoint: .trailing)
            .blur(radius: 36)
            .mask(LinearGradient(colors: [.black, .clear], startPoint: .top, endPoint: .bottom))
            .opacity(reduceMotion ? 0 : (on ? 1 : 0))
            .animation(.easeInOut(duration: 1.1 / 2), value: on)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }
}

private extension Array where Element == Color {
    /// A gradient needs two stops.
    func appendingIfSingle() -> [Color] { count == 1 ? self + self : self }
}

// MARK: - Bot arrival

/// The big avatar over a wash in the bot's colour. The caller removes it at
/// the end of the timeline; the avatar settles into its roster face.
struct BotArrivalOverlay: View {
    let bot: AgentRecord
    let hex: String?
    let namespace: Namespace.ID
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    /// 0 wash, 1 big avatar (600 ms), 2 settled (1600 ms).
    @State private var stage = 0

    var body: some View {
        let color = hex.map { Color(hex: $0) } ?? Color("TextPrimary")
        ZStack {
            if !reduceMotion {
                RadialGradient(colors: [color.opacity(hex == nil ? 0.06 : 0.30), .clear],
                               center: .top, startRadius: 0, endRadius: 420)
                    .ignoresSafeArea()
                    .opacity(stage >= 2 ? 0 : 1)
                    .transition(.opacity)
            }
            if stage >= 1 && stage < 2 {
                BotAvatarView(bot: bot, size: 120, live: true)
                    .matchedGeometryEffect(id: bot.id, in: namespace, isSource: false)
                    .transition(reduceMotion ? .opacity : .modifier(active: BlurRise(on: false), identity: BlurRise(on: true)))
                    .padding(.top, 90)
                    .frame(maxHeight: .infinity, alignment: .top)
            }
        }
        .animation(reduceMotion ? .easeOut(duration: 0.25) : PolishTokens.arrivalBig, value: stage)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .task {
            await ModeIntroGate.shared.waitUntilClear()
            // 0 ms: the wash is up. 600 ms: the big avatar. 1600 ms: settle.
            try? await Task.sleep(nanoseconds: 600_000_000)
            guard !Task.isCancelled else { return }
            stage = 1
            Haptics.soft()
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            guard !Task.isCancelled else { return }
            stage = 2
            Haptics.light()
        }
    }
}

/// Rises out of a blur: small, soft and low, then full size and sharp.
private struct BlurRise: ViewModifier {
    let on: Bool
    func body(content: Content) -> some View {
        content
            .opacity(on ? 1 : 0)
            .scaleEffect(on ? 1 : 0.6)
            .offset(y: on ? 0 : 40)
            .blur(radius: on ? 0 : 18)
    }
}
