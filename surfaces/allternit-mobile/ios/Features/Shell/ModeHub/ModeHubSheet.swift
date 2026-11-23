import SwiftUI

/// One preview row inside a hub section (a chat, project, bot, ...). The host
/// supplies these from its real stores; the sheet never invents data.
struct ModeHubItem: Identifiable, Equatable {
    let id: String
    let title: String
    var subtitle: String? = nil
    var symbol: String? = nil
}

/// The mode hub "Shelves" sheet, phone variant (mockup base2.jsx HubShelves,
/// contract CONTRACT.md). Rises from the bottom edge. Top to bottom: path +
/// More, up to 4 item rows (3 fit above the cards), the section cards (first
/// tap selects, second tap opens), then the four modes at the bottom.
/// Not mounted anywhere yet (steps I2-I4).
struct ModeHubSheet: View {
    @Binding var isPresented: Bool
    /// The app's current mode; the sheet starts here.
    let currentMode: ShellMode
    /// Preview items for a section (empty = the card shows its label only).
    var items: (ModeHubSection) -> [ModeHubItem] = { _ in [] }
    /// Enabled feature flags for `requires` (contract "Requires").
    var enabledFlags: Set<String> = ["meetings"]
    /// Open a section (item nil) or one item in it. The host switches the mode
    /// and fires `allternit:mode-arrive` when it differs from `currentMode`.
    var onOpen: (ModeHubSection, ModeHubItem?) -> Void
    /// A mode's own root opened (tap on the already-shown mode).
    var onOpenMode: (ShellMode) -> Void

    @ReducedMotion private var reduced
    @State private var shownMode: ShellMode
    @State private var selectedId: String?
    @State private var drag: CGFloat = 0
    @State private var appeared = false
    @Namespace private var ns

    private let visibleItems = 3

    init(isPresented: Binding<Bool>,
         currentMode: ShellMode,
         items: @escaping (ModeHubSection) -> [ModeHubItem] = { _ in [] },
         enabledFlags: Set<String> = ["meetings"],
         onOpen: @escaping (ModeHubSection, ModeHubItem?) -> Void,
         onOpenMode: @escaping (ShellMode) -> Void) {
        _isPresented = isPresented
        self.currentMode = currentMode
        self.items = items
        self.enabledFlags = enabledFlags
        self.onOpen = onOpen
        self.onOpenMode = onOpenMode
        _shownMode = State(initialValue: currentMode)
    }

