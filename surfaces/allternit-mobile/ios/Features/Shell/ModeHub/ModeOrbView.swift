import SwiftUI

/// Item frames of the open pill, in the orb's own coordinate space.
private struct ModeItemFrames: PreferenceKey {
    static let defaultValue: [ShellMode: CGRect] = [:]
    static func reduce(value: inout [ShellMode: CGRect], nextValue: () -> [ShellMode: CGRect]) {
        value.merge(nextValue()) { $1 }
    }
}

/// Mode orb (contract "Mode orb" + "Timings and motion").
/// Rest: a 36 pt interactive glass orb with the current mode's icon. Tap: it
/// opens into a glass pill of the four modes (light haptic). Hold 450 ms
/// (medium haptic): opens the hub sheet via `onHold`; with no `onHold` the orb
/// is tap-only (the drawer's switcher).
/// Liquid lens (2026-11-26, Eoj OK, Option B): press the orb and slide, and the
/// pill opens under the finger; one clear glass lens swells, follows the finger
/// 1:1, stretches with speed and magnifies the item under it; lifting snaps it to
/// the nearest mode with a jelly settle (medium haptic) and the pill folds back
/// after `pickSettle`. One drag gesture on the container is the only touch path
/// (the orb node is removed when the pill opens, so a gesture on it would die).
/// Reduce Motion: states change at once, no springs, no swell, stretch or magnify.
struct ModeOrbView: View {
    let mode: ShellMode
    /// Orb diameter (the drawer matches its 40 pt glass buttons).
    var size: CGFloat = 36
    /// A mode was picked in the pill.
    var onSelect: (ShellMode) -> Void
    /// The hold completed: open the hub sheet. nil = no hold (tap-only orb).
    var onHold: (() -> Void)?
    /// The pill opened (true) or folded back to the orb (false).
    var onOpenChange: ((Bool) -> Void)? = nil

