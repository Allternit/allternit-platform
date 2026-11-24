import SwiftUI
import Combine

// MARK: - Gate (hand-off to the arrivals lane)

extension Notification.Name {
    /// Posted when a mode intro is dismissed with Continue.
    static let modeIntroFinished = Notification.Name("allternit:mode-intro-finished")
}

/// Whether a mode intro is deciding or on screen. The mode arrivals (I13)
/// hold their animation while `isHolding` is true: `await ModeIntroGate.shared.waitUntilClear()`.
@MainActor
final class ModeIntroGate: ObservableObject {
    static let shared = ModeIntroGate()

    /// An intro is being looked up (seen flag) for the mode that just arrived.
    @Published fileprivate(set) var isPending = false
    /// An intro sheet is on screen.
    @Published fileprivate(set) var isShowing = false

    var isHolding: Bool { isPending || isShowing }

    private init() {}

    /// Returns once no intro is pending or showing.
    func waitUntilClear() async {
        while isHolding {
            _ = await Publishers.CombineLatest($isPending, $isShowing)
                .map { $0 || $1 }
                .values
                .first(where: { !$0 })
        }
    }
}

// MARK: - Host

private extension ShellMode {
    var introKey: IntroKey {
        switch self {
        case .home: return .home
        case .bots: return .bots
        case .code: return .code
        case .aci: return .aci
        }
    }
}

/// Shows each mode's first-visit intro (D2): watches the shell mode store, so
/// orb taps, the rail, pills and deep links all count, not only the
/// `allternit:mode-arrive` post. Home shows on the first open. Bots shares
/// `botsIntroSeenAt` with "Meet your bots" and never stacks with it (D4).
struct ModeIntrosHost: ViewModifier {
    @EnvironmentObject private var modeStore: AppModeStore
    @ObservedObject private var gate = ModeIntroGate.shared
    @ObservedObject private var meetBots = BotsIntroStore.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    @State private var current: ShellMode?

    private struct Trigger: Equatable {
        let mode: ShellMode
        let meetBotsPresented: Bool
    }

    func body(content: Content) -> some View {
        content
            .overlay {
                if let mode = current {
                    IntroSheet(intro: mode.introKey, onContinue: { finish(mode) }, hero: { hero(for: mode) })
                        .id(mode)
                }
            }
            .animation(reduceMotion ? .easeOut(duration: 0.2) : PolishTokens.intro, value: current)
            .task(id: Trigger(mode: modeStore.shellMode, meetBotsPresented: meetBots.isPresented)) {
                await evaluate(modeStore.shellMode)
            }
    }

    @MainActor
    private func evaluate(_ mode: ShellMode) async {
        // A different mode arrived, or "Meet your bots" took the screen:
        // drop any intro that is up (it was not dismissed, so it is not seen).
        current = nil
        gate.isShowing = false
        gate.isPending = true
        defer { gate.isPending = false }

        // Never stack with "Meet your bots": wait for its real presented
        // state, not a timer. A later presentation re-runs this task.
        if meetBots.isPresented { return }
        let key = mode.introKey
        guard await IntroSeenStore.shared.shouldShow(key), !Task.isCancelled else { return }
        // The Meet-your-bots check can finish while the lookup above ran.
        if BotsIntroStore.shared.isPresented { return }
        current = mode
        gate.isShowing = true
    }

    private func finish(_ mode: ShellMode) {
        IntroSeenStore.shared.markSeen(mode.introKey)
        current = nil
        gate.isShowing = false
        NotificationCenter.default.post(name: .modeIntroFinished, object: nil,
                                        userInfo: ["mode": mode.hubId])
    }

    @ViewBuilder
    private func hero(for mode: ShellMode) -> some View {
        switch mode {
        case .home: HeroHome()
        case .bots: HeroBots()
        case .code: HeroCode()
        case .aci: HeroAci()
        }
    }
}

extension View {
    /// Mounts the four mode intros over the workspace shell.
    func modeIntros() -> some View { modifier(ModeIntrosHost()) }
}

// MARK: - Heroes (panel B)

private let heroHeight: CGFloat = 200

/// The light hero ground: a quiet fill with the 12 pt dot grid.
private struct HeroDots: View {
    var body: some View {
        ZStack {
            Color(.secondarySystemBackground)
            Canvas { ctx, size in
                var y: CGFloat = 6
                while y < size.height {
                    var x: CGFloat = 6
                    while x < size.width {
                        ctx.fill(Path(ellipseIn: CGRect(x: x, y: y, width: 1.2, height: 1.2)),
                                 with: .color(Color.primary.opacity(0.10)))
                        x += 12
                    }
                    y += 12
                }
            }
        }
    }
}

