import SwiftUI

/// Mode orb (contract "Mode orb" + "Timings and motion").
/// Rest: a 36 pt interactive glass orb with the current mode's icon. Tap: it
/// opens into a glass pill of the four modes (light haptic). Hold 450 ms
/// (medium haptic): opens the hub sheet via `onHold`; with no `onHold` the orb
/// is tap-only (the drawer's switcher). Dragging across the pill moves the lens
/// (selection haptic); lifting on a mode picks it (medium haptic). Reduce
/// Motion: states change at once, no springs, no press scale.
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
    @ObservedObject private var botStore = BotModeStore.shared
    @State private var open = false
    @State private var lens: ShellMode?
    @State private var pressing = false
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
        .frame(minWidth: size, minHeight: size)
        .polishGlass(cornerRadius: 999, interactive: true)
        .scaleEffect(pressing && !reduced ? ModeHubSpec.holdPressScale : 1)
        .animation(reduced ? nil : ModeHubSpec.pillSpring, value: open)
        .animation(reduced ? nil : ModeHubSpec.tickSpring, value: pressing)
        .onDisappear {
            collapseTask?.cancel()
            pressing = false
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
                .matchedGeometryEffect(id: "orb", in: ns)
                // System tap + long-press: both reset on cancel (sheet presenting, drawer drag,
                // keyboard tap recognizer), so a cancelled press can never leave the orb stuck.
                .onTapGesture { expand() }
                .accessibilityElement()
                .accessibilityLabel(focusedBot.map { "Bots, in chat with \($0.botDisplayName)" } ?? "Mode: \(mode.label)")
                .accessibilityHint(onHold == nil ? "Tap to switch mode" : "Tap to switch mode, hold for all modes")
                .accessibilityAddTraits(.isButton)
                .accessibilityAction { expand() }
        )
    }

    /// The hold that opens the hub sheet, only when the host gives `onHold`.
    @ViewBuilder
    private func holdable(_ glyph: some View) -> some View {
        if let onHold {
            glyph
                .onLongPressGesture(
                    minimumDuration: ModeHubSpec.holdDuration,
                    maximumDistance: 40,
                    perform: {
                        pressing = false
                        ModeHubSpec.fire(.holdComplete)
                        onHold()
                    },
                    onPressingChanged: { down in
                        if down { Haptics.prepare() }
                        pressing = down
                    }
                )
                .accessibilityAction(named: "Open all modes") { ModeHubSpec.fire(.holdComplete); onHold() }
        } else {
            glyph
        }
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
                    // The drawer header is narrower than the composer row.
                    .minimumScaleFactor(0.8)
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
        onOpenChange?(true)
        scheduleCollapse()
    }

    private func collapse() {
        collapseTask?.cancel()
        lens = nil
        withOptionalAnimation { open = false }
        onOpenChange?(false)
    }

    /// The pill folds back into the orb, which arrives showing the new mode's icon.
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
