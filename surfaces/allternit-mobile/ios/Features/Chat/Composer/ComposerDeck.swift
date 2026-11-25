import SwiftUI

// MARK: - Deck modes

/// The modes the chat deck's Agent mode ▾ offers: the generated registry
/// (Core/AgentModes/AgentModes.gen.swift, from contracts/agent-modes) filtered
/// to this surface, in the registry's order with its labels and badges. Each
/// entry maps onto the app's AgentModeTile (which stamps
/// `metadata.agentModeId`) through its wire id; kinds with no wire id
/// (Dashboard, Ops sheet, Motion, PDF) keep the Artifact tile only as stored
/// state: Send starts their own artifact session (DeckLaunch), and PDF opens
/// the library's file picker.
struct ChatDeckMode: Identifiable, Equatable {
    let entry: AgentModeEntry

    var id: String { entry.id }
    var label: String { entry.label }
    var tile: AgentModeTile { entry.agentMode.flatMap { AgentModeTile(rawValue: $0) } ?? .artifact }
    var color: Color {
        switch tile {
        case .website, .image, .artifact: return Color("TextPrimary")
        default: return tile.color
        }
    }
    /// The Allternit icon family tile (deck-<kind> in Assets.xcassets).
    var icon: String {
        switch entry.icon {
        case "app": return "deck-build"
        default: return "deck-\(entry.icon)"
        }
    }
    /// Placeholder the composer shows once this mode is picked (step 5.2 reads it).
    var placeholder: String { entry.placeholder }

    /// The menu's second line: the Agent row's hint, else the Beta / New badge.
    var detail: String? {
        if entry.id == "agent" { return "Let Allternit pick the format" }
        if entry.beta { return "Beta" }
        if entry.isNew { return "New" }
        return nil
    }

    static func == (a: ChatDeckMode, b: ChatDeckMode) -> Bool { a.id == b.id }

    private static func modes(_ surface: String) -> [ChatDeckMode] {
        AgentModes.modes(for: surface).map(ChatDeckMode.init(entry:))
    }

    /// The chat's modes (registry surface `chat`).
    static var chat: [ChatDeckMode] { modes("chat") }
    /// A bot's chat (registry surface `bot`): no Docs/Sheets/Slides, since bot
    /// threads have no editor pane.
    static var bot: [ChatDeckMode] { modes("bot") }

    /// The mode a stored tile stands for (Sites is "Build", not "Page").
    static func forTile(_ tile: AgentModeTile) -> ChatDeckMode? {
        let all = chat
        let wire = tile.rawValue
        let preferred = tile == .website ? "build" : tile == .artifact ? "agent" : wire
        return all.first { $0.id == preferred && $0.entry.agentMode == wire }
            ?? all.first { $0.entry.agentMode == wire }
    }

    /// The picked mode when Send should start an artifact session for it
    /// (nil for Agent, Swarm and a PDF, which opens a file picker instead).
    static func pendingLaunch(tile: AgentModeTile, surface: AppMode, bot: Bool) -> ChatDeckMode? {
        guard let m = picked(tile: tile, surface: surface, in: bot ? Self.bot : Self.chat),
              DeckLaunch.kind(for: m.entry) != nil else { return nil }
        return m
    }

    /// The mode the deck shows as picked: the remembered registry id when it
    /// still matches the stored tile (several kinds share the `website` tile),
    /// else the tile's own mode.
    static func picked(tile: AgentModeTile, surface: AppMode, in list: [ChatDeckMode]) -> ChatDeckMode? {
        if let id = DeckModePick.id(for: surface),
           let m = list.first(where: { $0.id == id }), m.tile == tile { return m }
        return list.first { $0.tile == tile && $0.entry.agentMode != nil } ?? forTile(tile)
    }
}

/// Plugins the deck lists (web deckPlugins.ts), each with its OfficeAppLogo
/// tile. Picking one puts it in the message as an @-mention.
private struct DeckPlugin: Identifiable {
    let id: String
    let name: String
    let icon: String

    static let suite: [DeckPlugin] = [
        DeckPlugin(id: "allternit-docs", name: "Allternit Docs", icon: "deck-docs"),
        DeckPlugin(id: "allternit-sheets", name: "Allternit Sheets", icon: "deck-sheets"),
        DeckPlugin(id: "allternit-slides", name: "Allternit Slides", icon: "deck-slides"),
        DeckPlugin(id: "allternit-pdf", name: "Allternit PDF", icon: "deck-pdf"),
        DeckPlugin(id: "allternit-sign", name: "Allternit Sign", icon: "deck-sign"),
    ]

