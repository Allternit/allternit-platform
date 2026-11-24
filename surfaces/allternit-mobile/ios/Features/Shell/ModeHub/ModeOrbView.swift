import SwiftUI

/// Mode orb (contract "Mode orb" + "Timings and motion").
/// Rest: a 36 pt glass orb with the current mode's icon. Tap: it opens into a
/// pill of the four modes (light haptic). Hold 450 ms (medium haptic): opens
/// the hub sheet via `onHold`. Dragging across the pill moves the lens
/// (selection haptic); lifting on a mode picks it. Reduce Motion: states
/// change at once, no springs, no press scale.
struct ModeOrbView: View {
    let mode: ShellMode
    /// A mode was picked in the pill.
    var onSelect: (ShellMode) -> Void
    /// The hold completed: open the hub sheet.
    var onHold: () -> Void

    @ReducedMotion private var reduced
    @ObservedObject private var botStore = BotModeStore.shared
    @State private var open = false
    @State private var lens: ShellMode?
    @State private var pressing = false
    @State private var holdTask: Task<Void, Never>?
    @State private var held = false
    @State private var collapseTask: Task<Void, Never>?
    @Namespace private var ns

    private let tickRowHeight: CGFloat = 36

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
        HStack(spacing: 2) {
            if open {
                ForEach(ShellMode.allCases, id: \.self) { m in pillButton(m) }
            } else {
                orb
            }
        }
        .padding(open ? 4 : 0)
        .frame(minWidth: 36, minHeight: 36)
        .glassSurface(cornerRadius: 999)
        .scaleEffect(pressing && !reduced ? ModeHubSpec.holdPressScale : 1)
        .animation(reduced ? nil : ModeHubSpec.pillSpring, value: open)
        .animation(reduced ? nil : ModeHubSpec.tickSpring, value: pressing)
        .onDisappear {
            holdTask?.cancel()
            holdTask = nil
            collapseTask?.cancel()
            pressing = false
            held = false
        }
        .accessibilityElement(children: .contain)
        .accessibilityAction(.escape) { collapse() }
    }

    // MARK: Rest orb

    private var orb: some View {
        slotGlyph(mode, size: 16, weight: .semibold)
            .foregroundStyle(mode.accent)
            .frame(width: 36, height: 36)
            .contentShape(Circle())
            .id(mode)
            .transition(reduced ? .identity : .scale(scale: 0.5).combined(with: .opacity))
            .matchedGeometryEffect(id: "orb", in: ns)
            .gesture(
                DragGesture(minimumDistance: 0)
                    .onChanged { _ in beginPress() }
                    .onEnded { _ in endPress() }
            )
            .accessibilityElement()
            .accessibilityLabel(focusedBot.map { "Bots, in chat with \($0.botDisplayName)" } ?? "Mode: \(mode.label)")
            .accessibilityHint("Tap to switch mode, hold for all modes")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { expand() }
            .accessibilityAction(named: "Open all modes") { ModeHubSpec.fire(.holdComplete); onHold() }
    }

    private func beginPress() {
        guard holdTask == nil else { return }
        held = false
        pressing = true
        Haptics.prepare()
        holdTask = Task { @MainActor in
            try? await Task.sleep(nanoseconds: UInt64(ModeHubSpec.holdDuration * 1_000_000_000))
            guard !Task.isCancelled else { return }
            held = true
            pressing = false
            ModeHubSpec.fire(.holdComplete)
            onHold()
        }
    }

    private func endPress() {
        let wasHeld = held
        holdTask?.cancel()
        holdTask = nil
        pressing = false
        if !wasHeld { expand() }
        held = false
    }

    // MARK: Pill

    private func pillButton(_ m: ShellMode) -> some View {
        let current = m == mode
        let on = m == (lens ?? mode)
        return Button {
            pick(m)
        } label: {
            HStack(spacing: 6) {
                slotGlyph(m, size: 14, weight: current ? .bold : .regular)
                Text(m.hubLabel)
                    .font(.system(size: 12.5, weight: current ? .semibold : .medium))
                    .lineLimit(1)
            }
            .foregroundStyle(current ? Color.primary : Color.secondary)
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background {
                if on {
                    Capsule()
                        .fill(Color("FillNeutral"))
                        .matchedGeometryEffect(id: "lens", in: ns)
                        .animation(reduced ? nil : ModeHubSpec.lensSpring, value: lens)
                }
            }
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(m == .bots ? (focusedBot.map { "Bots, in chat with \($0.botDisplayName)" } ?? m.label) : m.label)
        .accessibilityAddTraits(current ? [.isSelected] : [])
        .transition(reduced ? .identity : .scale(scale: 0.8).combined(with: .opacity))
        // Press-and-slide across the pill moves the lens.
        .simultaneousGesture(
            DragGesture(minimumDistance: 0).onChanged { _ in
                if lens != m { lens = m; ModeHubSpec.fire(.lensMove) }
                scheduleCollapse()
            }
        )
    }

    // MARK: Actions

    private func expand() {
        ModeHubSpec.fire(.orbTap)
        lens = mode
        withOptionalAnimation { open = true }
        scheduleCollapse()
    }

    private func collapse() {
        collapseTask?.cancel()
        lens = nil
        withOptionalAnimation { open = false }
    }

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