    private var sections: [ModeHubSection] { ModeHubSpec.sections(for: shownMode, enabledFlags: enabledFlags) }
    private var selected: ModeHubSection? {
        sections.first { $0.id == selectedId } ?? sections.first
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            Color.black.opacity(appeared ? 0.28 : 0)
                .ignoresSafeArea()
                .onTapGesture { dismiss() }
                .accessibilityHidden(true)
            sheet
                .offset(y: appeared ? max(drag, 0) : 600)
                .gesture(
                    DragGesture()
                        .onChanged { drag = $0.translation.height }
                        .onEnded { v in
                            if v.translation.height > 90 || v.predictedEndTranslation.height > 220 { dismiss() }
                            else { withSheet { drag = 0 } }
                        }
                )
        }
        .onAppear { withSheet { appeared = true } }
        .accessibilityAction(.escape) { dismiss() }
    }

    // MARK: Sheet

    private var sheet: some View {
        VStack(spacing: 8) {
            Capsule().fill(Color.primary.opacity(0.18)).frame(width: 36, height: 4).padding(.top, 2)
            header
            itemList
            cards
            modeBar
        }
        .padding(8)
        .padding(.bottom, 6)
        .frame(maxWidth: .infinity)
        .background(Color("BgPrimary").opacity(0.94), in: RoundedRectangle(cornerRadius: 26, style: .continuous))
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 26, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 26, style: .continuous).strokeBorder(Color("BorderSubtle"), lineWidth: 0.5))
        .shadow(color: .black.opacity(0.14), radius: 24, y: 8)
        .padding(.horizontal, 8)
    }

    private var header: some View {
        HStack {
            Text("\(shownMode.hubLabel) \u{203A} \(selected?.label ?? "")")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Color("TextSecondary"))
                .id(shownMode.rawValue + (selected?.id ?? ""))
                .transition(.opacity)
            Spacer()
            if let s = selected {
                Button { open(s, nil) } label: {
                    Text("\(s.more) \u{203A}")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(Color("TextSecondary"))
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .contentShape(Capsule())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(s.more)
            }
        }
        .padding(.horizontal, 6)
    }

    private var itemList: some View {
        let rows = selected.map { Array(items($0).prefix(visibleItems)) } ?? []
        return VStack(spacing: 0) {
            ForEach(rows) { it in
                Button { if let s = selected { open(s, it) } } label: {
                    HStack(spacing: 10) {
                        Image(systemName: it.symbol ?? selected?.symbol ?? "circle")
                            .font(.system(size: 14))
                            .foregroundStyle(Color("TextSecondary"))
                            .frame(width: 22)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(it.title).font(.system(size: 13.5, weight: .medium)).foregroundStyle(Color("TextPrimary")).lineLimit(1)
                            if let sub = it.subtitle {
                                Text(sub).font(.system(size: 11)).foregroundStyle(Color("TextTertiary")).lineLimit(1)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 10)
                    .frame(height: 42)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            Spacer(minLength: 0)
        }
        .frame(height: CGFloat(visibleItems) * 42, alignment: .top)
        .clipped()
        .id(shownMode.rawValue + (selected?.id ?? ""))
        .transition(reduced ? .identity : .opacity.combined(with: .offset(y: 4)))
    }

    private var cards: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(sections) { s in card(s) }
            }
            .padding(.trailing, 24)
        }
        .mask(
            LinearGradient(stops: [.init(color: .black, location: 0.9), .init(color: .clear, location: 1)],
                           startPoint: .leading, endPoint: .trailing)
        )
    }

    private func card(_ s: ModeHubSection) -> some View {
        let on = s.id == selected?.id
        let first = items(s).first
        return Button {
            if on {
                open(s, nil)                       // second tap opens
            } else {
                ModeHubSpec.fire(.cardPick)        // first tap selects
                withTick { selectedId = s.id }
            }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                Image(systemName: s.symbol).font(.system(size: 14, weight: on ? .semibold : .regular))
                Text(s.short).font(.system(size: 12, weight: on ? .semibold : .medium)).lineLimit(1)
                Text(first?.title ?? " ").font(.system(size: 10.5)).foregroundStyle(Color("TextTertiary")).lineLimit(1)
            }
            .foregroundStyle(on ? Color("TextPrimary") : Color("TextSecondary"))
            .padding(10)
            .frame(width: 104, alignment: .leading)
            .background {
                RoundedRectangle(cornerRadius: 16, style: .continuous)
                    .fill(on ? Color("FillNeutral") : Color.clear)
                    .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color("BorderSubtle"), lineWidth: on ? 0 : 0.5))
            }
            .contentShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(s.label)
        .accessibilityHint(on ? "Opens \(s.label)" : "Selects \(s.label)")
        .accessibilityAddTraits(on ? [.isSelected] : [])
    }

    private var modeBar: some View {
        HStack(spacing: 2) {
            ForEach(ShellMode.allCases, id: \.self) { m in
                let on = m == shownMode
                Button {
                    if on {
                        ModeHubSpec.fire(.openItem)
                        onOpenMode(m)
                        dismiss()
                    } else {
                        ModeHubSpec.fire(.lensMove)
                        withLens { shownMode = m; selectedId = nil }
                    }
                } label: {
                    VStack(spacing: 2) {
                        Image(systemName: m.icon).font(.system(size: 15, weight: on ? .semibold : .regular))
                        Text(m.hubLabel).font(.system(size: 12, weight: on ? .semibold : .medium))
                    }
                    .foregroundStyle(on ? Color("TextPrimary") : Color("TextSecondary"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 7)
                    .background {
                        if on {
                            RoundedRectangle(cornerRadius: 17, style: .continuous)
                                .fill(Color("BgPrimary"))
                                .matchedGeometryEffect(id: "modeLens", in: ns)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(m.label)
                .accessibilityAddTraits(on ? [.isSelected] : [])
            }
        }
        .padding(3)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 20, style: .continuous))
    }

    // MARK: Actions and motion

    private func open(_ s: ModeHubSection, _ item: ModeHubItem?) {
        ModeHubSpec.fire(.openItem)
        onOpen(s, item)
        dismiss()
    }

    private func dismiss() {
        if reduced { appeared = false; isPresented = false; return }
        withAnimation(ModeHubSpec.sheetSpring) { appeared = false; drag = 0 }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.22) { isPresented = false }
    }

    private func withSheet(_ body: () -> Void) {
        if reduced { body() } else { withAnimation(ModeHubSpec.sheetSpring, body) }
    }
    private func withLens(_ body: () -> Void) {
        if reduced { body() } else { withAnimation(ModeHubSpec.lensSpring, body) }
    }
    private func withTick(_ body: () -> Void) {
        if reduced { body() } else { withAnimation(ModeHubSpec.tickSpring, body) }
    }
}