    static let extensions: [DeckPlugin] = [
        DeckPlugin(id: "office-word", name: "Allternit for Word", icon: "deck-word"),
        DeckPlugin(id: "office-excel", name: "Allternit for Excel", icon: "deck-excel"),
        DeckPlugin(id: "office-powerpoint", name: "Allternit for PowerPoint", icon: "deck-powerpoint"),
    ]
}

// MARK: - Deck band

/// The composer's bottom deck (web ComposerDeck + DeckSelectors): a tray
/// tucked behind the card's bottom edge holding Agent mode ▾ | Plugins ▾ |
/// Artifacts ▾ on one line that scrolls sideways when it outgrows the phone.
struct ComposerDeckBand: View {
    let variant: ComposerDeckVariant
    @Binding var inputText: String
    /// An artifact session's own kind: a fixed label in place of the mode menu,
    /// since the session makes what it was started for.
    var sessionKind: ArtifactStudioKind? = nil
    /// Artifacts ▾ → All artifacts, where leaving means more than switching
    /// the section underneath (an artifact session is a full-screen cover).
    var onAllArtifacts: (() -> Void)? = nil
    /// True while one of the band's sheets is up, so the folded band does not
    /// close under it when the keyboard focus moves (contract C1).
    var onSheetActive: ((Bool) -> Void)? = nil
    /// The tray's own bottom corners (the deck's size before the tray).
    private let radius: CGFloat = 16

    @EnvironmentObject private var modeStore: AppModeStore
    @EnvironmentObject private var agentModeStore: AgentModeStore
    @Environment(\.chatBot) private var chatBot
    @State private var isManagePluginsPresented = false
    @State private var isArtifactsPresented = false

    private var surface: AppMode { modeStore.mode }
    /// The chat's deck shows what a send will use: a mode only once agent
    /// mode is on.
    private var current: ChatDeckMode? {
        guard agentModeStore.isAgentEnabled(for: surface) else { return nil }
        return ChatDeckMode.picked(tile: agentModeStore.selectedTile(for: surface), surface: surface,
                                   in: chatBot == nil ? ChatDeckMode.chat : ChatDeckMode.bot)
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                if let sessionKind {
                    sessionKindLabel(sessionKind)
                } else {
                    modeMenu
                }
                separator
                pluginsMenu
                separator
                Button(action: { isArtifactsPresented = true }) {
                    trigger(icon: "square.stack.3d.up", label: "Artifacts")
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, variant == .full ? 16 : 12)
        }
        // Full width and tucked under the card past its round bottom
        // corners, so the tray fills them and its side walls run on from the
        // card's sides. Only the part below the card shows: 14 (12) above
        // the row, 52 (40) tall at least, the deck's size before the tray.
        .padding(.top, (variant == .full ? 14 : 12) + DeckTuck.underCard)
        .padding(.bottom, variant == .full ? 8 : 6)
        .frame(minHeight: (variant == .full ? 52 : 40) + DeckTuck.underCard)
        .background(
            UnevenRoundedRectangle(bottomLeadingRadius: radius, bottomTrailingRadius: radius)
                .fill(Color("FillNeutral"))
        )
        .overlay(
            DeckOutline(radius: radius)
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .padding(.top, -DeckTuck.underCard)
        .onChange(of: isManagePluginsPresented || isArtifactsPresented) { _, active in
            onSheetActive?(active)
        }
        .sheet(isPresented: $isManagePluginsPresented) {
            NavigationStack {
                CapabilitiesSettingsView(kind: .plugin)
                    .toolbar {
                        ToolbarItem(placement: .topBarTrailing) {
                            Button("Done") { isManagePluginsPresented = false }
                        }
                    }
            }
            .environmentObject(AuthManager.shared)
        }
        .sheet(isPresented: $isArtifactsPresented) {
            DeckArtifactsSheet(onAllArtifacts: onAllArtifacts, onAttach: attach)
                .environmentObject(modeStore)
        }
    }

    private var separator: some View {
        Text("|")
            .font(.system(size: 11))
            .foregroundColor(Color("TextSecondary"))
            .opacity(0.4)
            .accessibilityHidden(true)
    }