private struct HeroHome: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var on = false

    var body: some View {
        // 5 x 6 block A mark; ink blocks, coral core (cells 12 and 17).
        let rows = ["..#..", ".###.", "##.##", "#####", "#...#", "#...#"].joined()
        ZStack {
            HeroDots()
            HStack(alignment: .bottom, spacing: 8) {
                LazyVGrid(columns: Array(repeating: GridItem(.fixed(12), spacing: 2), count: 5), spacing: 2) {
                    ForEach(Array(rows.enumerated()), id: \.offset) { i, c in
                        Rectangle()
                            .fill(i == 12 || i == 17 ? PolishTokens.coral : PolishTokens.ink)
                            .frame(width: 12, height: 12)
                            .opacity(c == "#" ? (on ? 1 : 0) : 0)
                            .offset(y: on || reduceMotion ? 0 : -24)
                            .animation(reduceMotion ? nil :
                                .spring(response: 0.3, dampingFraction: 0.55)
                                    .delay(0.1 + Double(i % 5) * 0.05 + Double(i / 5) * 0.06), value: on)
                    }
                }
                .frame(width: 5 * 12 + 4 * 2)
                BlockText("://", height: 24, color: PolishTokens.coral)
                    .opacity(on ? 1 : 0)
                    .offset(x: on || reduceMotion ? 0 : -6)
                    .animation(reduceMotion ? nil : .easeOut(duration: 0.3).delay(0.9), value: on)
            }
        }
        .frame(height: heroHeight)
        .onAppear { on = true }
    }
}

private struct HeroBots: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var on = false
    @State private var dot = false

    private let pts: [CGPoint] = [CGPoint(x: 70, y: 70), CGPoint(x: 250, y: 70), CGPoint(x: 160, y: 150)]
    private let names = ["Scout", "Writer", "Gizzi"]

    var body: some View {
        GeometryReader { geo in
            let sx = geo.size.width / 320
            ZStack {
                HeroDots()
                Path { p in
                    for (a, b) in [(0, 2), (1, 2), (0, 1)] {
                        p.move(to: CGPoint(x: pts[a].x * sx, y: pts[a].y))
                        p.addLine(to: CGPoint(x: pts[b].x * sx, y: pts[b].y))
                    }
                }
                .trim(from: 0, to: on || reduceMotion ? 1 : 0)
                .stroke(Color.primary.opacity(0.35), style: StrokeStyle(lineWidth: 1.5, dash: [3, 5]))
                .animation(reduceMotion ? nil : .easeInOut(duration: 0.8).delay(0.6), value: on)

                if !reduceMotion {
                    Circle().fill(PolishTokens.coral).frame(width: 8, height: 8)
                        .position(x: (dot ? pts[0].x : pts[2].x) * sx, y: dot ? pts[0].y : pts[2].y)
                        .opacity(on ? 1 : 0)
                        .animation(.easeInOut(duration: 1.2).repeatForever(autoreverses: false).delay(1.4), value: dot)
                }

                ForEach(0..<3, id: \.self) { i in
                    let big = i == 2
                    VStack(spacing: 3) {
                        ZStack {
                            Circle().fill(big ? Color.white : PolishTokens.ink)
                            if big {
                                SetupGizzi(size: 50, emotion: .pleased)
                            } else {
                                BlockText(String(names[i].prefix(1)), height: 15, color: .white)
                            }
                        }
                        .frame(width: big ? 60 : 44, height: big ? 60 : 44)
                        .overlay(Circle().stroke(PolishTokens.glassBorder, lineWidth: 1))
                        Text(names[i]).font(.system(size: 10.5)).foregroundStyle(.secondary)
                    }
                    .position(x: pts[i].x * sx, y: pts[i].y + 8)
                    .scaleEffect(on || reduceMotion ? 1 : 0)
                    .opacity(on || reduceMotion ? 1 : 0)
                    .animation(reduceMotion ? nil : .spring(response: 0.35, dampingFraction: 0.6).delay(0.1 + Double(i) * 0.15), value: on)
                }
            }
        }
        .frame(height: heroHeight)
        .onAppear { on = true; dot = true }
    }
}

