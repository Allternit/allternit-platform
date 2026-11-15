import SwiftUI
import UIKit

/// The auto-looping preview of the four real Allternit views (Home, Bots,
/// Code, ACI) on top of the sign-in screen — the native port of the web
/// `SignInStage` with `variant="phone"` that m.allternit.com shows
/// (allternit-ai src/components/auth/sign-in-stage/SignInStage.tsx).
///
/// Each scene builds itself when it comes on screen (beat sheet in
/// SignInStageMotion / SignInStageScenes). Scenes advance every 4.6 s, on a
/// tap on the stage, from the ticks, or a sideways swipe. The dwell pauses
/// while the app is not active. Reduce Motion shows every scene finished and
/// never advances on its own. Nothing here calls an API, so it is safe
/// signed out.
struct SignInStageView: View {
    /// How long each scene stays before the next one.
    static let dwell: TimeInterval = 4.6
    /// A horizontal drag longer than this is a swipe.
    private static let swipeDistance: CGFloat = 40

    struct Slide: Identifiable {
        enum ID { case home, bots, code, aci }
        let id: ID
        let name: String
        /// Block-type word after A://TERNIT in the lockup ("" for Home).
        let suffix: String
        let title: String
        let body: String
    }

    static let slides: [Slide] = [
        Slide(id: .home, name: "Home", suffix: "",
              title: "Ask once. Gizzi gets to work.",
              body: "One composer for every brain. Replies, research and documents land in one thread."),
        Slide(id: .bots, name: "Bots", suffix: "BOTS",
              title: "Your team, assembled.",
              body: "Gizzi brings in Drop, Pixel, a ChatGPT dot, Grok and Muse. Each takes a thread and checks with you before it acts."),
        Slide(id: .code, name: "Code", suffix: "CODE",
              title: "Code with the proof attached.",
              body: "gizzi code edits your files, runs the tests and shows you the diff."),
        Slide(id: .aci, name: "ACI", suffix: "ACI",
              title: "Agents with a real computer.",
              body: "Allternit Explorer browses and acts for you. Take over any time."),
    ]

    private struct Position: Equatable {
        var slide = 0
        var prev = -1
        var forward = true
        /// Bumps on every change so a re-entered slide restarts its build.
        var seq = 0
        var changedAt = Date()
        var prevChangedAt = Date()
    }