    private func trigger(icon: String?, label: String, iconSize: CGFloat = 13) -> some View {
        HStack(spacing: 6) {
            if let icon {
                Image(systemName: icon)
                    .font(.system(size: iconSize, weight: .bold))
            }
            Text(label)
                .font(.system(size: 12, weight: .bold))
            Image(systemName: "chevron.up")
                .font(.system(size: 8, weight: .bold))
                .opacity(0.6)
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 4)
        .frame(minHeight: 28)
        .contentShape(Rectangle())
    }

    private func sessionKindLabel(_ kind: ArtifactStudioKind) -> some View {
        HStack(spacing: 6) {
            Image(kind.iconAsset)
                .resizable()
                .frame(width: 14, height: 14)
            Text(kind.label)
                .font(.system(size: 12, weight: .bold))
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 6)
        .frame(minHeight: 28)
        .accessibilityElement(children: .combine)
    }

    // MARK: Agent mode ▾

    private var modeMenu: some View {
        Menu {
            Section("Agent mode") {
                ForEach(chatBot == nil ? ChatDeckMode.chat : ChatDeckMode.bot) { mode in
                    Button(action: { select(mode) }) {
                        if current == mode {
                            Label(mode.label, systemImage: "checkmark")
                        } else {
                            Label {
                                Text(mode.label)
                                if let detail = mode.detail { Text(detail) }
                            } icon: { Image(mode.icon) }
                        }
                    }
                }
            }
            if current != nil {
                Divider()
                Button(action: clearMode) {
                    Label("Clear mode", systemImage: "xmark")
                }
            }
        } label: {
            if let current {
                // Picking a mode turns the trigger into that mode's chip.
                HStack(spacing: 6) {
                    Image(current.icon)
                        .resizable()
                        .frame(width: 14, height: 14)
                    Text(current.label)
                        .font(.system(size: 12, weight: .bold))
                    Image(systemName: "chevron.up")
                        .font(.system(size: 8, weight: .bold))
                        .opacity(0.6)
                }
                .foregroundColor(current.color)
                .padding(.horizontal, 6)
                .frame(minHeight: 28)
                .contentShape(Rectangle())
                .accessibilityLabel("Agent mode: \(current.label). Change mode")
            } else {
                trigger(icon: "diamond.fill", label: "Agent mode", iconSize: 10)
            }
        }
        .buttonStyle(.plain)
    }

    private func select(_ mode: ChatDeckMode) {
        Haptics.light()
        // PDF has nothing to describe: it opens the Artifacts library's file
        // picker (open a PDF to edit, fill in and sign).
        if mode.entry.launchType == .pick {
            ItCanStore.shared.wantsPDFPick = true
            modeStore.selectBarItem(.artifacts)
            return
        }
        DeckModePick.set(mode.id, for: surface)
        agentModeStore.selectTile(mode.tile, for: surface)
        // Picking a mode in the chat's deck turns that mode on for the next send.
        if !agentModeStore.isAgentEnabled(for: surface) {
            agentModeStore.setAgentEnabled(true, for: surface)
        }
    }

    private func clearMode() {
        DeckModePick.set(nil, for: surface)
        agentModeStore.setAgentEnabled(false, for: surface)
    }

    // MARK: Plugins ▾

    private var pluginsMenu: some View {
        Menu {
            Section("Allternit Office") {
                ForEach(DeckPlugin.suite) { plugin in
                    Button(action: { mention(plugin) }) {
                        Label { Text(plugin.name) } icon: { Image(plugin.icon) }
                    }
                }
            }
            Section("Extensions") {
                ForEach(DeckPlugin.extensions) { plugin in
                    Button(action: { mention(plugin) }) {
                        Label { Text(plugin.name) } icon: { Image(plugin.icon) }
                    }
                }
            }
            Divider()
            Button("More plugins…") {
                // The web opens its @ picker; here the @ starts the mention.
                inputText += inputText.isEmpty || inputText.hasSuffix(" ") ? "@" : " @"
            }
            Button("Manage plugins…") { isManagePluginsPresented = true }
        } label: {
            trigger(icon: nil, label: "Plugins")
        }
        .buttonStyle(.plain)
    }

    /// Attach on a Recent artifact: a removable chip in the composer, and on
    /// Send one `[Attached artifact: ...]` line (the web deck's Attach).
    private func attach(_ artifact: ArtifactV2Summary) {
        AttachedArtifactStore.shared.attach(AttachedArtifact(summary: artifact))
    }

    private func mention(_ plugin: DeckPlugin) {
        let tag = "@\(plugin.name) "
        guard !inputText.contains(tag) else { return }
        inputText = tag + inputText
    }
}