    @ReducedMotion private var reduced
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.colorScheme) private var scheme
    @ObservedObject private var botStore = BotModeStore.shared
    @State private var open = false
    /// The mode under the finger (or under the lens at rest); nil = the current mode.
    @State private var hover: ShellMode?
    @State private var pressing = false
    @State private var collapseTask: Task<Void, Never>?
    @Namespace private var ns

    // Gesture. `touching` is a GestureState so a cancelled touch (sheet presenting,
    // drawer drag, keyboard tap recognizer) always resets and can never leave the orb stuck.
    @GestureState private var touching = false
    private enum Origin { case orb, pill }
    @State private var active = false
    @State private var origin: Origin = .orb
    @State private var slid = false
    @State private var held = false
    @State private var settling = false
    @State private var lastX: CGFloat = 0
    @State private var lastSample: (x: CGFloat, t: Date)?
    @State private var holdTask: Task<Void, Never>?
    @State private var settleTask: Task<Void, Never>?
    @State private var stretchTask: Task<Void, Never>?

    // Lens
    @State private var itemFrames: [ShellMode: CGRect] = [:]
    @State private var lensLeft: CGFloat = 0
    @State private var lensW: CGFloat = 0
    @State private var lensSwell: CGFloat = 1
    @State private var lensStretch: CGFloat = 1
    @State private var lensReady = false

    private static let space = "modeOrbPill"

    /// In Bots mode, the bot whose page or chat is open; its face replaces the icon.
    private var focusedBot: AgentRecord? {
        guard mode == .bots, let id = botStore.focusedBotId else { return nil }
        return botStore.bots.first { $0.id == id }
    }

    /// The Bots slot: the bot's round face while one is open, else the plain icon.
    /// Swaps with a crossfade and scale on the polish spring; instantly under Reduce Motion.
    @ViewBuilder
    private func slotGlyph(_ m: ShellMode, size: CGFloat, weight: Font.Weight) -> some View {
        ZStack {
            if m == .bots, let bot = focusedBot {
                BotAvatarView(bot: bot, size: size + 6)
                    .frame(width: size + 6, height: size + 6)
                    .clipShape(Circle())
                    .background(Circle().fill(Color("FillNeutral")))
                    .id("bot-\(bot.id)")
                    .transition(reduced ? .identity : .scale(scale: 0.6).combined(with: .opacity))
                    .accessibilityHidden(true)
            } else {
                Image(systemName: m.icon)
                    .font(.system(size: size, weight: weight))
                    .id("icon-\(m.rawValue)")
                    .transition(reduced ? .identity : .scale(scale: 0.6).combined(with: .opacity))
            }
        }
        .animation(reduced ? nil : PolishTokens.base, value: m == .bots ? focusedBot?.id : nil)
    }

    var body: some View {
        GlassContainer(spacing: 12) {
            HStack(spacing: 2) {
                if open {
                    ForEach(ShellMode.allCases, id: \.self) { m in pillItem(m) }
                } else {
                    orb
                }
            }
            .padding(open ? 4 : 0)
            .frame(minWidth: size, minHeight: size)
            .overlay(alignment: .topLeading) { lensOverlay }
            .coordinateSpace(name: Self.space)
            .onPreferenceChange(ModeItemFrames.self) { frames in
                itemFrames = frames
                syncLens()
            }
            .contentShape(Capsule())
            .gesture(drag)
            .polishGlass(cornerRadius: 999, interactive: true)
            .scaleEffect(pressing && !open && !reduced ? ModeHubSpec.holdPressScale : 1)
            .animation(reduced ? nil : ModeHubSpec.pillSpring, value: open)
            .animation(reduced ? nil : ModeHubSpec.swellSpring, value: pressing)
        }
        .onChange(of: touching) { _, isDown in
            // A GestureState resets to false on a normal lift too; `active` is already
            // false by then (onEnded ran), so only a cancelled touch gets through.
            if !isDown { Task { @MainActor in if active { cancelTouch() } } }
        }
        .onDisappear {
            cancelAllTasks()
            pressing = false
            active = false
        }
        .accessibilityElement(children: .contain)
        .accessibilityAction(.escape) { collapse() }
    }

    // MARK: Rest orb

    private var orb: some View {
        holdable(
            slotGlyph(mode, size: 16, weight: .semibold)
                .foregroundStyle(mode.accent)
                .frame(width: size, height: size)
                .contentShape(Circle())
                .id(mode)
                .transition(reduced ? .identity : .scale(scale: 0.5).combined(with: .opacity))
                // Tap, hold and slide all come from the container drag (see `drag`);
                // VoiceOver uses the actions below.
                .accessibilityElement()
                .accessibilityLabel(focusedBot.map { "Bots, in chat with \($0.botDisplayName)" } ?? "Mode: \(mode.label)")
                .accessibilityHint(onHold == nil ? "Tap to switch mode" : "Tap to switch mode, hold for all modes")
                .accessibilityAddTraits(.isButton)
                .accessibilityAction { expand() }
        )
    }

    /// The VoiceOver path to the hub sheet, only when the host gives `onHold`.
    @ViewBuilder
    private func holdable(_ glyph: some View) -> some View {
        if let onHold {
            glyph.accessibilityAction(named: "Open all modes") { ModeHubSpec.fire(.holdComplete); onHold() }
        } else {
            glyph
        }
    }

    // MARK: Pill

    private func pillItem(_ m: ShellMode) -> some View {
        let current = m == mode
        let magnified = slid && !reduced && hover == m
        return HStack(spacing: 6) {
            slotGlyph(m, size: 14, weight: current ? .bold : .regular)
            Text(m.hubLabel)
                .font(.system(size: 12.5, weight: current ? .semibold : .medium))
                .lineLimit(1)
                // The drawer header is narrower than the composer row.
                .minimumScaleFactor(0.8)
        }
        .foregroundStyle(current || m == hover ? Color.primary : Color.secondary)
        .padding(.horizontal, 10)
        .frame(height: 30)
        .background(
            GeometryReader { g in
                Color.clear.preference(key: ModeItemFrames.self, value: [m: g.frame(in: .named(Self.space))])
            }
        )
        .scaleEffect(magnified ? ModeHubSpec.lensMagnifyScale : 1)
        .animation(reduced ? nil : ModeHubSpec.lensSpring, value: magnified)
        .contentShape(Capsule())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(m == .bots ? (focusedBot.map { "Bots, in chat with \($0.botDisplayName)" } ?? m.label) : m.label)
        .accessibilityAddTraits(current ? [.isButton, .isSelected] : [.isButton])
        .accessibilityAction { pick(m) }
        .transition(reduced ? .identity : .scale(scale: 0.8).combined(with: .opacity))
    }

    // MARK: Lens

    private var framesReady: Bool { itemFrames.count == ShellMode.allCases.count }

    private var nativeLens: Bool {
        ModeHubSpec.lensUsesNativeGlass && !reduceTransparency && NativeGlass.available
    }

    @ViewBuilder
    private var lensOverlay: some View {
        if open, lensReady, framesReady, let top = itemFrames.values.map(\.minY).min(), let h = itemFrames.values.first?.height {
            let stretch = reduced ? 1 : lensStretch
            let swell = reduced ? 1 : lensSwell
            lensBody
                .frame(width: lensW, height: h)
                .scaleEffect(x: swell * stretch,
                             y: swell * (1 - (stretch - 1) * ModeHubSpec.lensStretchSquash))
                .offset(x: lensLeft, y: top)
                .allowsHitTesting(false)
                .accessibilityHidden(true)
        }
    }

    /// Clear glass: native `Glass.clear` on iOS 26 (behind `lensUsesNativeGlass`), else a
    /// material lens. Both carry the white rim and the specular highlight.
    private var lensBody: some View {
        let dark = scheme == .dark
        return ZStack {
            if nativeLens {
                Color.clear.clearGlassLens(in: Capsule(), id: "lens", namespace: ns)
            } else {
                Capsule().fill(.ultraThinMaterial)
                Capsule().fill(Color("FillNeutral"))
                Capsule().fill(Color.white.opacity(dark ? 0.10 : 0.22))
            }
            Capsule().fill(
                RadialGradient(colors: [Color.white.opacity(0.55), .clear],
                               center: UnitPoint(x: 0.28, y: 0.12), startRadius: 0, endRadius: 22)
            )
            Capsule().strokeBorder(Color.white.opacity(dark ? 0.30 : 0.75), lineWidth: 1)
        }
        .shadow(color: (dark ? Color.black.opacity(0.35) : Color.black.opacity(0.14)), radius: 9, x: 0, y: 6)
    }

    private func noAnimation(_ body: () -> Void) {
        var t = Transaction()
        t.disablesAnimations = true
        withTransaction(t, body)
    }

    private func nearestMode(_ x: CGFloat) -> ShellMode? {
        func distance(_ r: CGRect) -> CGFloat { x < r.minX ? r.minX - x : (x > r.maxX ? x - r.maxX : 0) }
        return itemFrames.min { distance($0.value) < distance($1.value) }?.key
    }

    /// Puts the lens under x (free follow, clamped past the ends), or on the item's own frame
    /// under Reduce Motion.
    private func placeLens(x: CGFloat, animated: Bool) {
        guard framesReady, let target = nearestMode(x), let f = itemFrames[target],
              let first = itemFrames.values.map(\.minX).min(),
              let last = itemFrames.values.map(\.maxX).max() else { return }
        let w = f.width
        let left = reduced
            ? f.minX
            : min(max(x - w / 2, first - ModeHubSpec.lensOvershoot), last - w + ModeHubSpec.lensOvershoot)
        if animated && !reduced {
            withAnimation(ModeHubSpec.followSpring) { lensLeft = left; lensW = w }
        } else {
            noAnimation { lensLeft = left; lensW = w }
        }
    }

    /// Frames changed (the pill opened or relaid out): keep the lens on the finger or on the mode.
    private func syncLens() {
        guard open, framesReady else { return }
        if slid {
            placeLens(x: lastX, animated: lensReady)
        } else if !settling, let f = itemFrames[hover ?? mode] {
            noAnimation { lensLeft = f.minX; lensW = f.width }
        }
        if !lensReady { noAnimation { lensReady = true } }
    }

    // MARK: Gesture

    private var drag: some Gesture {
        DragGesture(minimumDistance: 0, coordinateSpace: .named(Self.space))
            .updating($touching) { _, state, _ in state = true }
            .onChanged { dragChanged($0) }
            .onEnded { dragEnded($0) }
    }

    private func dragChanged(_ v: DragGesture.Value) {
        let x = v.location.x
        if !active { begin(v) }
        guard !held else { return }
        let dx = v.translation.width, dy = v.translation.height
        if !slid {
            if origin == .orb {
                if abs(dx) > ModeHubSpec.slideStart { startScrub(at: x, haptic: true) }
                else if hypot(dx, dy) > ModeHubSpec.slideStart { holdTask?.cancel() }
            }
            if !slid { return }
        }
        trackVelocity(x: x, at: v.time)
        moveScrub(to: x)
    }

    private func begin(_ v: DragGesture.Value) {
        active = true
        slid = false
        held = false
        lastSample = nil
        cancelAllTasks()
        settling = false
        origin = open ? .pill : .orb
        if open {
            startScrub(at: v.location.x, haptic: false)
        } else {
            pressing = true
            Haptics.prepare()
            armHold()
        }
    }

    /// The 450 ms hold, only for an orb-origin press with an `onHold` host. Cancelled when
    /// the finger travels past `slideStart`, on lift and on reset.
    private func armHold() {
        guard onHold != nil else { return }
        holdTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: UInt64(ModeHubSpec.holdDuration * 1_000_000_000))
            guard !Task.isCancelled, active, !slid else { return }
            held = true
            pressing = false
            ModeHubSpec.fire(.holdComplete)
            onHold?()
        }
    }

    private func startScrub(at x: CGFloat, haptic: Bool) {
        holdTask?.cancel()
        slid = true
        lastX = x
        pressing = false
        if !open {
            withOptionalAnimation { open = true }
            onOpenChange?(true)
        }
        hover = nearestMode(x) ?? mode
        if haptic { ModeHubSpec.fire(.orbTap) }
        if !reduced { withAnimation(ModeHubSpec.swellSpring) { lensSwell = ModeHubSpec.lensPressScale } }
        syncLens()
    }

    private func moveScrub(to x: CGFloat) {
        lastX = x
        if let t = nearestMode(x), t != hover {
            hover = t
            ModeHubSpec.fire(.lensMove)
        }
        placeLens(x: x, animated: lensReady)
    }

    /// Speed from the translation deltas (DragGesture.Value.velocity needs iOS 18; the target
    /// is 17). Stretch eases to the speed and settles back when the finger stops.
    private func trackVelocity(x: CGFloat, at time: Date) {
        guard !reduced else { return }
        if let s = lastSample {
            let dt = time.timeIntervalSince(s.t)
            if dt > 0.001 {
                let speed = abs(x - s.x) / CGFloat(dt)
                let target = 1 + min(speed / ModeHubSpec.lensStretchVelocity, ModeHubSpec.lensStretchMax)
                withAnimation(ModeHubSpec.stretchSpring) { lensStretch = target }
            }
        }
        lastSample = (x, time)
        stretchTask?.cancel()
        stretchTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 90_000_000)
            guard !Task.isCancelled else { return }
            withAnimation(ModeHubSpec.stretchSpring) { lensStretch = 1 }
        }
    }

    private func dragEnded(_ v: DragGesture.Value) {
        guard active else { return }
        active = false
        holdTask?.cancel()
        stretchTask?.cancel()
        lastSample = nil
        pressing = false
        if held {
            held = false
            return
        }
        if slid {
            lift(at: v.location.x)
        } else if origin == .orb, abs(v.translation.height) <= 24 {
            expand()
        }
    }

    /// Lift on the pill: snap to the nearest mode on the lens spring, size settles on the jelly
    /// spring, pick now, fold back after `pickSettle` so the snap is seen.
    private func lift(at x: CGFloat) {
        let m = nearestMode(x) ?? hover ?? mode
        hover = m
        settling = true
        slid = false
        if let f = itemFrames[m] {
            if reduced {
                noAnimation { lensLeft = f.minX; lensW = f.width }
            } else {
                withAnimation(ModeHubSpec.lensSpring) { lensLeft = f.minX; lensW = f.width }
            }
        }
        if !reduced {
            withAnimation(ModeHubSpec.jellySpring) { lensSwell = 1 }
            withAnimation(ModeHubSpec.stretchSpring) { lensStretch = 1 }
        }
        ModeHubSpec.fire(.openItem)
        if m != mode { onSelect(m) }
        collapseTask?.cancel()
        if reduced {
            collapse()
        } else {
            settleTask = Task { @MainActor in
                try? await Task.sleep(nanoseconds: UInt64(ModeHubSpec.pickSettle * 1_000_000_000))
                guard !Task.isCancelled else { return }
                collapse()
            }
        }
    }

    /// The system cancelled the touch: settle without a pick.
    private func cancelTouch() {
        active = false
        held = false
        slid = false
        pressing = false
        holdTask?.cancel()
        stretchTask?.cancel()
        lastSample = nil
        if !reduced {
            withAnimation(ModeHubSpec.jellySpring) { lensSwell = 1 }
            withAnimation(ModeHubSpec.stretchSpring) { lensStretch = 1 }
        }
        if open, !settling { scheduleCollapse() }
    }

    private func cancelAllTasks() {
        collapseTask?.cancel()
        holdTask?.cancel()
        settleTask?.cancel()
        stretchTask?.cancel()
    }

    // MARK: Actions

    private func expand() {
        ModeHubSpec.fire(.orbTap)
        hover = mode
        withOptionalAnimation { open = true }
        onOpenChange?(true)
        scheduleCollapse()
    }

    private func collapse() {
        cancelAllTasks()
        hover = nil
        settling = false
        slid = false
        lensReady = false
        lensSwell = 1
        lensStretch = 1
        withOptionalAnimation { open = false }
        onOpenChange?(false)
    }

    /// The pill folds back into the orb, which arrives showing the new mode's icon.
    /// (VoiceOver and escape paths; touch picks go through `lift`.)
    private func pick(_ m: ShellMode) {
        ModeHubSpec.fire(.openItem)
        collapse()
        if m != mode { onSelect(m) }
    }

    /// Phone has no hover-out, so an untouched pill folds back after a pause.
    private func scheduleCollapse() {
        collapseTask?.cancel()
        collapseTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: 4_000_000_000)
            guard !Task.isCancelled else { return }
            collapse()
        }
    }

    private func withOptionalAnimation(_ body: () -> Void) {
        if reduced { body() } else { withAnimation(ModeHubSpec.pillSpring, body) }
    }
}

/// The shell's mode orb on the current mode. A pick switches mode with the
/// polish crossfade (a fade under Reduce Motion) and posts the arrive event.
/// `opensShelves`: the hold opens the hub sheet (the composer orb and ACI's
/// floating orb); the drawer's switcher is tap-only.
struct ShellModeOrb: View {
    var size: CGFloat = 36
    var opensShelves = true
    var onOpenChange: ((Bool) -> Void)? = nil

    @EnvironmentObject private var modeStore: AppModeStore
    @ReducedMotion private var reduced

    var body: some View {
        ModeOrbView(mode: modeStore.shellMode,
                    size: size,
                    onSelect: select,
                    onHold: opensShelves ? { NotificationCenter.default.post(name: ModeHubSpec.openModeHub, object: nil) } : nil,
                    onOpenChange: onOpenChange)
    }

    private func select(_ mode: ShellMode) {
        let from = modeStore.shellMode
        withAnimation(Motion.pick(PolishTokens.base, reduced: reduced)) {
            modeStore.setShellMode(mode)
        }
        NotificationCenter.default.post(name: ModeHubSpec.modeArrive, object: nil,
                                        userInfo: ["from": from.hubId, "to": mode.hubId])
    }
}