    private struct DwellKey: Equatable {
        let seq: Int
        let running: Bool
    }

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.colorScheme) private var colorScheme

    @State private var pos = Position()
    /// Dwell left on the current slide, and when the running stretch began.
    @State private var dwellLeft: TimeInterval = 4.6
    @State private var dwellStartedAt: Date?

    private var palette: SignInPalette { SignInPalette(colorScheme) }
    private var running: Bool { !reduceMotion && scenePhase == .active }
    private var count: Int { Self.slides.count }

    var body: some View {
        TimelineView(.animation(minimumInterval: nil, paused: !running)) { timeline in
            let now = timeline.date
            VStack(spacing: 0) {
                header(now: now)
                stage(now: now)
                ticks(now: now)
                captions(now: now)
            }
        }
        .task(id: DwellKey(seq: pos.seq, running: running)) {
            await runDwell()
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("What you can do in Allternit")
    }

    // MARK: Header

    private func header(now: Date) -> some View {
        let current = Self.slides[pos.slide]
        let t = sceneTime(pos.changedAt, now)
        let pop = StageFrame.of(.pop, at: t, delay: 0.2)
        return HStack(spacing: 7) {
            WordmarkView(height: 11)
            if !current.suffix.isEmpty {
                PixelWord(text: current.suffix, fill: palette.ink, height: 11)
                    .scaleEffect(pop.scaleX)
                    .offset(y: pop.dy)
                    .opacity(pop.opacity)
                    .id(pos.seq)
            }
            Spacer(minLength: 0)
            Text("\(Text(String(format: "%02d", pos.slide + 1)).foregroundStyle(palette.ink)) / \(String(format: "%02d", count))")
                .foregroundStyle(palette.sub)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .tracking(0.66)
                .accessibilityLabel("Preview \(pos.slide + 1) of \(count)")
        }
        .frame(height: 22)
        .padding(.bottom, 8)
        .accessibilityElement(children: .combine)
    }

    // MARK: Stage card

    private func stage(now: Date) -> some View {
        GeometryReader { proxy in
            let k = max(0.3, proxy.size.width / StageScene.width)
            ZStack(alignment: .topLeading) {
                fieldPattern
                // Plinth: the floor glow under the window.
                Ellipse()
                    .fill(RadialGradient(
                        stops: [
                            .init(color: palette.stageBottom, location: 0),
                            .init(color: palette.stageBottom.opacity(0.9), location: 0.4),
                            .init(color: palette.stageBottom.opacity(0), location: 0.72),
                        ],
                        center: UnitPoint(x: 0.5, y: 0.4),
                        startRadius: 0,
                        endRadius: proxy.size.width * 0.375
                    ))
                    .frame(width: proxy.size.width * 0.75, height: 150 * k)
                    .offset(x: proxy.size.width * 0.125, y: proxy.size.height - 92 * k)
                ForEach(slideLayers(now: now), id: \.key) { layer in
                    sceneView(layer.index)
                        .environment(\.stageTime, layer.time)
                        .environment(\.stageStill, reduceMotion)
                        .environment(\.colorScheme, .light)
                        .frame(width: StageScene.width, height: StageScene.height, alignment: .topLeading)
                        .offset(x: layer.dx)
                        .blur(radius: layer.blur)
                        .opacity(layer.opacity)
                        .zIndex(layer.isCurrent ? 2 : 1)
                        .scaleEffect(k, anchor: .topLeading)
                }
            }
            .frame(width: proxy.size.width, height: proxy.size.height, alignment: .topLeading)
        }
        .aspectRatio(StageScene.width / StageScene.height, contentMode: .fit)
        .background(
            LinearGradient(
                stops: [.init(color: palette.stageTop, location: 0), .init(color: palette.stageBottom, location: 0.7)],
                startPoint: .top,
                endPoint: .bottom
            )
        )
        .clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(palette.line, lineWidth: 1))
        .contentShape(RoundedRectangle(cornerRadius: 18, style: .continuous))
        .onTapGesture { next() }
        .simultaneousGesture(
            DragGesture(minimumDistance: 12)
                .onEnded { value in
                    let dx = value.translation.width
                    let dy = value.translation.height
                    guard abs(dx) >= Self.swipeDistance, abs(dx) >= abs(dy) else { return }
                    if dx < 0 { next() } else { prev() }
                }
        )
        .accessibilityElement()
        .accessibilityLabel("\(pos.slide + 1) of \(count): \(Self.slides[pos.slide].name)")
        .accessibilityHint("Shows the next preview")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { next() }
        .accessibilityAdjustableAction { direction in
            switch direction {
            case .increment: next()
            case .decrement: prev()
            @unknown default: break
            }
        }
    }

    /// Rounded-square field behind the window (22 pt grid, 18 pt blocks).
    private var fieldPattern: some View {
        Canvas { context, size in
            var y: CGFloat = 2
            while y < size.height {
                var x: CGFloat = 2
                while x < size.width {
                    let r = CGRect(x: x, y: y, width: 18, height: 18)
                    context.fill(Path(roundedRect: r, cornerRadius: 3.2), with: .color(palette.field))
                    x += 22
                }
                y += 22
            }
        }
        .opacity(0.55)
        .allowsHitTesting(false)
    }

    @ViewBuilder
    private func sceneView(_ index: Int) -> some View {
        switch Self.slides[index].id {
        case .home: StageHomeScene()
        case .bots: StageBotsScene()
        case .code: StageCodeScene()
        case .aci: StageAciScene()
        }
    }

    private struct Layer {
        let key: String
        let index: Int
        let isCurrent: Bool
        let time: Double
        let dx: CGFloat
        let blur: CGFloat
        let opacity: Double
    }

    /// The current slide pushing in, and the previous one leaving while it
    /// keeps its own frame (web .ast-sl.cur / .ast-sl.out). The push is in the
    /// layer's own units: canvas points for scenes, points for captions.
    private func slideLayers(now: Date, push: CGFloat = 48, leave: CGFloat = 36) -> [Layer] {
        let sign: CGFloat = pos.forward ? 1 : -1
        let since = sceneTime(pos.changedAt, now)
        var layers: [Layer] = []
        if !reduceMotion, pos.prev >= 0 {
            let p = StageMotion.progress(since, delay: 0, duration: 0.32)
            if p < 1 {
                let e = StageCurve.leave(p)
                layers.append(Layer(
                    key: "\(pos.prev)-\(pos.seq - 1)",
                    index: pos.prev,
                    isCurrent: false,
                    time: now.timeIntervalSince(pos.prevChangedAt),
                    dx: -sign * leave * e,
                    blur: 6 * e,
                    opacity: 1 - e
                ))
            }
        }
        let e = reduceMotion ? 1 : StageCurve.push(StageMotion.progress(since, delay: 0, duration: 0.45))
        layers.append(Layer(
            key: "\(pos.slide)-\(pos.seq)",
            index: pos.slide,
            isCurrent: true,
            time: reduceMotion ? StageMotion.finished : since,
            dx: sign * push * (1 - e),
            blur: 0,
            opacity: StageMotion.clamp(e)
        ))
        return layers
    }

    // MARK: Ticks

    private func ticks(now: Date) -> some View {
        HStack(spacing: 0) {
            ForEach(Self.slides.indices, id: \.self) { i in
                Button {
                    go(to: i, forward: i > pos.slide)
                } label: {
                    tick(i, now: now)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Preview \(i + 1) of \(count): \(Self.slides[i].name)")
                .accessibilityAddTraits(i == pos.slide ? .isSelected : [])
            }
        }
        .frame(height: 24)
        .padding(.top, 6)
    }

    private func tick(_ i: Int, now: Date) -> some View {
        let on = i == pos.slide
        return ZStack(alignment: .bottom) {
            RoundedRectangle(cornerRadius: 2)
                .fill(on ? palette.tickOn : palette.tick)
                .frame(width: 3, height: on ? 16 : 9)
            if on && !reduceMotion {
                RoundedRectangle(cornerRadius: 2)
                    .fill(palette.ink)
                    .frame(width: 3, height: 16)
                    .scaleEffect(x: 1, y: 0.15 + 0.85 * dwellProgress(now), anchor: .bottom)
            }
        }
        .padding(.bottom, 3)
        .frame(width: 16, height: 24, alignment: .bottom)
        .contentShape(Rectangle())
        .animation(reduceMotion ? nil : .timingCurve(0.2, 0.8, 0.2, 1, duration: 0.25), value: pos.slide)
    }

    private func dwellProgress(_ now: Date) -> Double {
        var used = Self.dwell - dwellLeft
        if running, let start = dwellStartedAt {
            used += now.timeIntervalSince(start)
        }
        return StageMotion.clamp(used / Self.dwell)
    }

    // MARK: Captions

    private func captions(now: Date) -> some View {
        ZStack(alignment: .top) {
            ForEach(slideLayers(now: now), id: \.key) { layer in
                caption(Self.slides[layer.index], time: layer.time)
                    .offset(x: layer.dx)
                    .blur(radius: layer.blur)
                    .opacity(layer.opacity)
                    .zIndex(layer.isCurrent ? 2 : 1)
                    .accessibilityHidden(!layer.isCurrent)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 96, maxHeight: 96, alignment: .top)
        .clipped()
    }

    private func caption(_ slide: Slide, time t: Double) -> some View {
        // The headline inks grey → black; the body pops in after it.
        let ink = SignInPalette.blend(palette.inkFrom, palette.ink, StageCurve.easeOut(StageMotion.progress(t, delay: 0.45, duration: 0.6)))
        let bodyBeat = StageFrame.of(.caption, at: t, delay: 0.6)
        return VStack(spacing: 5) {
            Text(slide.title)
                .font(.system(size: 22, weight: .bold))
                .tracking(-0.66)
                .foregroundStyle(ink)
                .multilineTextAlignment(.center)
                .padding(.top, 6)
                .accessibilityAddTraits(.isHeader)
            Text(slide.body)
                .font(.system(size: 12.5))
                .lineSpacing(3)
                .foregroundStyle(palette.sub)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 300)
                .scaleEffect(bodyBeat.scaleX)
                .offset(y: bodyBeat.dy)
                .opacity(bodyBeat.opacity)
        }
        .padding(.horizontal, 8)
        .frame(maxWidth: .infinity)
    }

    // MARK: Navigation and dwell

    private func sceneTime(_ start: Date, _ now: Date) -> Double {
        reduceMotion ? StageMotion.finished : max(0, now.timeIntervalSince(start))
    }

    private func next() {
        go(to: pos.slide + 1, forward: true)
    }

    private func prev() {
        go(to: pos.slide - 1, forward: false)
    }

    private func go(to target: Int, forward: Bool) {
        let nextIndex = ((target % count) + count) % count
        guard nextIndex != pos.slide else { return }
        let now = Date()
        dwellLeft = Self.dwell
        dwellStartedAt = nil
        pos = Position(
            slide: nextIndex,
            prev: pos.slide,
            forward: forward,
            seq: pos.seq + 1,
            changedAt: now,
            prevChangedAt: pos.changedAt
        )
    }

    /// Runs the dwell for the current slide; when cancelled (paused, or the
    /// person moved the stage) it keeps the time that was left.
    private func runDwell() async {
        guard running else { return }
        let seq = pos.seq
        let start = Date()
        dwellStartedAt = start
        do {
            try await Task.sleep(for: .seconds(dwellLeft))
        } catch {
            // Only bank the elapsed time if this is still the same slide.
            if pos.seq == seq {
                dwellLeft = max(0, dwellLeft - Date().timeIntervalSince(start))
                dwellStartedAt = nil
            }
            return
        }
        guard pos.seq == seq else { return }
        next()
    }
}