/// How far the bottom tray runs in under the composer card. The card is
/// open (continuous corners, radius 22) whenever the tray shows, and a
/// continuous corner starts curving about 1.53 radii from the edge, so 34
/// clears it: the tray's walls start behind the card's straight sides and
/// meet them with no notch at the corners.
enum DeckTuck {
    static let underCard: CGFloat = 34
}

/// Sides and bottom of the deck tray — its top edge hides under the card.
private struct DeckOutline: Shape {
    var radius: CGFloat = 16
    func path(in rect: CGRect) -> Path {
        let r: CGFloat = radius
        var path = Path()
        path.move(to: CGPoint(x: rect.minX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.maxY - r))
        path.addArc(center: CGPoint(x: rect.minX + r, y: rect.maxY - r), radius: r,
                    startAngle: .degrees(180), endAngle: .degrees(90), clockwise: true)
        path.addLine(to: CGPoint(x: rect.maxX - r, y: rect.maxY))
        path.addArc(center: CGPoint(x: rect.maxX - r, y: rect.maxY - r), radius: r,
                    startAngle: .degrees(90), endAngle: .degrees(0), clockwise: true)
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        return path
    }
}

// MARK: - Top deck tray

/// The tray above the composer card (ThreadComposer `thread-top-deck`):
/// tucked 8pt under the card's top edge, one line that scrolls sideways.
struct ComposerTopDeckTray<Content: View>: View {
    @ViewBuilder let content: () -> Content

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            content().padding(.horizontal, 8)
        }
        .padding(.top, 6)
        .padding(.bottom, 6 + 8)
        .frame(minHeight: 46 + 8)
        .background(
            UnevenRoundedRectangle(topLeadingRadius: 16, topTrailingRadius: 16)
                .fill(Color("BgPanel"))
        )
        .overlay(
            DeckOutline()
                .rotation(.degrees(180))
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .padding(.bottom, -8)
    }
}

// MARK: - Artifacts ▾

