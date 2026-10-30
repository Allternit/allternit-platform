import SwiftUI

// MARK: - Deck modes

/// The modes the chat deck's Agent mode ▾ offers (web ModeDock MODE_TABS,
/// chat surface) with the web's labels, colors and icons, mapped onto the
/// app's AgentModeTile (which stamps `metadata.agentModeId`).
struct ChatDeckMode: Identifiable, Equatable {
    let tile: AgentModeTile
    let label: String
    let color: Color

    var id: String { tile.rawValue }
    /// The Allternit icon family tile (deck-<kind> in Assets.xcassets).
    var icon: String { tile.deckIcon ?? "deck-auto" }

    static let chat: [ChatDeckMode] = [
        ChatDeckMode(tile: .swarms, label: "Agent Swarm", color: Color(hex: "#14B8A6")),
        ChatDeckMode(tile: .research, label: "Deep Research", color: Theme.statusInfo),
        ChatDeckMode(tile: .website, label: "Build", color: Color("TextPrimary")),
        ChatDeckMode(tile: .docs, label: "Docs", color: Color(hex: "#3B82F6")),
        ChatDeckMode(tile: .data, label: "Sheets", color: Theme.statusSuccess),
        ChatDeckMode(tile: .slides, label: "Slides", color: Theme.statusWarning),
        ChatDeckMode(tile: .image, label: "Image", color: Color("TextPrimary")),
        ChatDeckMode(tile: .video, label: "Video", color: Color(hex: "#EC4899")),
        // Last, as on the web: the agent picks the format.
        ChatDeckMode(tile: .artifact, label: "Artifact", color: Color("TextPrimary")),
    ]

    /// A bot's chat (ModeDock `bot`): no Docs/Sheets/Slides, since bot
    /// threads have no editor pane.
    static let bot: [ChatDeckMode] = chat.filter {
        [.swarms, .research, .website, .image, .video, .artifact].contains($0.tile)
    }

    /// The menu's second line (DeckSelectors: only Artifact has one).
    var detail: String? { tile == .artifact ? "Let Allternit pick the format" : nil }

    static func forTile(_ tile: AgentModeTile) -> ChatDeckMode? {
        chat.first { $0.tile == tile }
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
        return ChatDeckMode.forTile(agentModeStore.selectedTile(for: surface))
    }

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 10) {
                modeMenu
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
        // Tucked 8pt under the card: the band's top padding clears it.
        .padding(.top, (variant == .full ? 14 : 12) + 8)
        .padding(.bottom, variant == .full ? 8 : 6)
        .frame(minHeight: (variant == .full ? 52 : 40) + 8)
        .background(
            UnevenRoundedRectangle(bottomLeadingRadius: 16, bottomTrailingRadius: 16)
                .fill(Color("BgPanel"))
        )
        .overlay(
            DeckOutline()
                .stroke(Color("BorderSubtle"), lineWidth: 1)
        )
        .padding(.top, -8)
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
            DeckArtifactsSheet()
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
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        agentModeStore.selectTile(mode.tile, for: surface)
        // Picking a mode in the chat's deck turns that mode on for the next send.
        if !agentModeStore.isAgentEnabled(for: surface) {
            agentModeStore.setAgentEnabled(true, for: surface)
        }
    }

    private func clearMode() {
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

    private func mention(_ plugin: DeckPlugin) {
        let tag = "@\(plugin.name) "
        guard !inputText.contains(tag) else { return }
        inputText = tag + inputText
    }
}

/// Sides and bottom of the deck tray — its top edge hides under the card.
private struct DeckOutline: Shape {
    func path(in rect: CGRect) -> Path {
        let r: CGFloat = 16
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
    @EnvironmentObject private var modeStore: AppModeStore
    @Environment(\.dismiss) private var dismiss

    private enum LoadState { case loading, loaded([ArtifactV2Summary]), failed }
    @State private var state: LoadState = .loading
    @State private var openArtifact: ArtifactV2Summary? = nil

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
                    // Pins are made with "Pin to deck" on an open artifact
                    // (web deck-pins); this app has none of its own yet.
                    (Text("Nothing pinned yet. Open an artifact and choose ")
                        + Text("Pin to deck").fontWeight(.semibold).foregroundColor(Color("TextPrimary"))
                        + Text("."))
                        .font(.system(size: 12.5))
                        .foregroundColor(Color("TextSecondary"))
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: .infinity)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 20)
                        .overlay(
                            RoundedRectangle(cornerRadius: 12)
                                .strokeBorder(Color("BorderStrong"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                        )

                    heading("Recent")
                        .padding(.top, 16)
                    recent

                    Button(action: {
                        dismiss()
                        modeStore.selectBarItem(.artifacts)
                    }) {
                        HStack(spacing: 6) {
                            Text("All artifacts")
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
        .fullScreenCover(item: $openArtifact) { summary in
            V2ArtifactView(artifactId: summary.id, initialTitle: summary.title)
        }
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
            statusLine("No artifacts yet")
        case .loaded(let items):
            LazyVGrid(columns: columns, spacing: 10) {
                ForEach(items) { artifact in
                    Button(action: { openArtifact = artifact }) {
                        card(artifact)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
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
            let latest = page.items
                .sorted { ($0.updatedDate ?? .distantPast) > ($1.updatedDate ?? .distantPast) }
                .prefix(3)
            state = .loaded(Array(latest))
        } catch {
            state = .failed
        }
    }
}