/// Sign-in page colours (web .ast-root tokens; `tone="auto"`: dark frame and
/// type in dark mode, the scenes stay light).
struct SignInPalette {
    let ink: Color
    let inkFrom: Color
    let sub: Color
    let mut: Color
    let line: Color
    let fill: Color
    let page: Color
    let raise: Color
    let stageTop: Color
    let stageBottom: Color
    let field: Color
    let tick: Color
    let tickOn: Color

    init(_ scheme: ColorScheme) {
        if scheme == .dark {
            ink = Color(hex: "#F0EEE6")
            inkFrom = Color(hex: "#52525B")
            sub = Color(hex: "#A1A1AA")
            mut = Color(hex: "#8A8A93")
            line = Color(hex: "#2E2E33")
            fill = Color(hex: "#222225")
            page = Color(hex: "#141413")
            raise = Color(hex: "#1C1C1E")
            stageTop = Color(hex: "#1C1C1E")
            stageBottom = Color(hex: "#141413")
            field = Color(red: 240 / 255, green: 238 / 255, blue: 230 / 255, opacity: 0.06)
            tick = Color(hex: "#3A3A3F")
            tickOn = Color(hex: "#2E2E33")
        } else {
            ink = Color(hex: "#141413")
            inkFrom = Color(hex: "#C9C9CF")
            sub = Color(hex: "#5E5E66")
            mut = Color(hex: "#8A8A93")
            line = Color(hex: "#E4E4E7")
            fill = Color(hex: "#F4F4F5")
            page = Color.white
            raise = Color.white
            stageTop = Color(hex: "#F7F7F9")
            stageBottom = Color.white
            field = Color(red: 20 / 255, green: 20 / 255, blue: 19 / 255, opacity: 0.05)
            tick = Color(hex: "#D4D4D8")
            tickOn = Color(hex: "#E4E4E7")
        }
    }

    /// `Color.mix` is iOS 18+; the app still runs on iOS 17.
    static func blend(_ from: Color, _ to: Color, _ amount: Double) -> Color {
        var (r1, g1, b1, a1): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        var (r2, g2, b2, a2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        UIColor(from).getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        UIColor(to).getRed(&r2, green: &g2, blue: &b2, alpha: &a2)
        let k = CGFloat(min(max(amount, 0), 1))
        return Color(red: r1 + (r2 - r1) * k, green: g1 + (g2 - g1) * k, blue: b1 + (b2 - b1) * k, opacity: a1 + (a2 - a1) * k)
    }
}