private struct HeroCode: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let task = "add guest checkout"
    private let lines = ["✓ read 4 files", "✓ wrote checkout.tsx", "✓ 12 tests pass"]
    @State private var typed = 0
    @State private var shown = 0

    var body: some View {
        let progress = CGFloat(typed) / CGFloat(task.count) * 0.4 + CGFloat(shown) / CGFloat(lines.count) * 0.6
        ZStack(alignment: .topLeading) {
            Color(hex: "#18181A")
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 10) {
                    SetupGizzi(size: 30, emotion: .focused)
                    BlockText("GIZZI CODE", height: 12, color: Color(hex: "#EDEDED"))
                    Spacer()
                    Text("MacBook Pro").font(.system(size: 10)).foregroundStyle(Color(hex: "#8A8A8E"))
                }
                .padding(.bottom, 16)
                (Text("❯ ").foregroundColor(PolishTokens.coral)
                 + Text(String(task.prefix(typed))).foregroundColor(Color(hex: "#EDEDED"))
                 + Text(typed < task.count ? "▍" : "").foregroundColor(PolishTokens.coral))
                    .font(.system(size: 12.5, design: .monospaced))
                ForEach(Array(lines.prefix(shown).enumerated()), id: \.offset) { _, l in
                    Text(l).font(.system(size: 12.5, design: .monospaced))
                        .foregroundColor(Color(hex: "#7FD48A"))
                        .padding(.top, 7)
                        .transition(.opacity.combined(with: .offset(x: -6)))
                }
            }
            .padding(EdgeInsets(top: 16, leading: 18, bottom: 16, trailing: 18))
            VStack {
                Spacer()
                GeometryReader { g in
                    Rectangle()
                        .fill(LinearGradient(colors: [Color(hex: "#D97757"), Color(hex: "#C9A27E")], startPoint: .leading, endPoint: .trailing))
                        .frame(width: g.size.width * progress, height: 3)
                }
                .frame(height: 3)
            }
        }
        .frame(height: heroHeight)
        .clipShape(RoundedRectangle(cornerRadius: 24, style: .continuous))
        .animation(.easeOut(duration: 0.3), value: shown)
        .task {
            if reduceMotion { typed = task.count; shown = lines.count; return }
            while typed < task.count {
                try? await Task.sleep(nanoseconds: 45_000_000)
                if Task.isCancelled { return }
                typed += 1
            }
            while shown < lines.count {
                try? await Task.sleep(nanoseconds: 420_000_000)
                if Task.isCancelled { return }
                shown += 1
            }
        }
    }
}

private struct HeroAci: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var clicked = false
    @State private var cursor = false

    var body: some View {
        ZStack(alignment: .topLeading) {
            HeroDots()
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Image(systemName: "lock.fill").font(.system(size: 9))
                    Text("shop.example.com").font(.system(size: 11))
                }
                .foregroundStyle(.secondary)
                .padding(.horizontal, 10).padding(.vertical, 7)
                Divider()
                HStack(alignment: .top, spacing: 12) {
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(Color(hex: "#EFE7FB")).frame(width: 70, height: 70)
                    VStack(alignment: .leading, spacing: 7) {
                        Capsule().fill(Color.primary.opacity(0.12)).frame(width: 90, height: 8)
                        Capsule().fill(Color.primary.opacity(0.08)).frame(width: 58, height: 8)
                        HStack(spacing: 5) {
                            if clicked { Image(systemName: "checkmark").font(.system(size: 10, weight: .bold)) }
                            Text(clicked ? "Added" : "Add to cart").font(.system(size: 11.5, weight: .semibold))
                        }
                        .foregroundStyle(.white)
                        .padding(.horizontal, 12).padding(.vertical, 6)
                        .background(Capsule().fill(clicked ? Color(hex: "#16A34A") : PolishTokens.ink))
                                        .padding(.top, 5)
                    }
                    Spacer(minLength: 0)
                }
                .padding(12)
                Spacer(minLength: 0)
            }
            .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color(.systemBackground)))
            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(PolishTokens.glassBorder, lineWidth: 1))
            .padding(14)

            Image(systemName: "cursorarrow")
                .font(.system(size: 20))
                .foregroundStyle(PolishTokens.coral)
                .offset(x: cursor || reduceMotion ? 188 : 260, y: cursor || reduceMotion ? 96 : 170)
                .animation(reduceMotion ? nil : .timingCurve(0.3, 0.7, 0.3, 1, duration: 0.9).delay(0.7), value: cursor)
        }
        .frame(height: heroHeight)
        .task {
            if reduceMotion { cursor = true; clicked = true; return }
            cursor = true
            try? await Task.sleep(nanoseconds: 1_700_000_000)
            if Task.isCancelled { return }
            withAnimation(.easeInOut(duration: 0.3)) { clicked = true }
        }
    }
}