/// Artifacts ▾ in the deck (web DeckArtifactsMenu): pinned artifacts, the
/// three most recent, and a way into the Artifacts library.
struct DeckArtifactsSheet: View {
    var onAllArtifacts: (() -> Void)? = nil
    /// Attach on a Recent card puts the artifact in the message.
    var onAttach: ((ArtifactV2Summary) -> Void)? = nil

    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.dismiss) private var dismiss

    private enum LoadState { case loading, loaded([ArtifactV2Summary]), failed }
    @State private var state: LoadState = .loading
    @State private var openArtifact: ArtifactV2Summary? = nil
    @State private var openPin: DeckPin? = nil
    @ObservedObject private var pinsStore = DeckPinsStore.shared

    private let columns = [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)]

    var body: some View {
        VStack(spacing: 0) {
            Capsule()
                .fill(Color("BorderStrong"))
                .frame(width: 36, height: 5)
                .padding(.top, 8)
                .padding(.bottom, 12)

            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    heading("Pinned")
                    // Pins live per user on the runtime (DeckPinsStore, web deck-pins);
                    // Pin / Unpin is on the artifact card's ⋯ menu.
                    if pinsStore.pins.isEmpty {
                        VStack(spacing: 8) {
                            EmptyArt(kind: .sheets, size: .panel)
                            (Text("Nothing pinned yet. Open an artifact and choose ")
                                + Text("Pin to deck").fontWeight(.semibold).foregroundColor(Color("TextPrimary"))
                                + Text("."))
                                .font(.system(size: 12.5))
                                .foregroundColor(Color("TextSecondary"))
                                .multilineTextAlignment(.center)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 20)
                        .overlay(
                            RoundedRectangle(cornerRadius: 12)
                                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                        )
                    } else {
                        LazyVGrid(columns: columns, spacing: 10) {
                            ForEach(pinsStore.pins) { pin in
                                Button(action: { openPin = pin }) { pinCard(pin) }
                                    .buttonStyle(.plain)
                                    .accessibilityLabel("Open \(pin.title)")
                            }
                        }
                    }

                    heading("Recent")
                        .padding(.top, 16)
                    recent

                    Button(action: {
                        dismiss()
                        if let onAllArtifacts {
                            onAllArtifacts()
                        } else {
                            modeStore.selectBarItem(.artifacts)
                        }
                    }) {
                        HStack(spacing: 6) {
                            Text("Open Artifacts")
                            Image(systemName: "arrow.right")
                                .font(.system(size: 12))
                        }
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .padding(.horizontal, 4)
                        .padding(.vertical, 8)
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 8)
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 20)
            }
        }
        .background(Color("BgPrimary"))
        .presentationDetents([.medium, .large])
        .presentationCornerRadius(28)
        .presentationDragIndicator(.hidden)
        .task { await load() }
        .task { await pinsStore.load() }
        .fullScreenCover(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title)
        }
        .fullScreenCover(item: $openPin) { pin in
            V2ArtifactView(artifactId: pin.id, initialTitle: pin.title)
        }
    }

    private func pinCard(_ pin: DeckPin) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                Color("FillNeutral")
                Image(ArtifactV2Kind.iconAsset(for: pin.kind))
                    .resizable()
                    .frame(width: 16, height: 16)
            }
            .aspectRatio(16 / 9, contentMode: .fit)
            VStack(alignment: .leading, spacing: 2) {
                Text(pin.title)
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(ArtifactV2Kind(rawValue: pin.kind)?.noun ?? pin.kind.capitalized)
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func heading(_ label: String) -> some View {
        Text(label.uppercased())
            .font(.system(size: 11, weight: .semibold))
            .tracking(0.5)
            .foregroundColor(Color("TextTertiary"))
            .padding(.horizontal, 4)
            .padding(.bottom, 8)
    }

    @ViewBuilder
    private var recent: some View {
        switch state {
        case .loading:
            statusLine("Loading…")
        case .failed:
            statusLine("Couldn’t load artifacts")
        case .loaded(let items) where items.isEmpty:
            FriendlyStateView(title: "No artifacts yet", art: .sheets, size: .panel)
        case .loaded(let items):
            LazyVGrid(columns: columns, spacing: 10) {
                ForEach(items) { artifact in
                    VStack(spacing: 0) {
                        Button(action: { openArtifact = artifact }) {
                            card(artifact)
                        }
                        .buttonStyle(.plain)
                        HStack(spacing: 8) {
                            if onAttach != nil {
                                actionButton("Attach", icon: "paperclip") {
                                    onAttach?(artifact)
                                    dismiss()
                                }
                            }
                            actionButton("Open", icon: "arrow.up.right") { openArtifact = artifact }
                        }
                        .padding(.top, 6)
                    }
                }
            }
        }
    }

    private func actionButton(_ label: String, icon: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 11))
                Text(label)
            }
            .font(.system(size: 12.5, weight: .medium))
            .foregroundColor(Color("TextSecondary"))
            .frame(maxWidth: .infinity, minHeight: 32)
            .background(Color("FillNeutral"))
            .clipShape(RoundedRectangle(cornerRadius: 8))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    private func statusLine(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12.5))
            .foregroundColor(Color("TextTertiary"))
            .padding(.horizontal, 4)
    }

    private func card(_ artifact: ArtifactV2Summary) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            ZStack {
                Color("FillNeutral")
                Image(ArtifactV2Kind.iconAsset(for: artifact.kind))
                    .resizable()
                    .frame(width: 16, height: 16)
            }
            .aspectRatio(16 / 9, contentMode: .fit)

            VStack(alignment: .leading, spacing: 2) {
                Text(artifact.title)
                    .font(.system(size: 13.5, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Text(meta(artifact))
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
                    .lineLimit(1)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
        .background(Color("BgPanel"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    /// "Doc · updated 3 h ago" (web deckRelativeTime).
    private func meta(_ artifact: ArtifactV2Summary) -> String {
        let label = artifact.knownKind?.noun ?? artifact.kind.capitalized
        guard let date = artifact.updatedDate else { return label }
        let s = Int(Date().timeIntervalSince(date))
        let when: String
        if s < 60 { when = "just now" }
        else if s < 3600 { when = "\(Int((Double(s) / 60).rounded())) min ago" }
        else if s < 86_400 { when = "\(Int((Double(s) / 3600).rounded())) h ago" }
        else if s < 86_400 * 7 { when = "\(Int((Double(s) / 86_400).rounded())) d ago" }
        else { when = date.formatted(date: .numeric, time: .omitted) }
        return "\(label) · updated \(when)"
    }

    private func load() async {
        do {
            let page = try await ArtifactsV2Client.shared.list(ArtifactV2ListQuery(limit: 20))
            // Plans are listed in Plans, never in the deck.
            let latest = page.items
                .filter { $0.kind != "plan" }
                .sorted { ($0.updatedDate ?? .distantPast) > ($1.updatedDate ?? .distantPast) }
                .prefix(3)
            state = .loaded(Array(latest))
        } catch {
            state = .failed
        }
    }
}
