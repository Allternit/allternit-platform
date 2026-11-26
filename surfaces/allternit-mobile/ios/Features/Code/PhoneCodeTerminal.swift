import SwiftUI
import UIKit
import SwiftTerm

// MARK: - Terminal sessions

/// The shell sessions behind the Code terminal (CodeTerminalCanvas's tiles
/// on the phone): up to 8 ptys on the computer's gizzi-code, named
/// "Terminal N". Owned by the Code screen, so closing and reopening the
/// terminal (or flipping to a thread) keeps every shell.
@MainActor
final class CodeTerminalStore: ObservableObject {
    struct Tile: Identifiable {
        let id = UUID()
        let name: String
        let session: PtySession
    }

    static let maxTiles = 8

    @Published private(set) var tiles: [Tile] = []
    @Published var selectedId: UUID? = nil
    @Published private(set) var isStarting = false
    /// No computer answers: the terminal shows its offline state.
    @Published private(set) var isOffline = false
    /// Tabs follow the coding agent running in them ("Claude Code"), else "Terminal N".
    @Published private(set) var agentNames: [UUID: String] = [:]
    private var nextNumber = 1

    var selected: Tile? { tiles.first { $0.id == selectedId } ?? tiles.first }

    func displayName(_ tile: Tile) -> String { agentNames[tile.id] ?? tile.name }

    /// A tile's agent frame changed: keep the agent's name as the tab name (CodeTerminalCanvas `handleAgentFrame`).
    func noteAgent(_ name: String?, for id: UUID) {
        guard let name, agentNames[id] != name else { return }
        agentNames[id] = name
    }

    /// First open: start one shell, or land on the offline state.
    func startIfNeeded() async {
        guard tiles.isEmpty, !isStarting else { return }
        await createTile()
    }

    func createTile() async {
        guard tiles.count < Self.maxTiles, !isStarting else { return }
        isStarting = true
        defer { isStarting = false }
        await InstanceStore.shared.refreshIfNeeded()
        guard let resolved = await InstanceConnection.resolve(from: InstanceStore.shared) else {
            isOffline = true
            return
        }
        isOffline = false
        // A relayed computer serves its terminal service (`/terminal/*`), as
        // m.allternit.com uses; a directly reached gizzi-code serves `/v1/pty`.
        let session = resolved.isRelay
            ? PtySession(service: TerminalServiceClient(baseURL: resolved.baseURL), attachedInstanceName: resolved.instanceName)
            : PtySession(client: PtyClient(baseURL: resolved.baseURL), attachedInstanceName: resolved.instanceName)
        let tile = Tile(name: "Terminal \(nextNumber)", session: session)
        nextNumber += 1
        tiles.append(tile)
        selectedId = tile.id
    }

    /// Shows a terminal that already runs on the computer (a row in the Code
    /// sessions view, or the terminal a Move / copy / new just started): the
    /// tile already attached to it, else a new tile attached by its session
    /// id. Nothing new starts on the computer. False when no computer is
    /// reached over the relay (only the terminal service lists sessions).
    @discardableResult
    func attach(sessionId: String, title: String) async -> Bool {
        if let tile = tiles.first(where: { $0.session.ptyId == sessionId }) {
            selectedId = tile.id
            return true
        }
        await InstanceStore.shared.refreshIfNeeded()
        guard let resolved = await InstanceConnection.resolve(from: InstanceStore.shared), resolved.isRelay else {
            return false
        }
        isOffline = false
        if tiles.count >= Self.maxTiles, let oldest = tiles.first {
            // Full: let go of the oldest tab without ending its shell (it is
            // still listed in the sessions view and can be reopened).
            tiles.removeFirst()
            agentNames[oldest.id] = nil
            oldest.session.stop()
        }
        let session = PtySession(service: TerminalServiceClient(baseURL: resolved.baseURL),
                                 attachedInstanceName: resolved.instanceName,
                                 sessionId: sessionId)
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        let tile = Tile(name: name.isEmpty ? "Terminal \(nextNumber)" : name, session: session)
        if name.isEmpty { nextNumber += 1 }
        tiles.append(tile)
        selectedId = tile.id
        return true
    }

    /// Closes a shell: detach and kill its pty server-side.
    func closeTile(_ id: UUID) {
        guard let index = tiles.firstIndex(where: { $0.id == id }) else { return }
        let tile = tiles.remove(at: index)
        agentNames[id] = nil
        tile.session.close()
        if selectedId == id { selectedId = tiles.last?.id }
    }

    /// The live pty of the selected shell (the Browser pane's dev-server ports).
    var livePtyId: String? { tiles.compactMap(\.session.ptyId).first }
}

// MARK: - Terminal bots

@MainActor
final class TerminalBotsStore: ObservableObject {
    @Published private(set) var bots: [TerminalBot] = []
    @Published private(set) var error: FactoryError? = nil
    @Published private(set) var engineNote: String? = nil
    @Published private(set) var isLoading = true

    private let client = FactoryPaneClient()

    func reload() async {
        do {
            let result = try await client.terminalBots()
            bots = result.bots
            error = nil
            engineNote = result.engineRunning ? nil : (result.engineError ?? "")
        } catch let factoryError as FactoryError {
            error = factoryError
        } catch {
            // Cancelled: keep what is on screen.
        }
        isLoading = false
    }

    /// No event stream on this surface: refresh every minute while shown
    /// (the web's fallback cadence when its event stream is down).
    func poll() async {
        while !Task.isCancelled {
            await reload()
            try? await Task.sleep(for: .seconds(60))
        }
    }
}

// MARK: - Terminal screen

/// The phone terminal (PhoneCodeTerminal.tsx, the Code screen's terminal on
/// the ai.allternit.com phone layout and the m.allternit.com PWA): the bot
/// strip on top picks what it shows, "Shell" (the shell sessions) or a
/// Terminal bot's engine pane. The shell stays mounted while a bot is shown,
/// so its session and scrollback are there on the way back.
struct PhoneCodeTerminal: View {
    @ObservedObject var store: CodeTerminalStore
    /// The computer's own shell only: no bot strip, no engine banner.
    var shellOnly = false
    @StateObject private var botsStore = TerminalBotsStore()
    @State private var botId: String? = nil
    @State private var isConnectPresented = false

    private var bot: TerminalBot? { botId.flatMap { id in botsStore.bots.first { $0.id == id } } }

    var body: some View {
        VStack(spacing: 0) {
            if !shellOnly { botStrip }
            ZStack {
                shell
                    .opacity(bot == nil ? 1 : 0)
                    .allowsHitTesting(bot == nil)
                if let bot {
                    PhoneBotPane(bot: bot)
                        .id(bot.id)
                }
            }
        }
        .background(TerminalPalette.background)
        .task { await store.startIfNeeded() }
        .task { if !shellOnly { await botsStore.poll() } }
        .onChange(of: botsStore.bots) { _, bots in
            // The chosen bot left (stopped, or its pane went away): back to Shell.
            if let botId, !bots.contains(where: { $0.id == botId }), botsStore.error == nil { self.botId = nil }
        }
        .sheet(isPresented: $isConnectPresented) {
            ConnectComputerSheet()
        }
    }

    // MARK: Bot strip

    private var botStrip: some View {
        VStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    stripButton(selected: botId == nil, action: { botId = nil }) {
                        Image(systemName: "apple.terminal")
                            .font(.system(size: 14))
                        Text("Shell")
                    }
                    ForEach(botsStore.bots) { bot in
                        stripButton(selected: botId == bot.id, action: { botId = bot.id }) {
                            Text(bot.initial)
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundColor(TerminalPalette.secondary)
                                .frame(width: 22, height: 22)
                                .background(TerminalPalette.keyPressed)
                                .clipShape(Circle())
                            Text(bot.name)
                                .lineLimit(1)
                                .truncationMode(.tail)
                            Circle()
                                .fill(Self.stateColor(bot.state))
                                .frame(width: 8, height: 8)
                        }
                        .accessibilityLabel("\(bot.name), \(bot.address), \(bot.stateLabel)")
                    }
                    if !botsStore.isLoading, botsStore.error == nil, botsStore.bots.isEmpty {
                        (Text("No Terminal bots. Start them with ") + Text("gizzi agents up").font(.system(size: 12, design: .monospaced)))
                            .font(.system(size: 12))
                            .foregroundColor(TerminalPalette.secondary)
                            .lineLimit(1)
                            .padding(.horizontal, 6)
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 4)
            }
            if let error = botsStore.error {
                note(error.engineMissing ? FactoryError.engineMissingText : error.fact)
            } else if let engineNote = botsStore.engineNote {
                note("The Factory can’t read its terminal panes right now.\(engineNote.isEmpty ? "" : " \(engineNote)")")
            }
        }
        .background(TerminalPalette.background)
        .overlay(alignment: .bottom) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Terminal: shell or a Terminal bot")
    }

    private func stripButton<Label: View>(selected: Bool, action: @escaping () -> Void, @ViewBuilder label: () -> Label) -> some View {
        Button(action: action) {
            HStack(spacing: 6) { label() }
                .font(.system(size: 13, weight: selected ? .semibold : .regular))
                .foregroundColor(TerminalPalette.foreground)
                .padding(.leading, 8)
                .padding(.trailing, 12)
                .frame(minWidth: 44, minHeight: 36)
                .frame(maxWidth: 180)
                .background(selected ? TerminalPalette.keyPressed : TerminalPalette.key)
                .clipShape(Capsule())
                .overlay(Capsule().stroke(selected ? TerminalPalette.pressedBorder : .clear, lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func note(_ text: String) -> some View {
        HStack(spacing: 10) {
            Text(text)
                .font(.system(size: 12))
                .foregroundColor(TerminalPalette.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button("Try again") { Task { await botsStore.reload() } }
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(TerminalPalette.foreground)
                .frame(minHeight: 36)
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 4)
    }

    static func stateColor(_ state: String) -> SwiftUI.Color {
        switch state {
        case "working": return Theme.statusInfo
        case "blocked", "needs_you": return Theme.statusWarning
        case "done": return Theme.statusSuccess
        case "failed": return Theme.statusError
        default: return TerminalPalette.secondary
        }
    }

    // MARK: Shell

    private var shell: some View {
        VStack(spacing: 0) {
            // The tab row stays while offline (only + then), as on the web.
            tabs
            if store.isOffline {
                offline
            } else if let tile = store.selected {
                // A coding agent in the shell gets the native composer (AgentComposerBar).
                TerminalSessionView(session: tile.session, agentMode: true,
                                    onAgentFrame: { [weak store] frame in store?.noteAgent(frame?.agent, for: tile.id) })
                    .id(tile.id)
            } else if store.isStarting {
                Text("Starting terminals…")
                    .font(.system(size: 14))
                    .foregroundColor(TerminalPalette.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                VStack(spacing: 10) {
                    Text("No terminal sessions")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(TerminalPalette.foreground)
                    Text("Start a shell for this session.")
                        .font(.system(size: 13))
                        .foregroundColor(TerminalPalette.secondary)
                    Button("New terminal") { Task { await store.createTile() } }
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundColor(TerminalPalette.foreground)
                        .padding(.horizontal, 16)
                        .frame(minHeight: 44)
                        .background(TerminalPalette.key)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    /// `.phone-terminal-tabs`: one button per shell, New terminal, Close.
    private var tabs: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 2) {
                ForEach(store.tiles) { tile in
                    let selected = store.selected?.id == tile.id
                    Button(action: { store.selectedId = tile.id }) {
                        tabLabel { Text(store.displayName(tile)).font(.system(size: 15, design: .monospaced)) }
                            .background(selected ? TerminalPalette.keyPressed : TerminalPalette.key)
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(selected ? .isSelected : [])
                }
                Button(action: { Task { await store.createTile() } }) {
                    tabLabel { Image(systemName: "plus").font(.system(size: 16)) }
                        .background(TerminalPalette.key)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
                .buttonStyle(.plain)
                .disabled(store.isStarting || store.tiles.count >= CodeTerminalStore.maxTiles)
                .opacity(store.isStarting || store.tiles.count >= CodeTerminalStore.maxTiles ? 0.5 : 1)
                .accessibilityLabel("New terminal")
                if let selected = store.selected {
                    // Ending a shell kills its pty: press and hold, never a tap.
                    tabLabel { Image(systemName: "xmark").font(.system(size: 15)) }
                        .background(TerminalPalette.key)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .onLongPressGesture(minimumDuration: 1.2) {
                            UINotificationFeedbackGenerator().notificationOccurred(.success)
                            store.closeTile(selected.id)
                        }
                        .accessibilityLabel("Hold to end \(store.displayName(selected))")
                        .accessibilityAddTraits(.isButton)
                        .accessibilityAction { store.closeTile(selected.id) }
                }
            }
            .padding(4)
        }
        .background(TerminalPalette.strip)
        .overlay(alignment: .bottom) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
        .accessibilityLabel("Terminal sessions")
    }

    private func tabLabel<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        content()
            .foregroundColor(TerminalPalette.foreground)
            .padding(.horizontal, 8)
            .frame(minWidth: 44, minHeight: 44)
    }

    /// `.phone-terminal-offline`: the calm no-computer state (phone calm
    /// I2/I3): what it needs, and Connect, which opens the connect kit.
    private var offline: some View {
        VStack(spacing: 14) {
            Spacer()
            Image(systemName: "laptopcomputer")
                .font(.system(size: 26, weight: .medium))
                .foregroundColor(TerminalPalette.secondary)
            Text("Connect a computer to use Shell")
                .font(.system(size: 17, weight: .medium))
                .foregroundColor(TerminalPalette.foreground)
                .multilineTextAlignment(.center)
            Text("Shell is your computer's own command line. Connect one, or start Allternit Cloud, and it opens here.")
                .font(.system(size: 14))
                .foregroundColor(TerminalPalette.secondary)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 28)
            Button(action: { isConnectPresented = true }) {
                Text("Connect")
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(TerminalPalette.foreground)
                    .padding(.horizontal, 22)
                    .frame(minHeight: 44)
                    .background(TerminalPalette.key)
                    .clipShape(Capsule())
                    .overlay(Capsule().stroke(TerminalPalette.keyBorder, lineWidth: 1))
            }
            .buttonStyle(.plain)
            Spacer()
            Spacer()
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(12)
        .task(id: isConnectPresented) {
            // Back from the computer picker: try again.
            if !isConnectPresented { await store.startIfNeeded() }
        }
    }
}

// MARK: - Agent composer

/// The Code terminal's composer while a coding agent (Claude Code, Codex,
/// Kimi, Antigravity, opencode, Gizzi, …) runs in the shell: the agent's own
/// input box is hidden and this drives it (agent-composer.tsx, and the phone's
/// `AgentKeys` in PhoneCodeTerminal.tsx). Its question as answer buttons,
/// agent · status with Stop while it works, its mode (tap = shift+tab), and a
/// message line in the Code prompt's look that pastes into the agent.
/// The accessory keys under it still type into the agent.
struct AgentComposerBar: View {
    let frame: AgentFrame
    @ObservedObject var session: PtySession
    @State private var text = ""
    @FocusState private var isFocused: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let ink = CodeInk(terminal: true)
    /// phone.css: prompt 15px and text 16px, both on a 22px line (CodeLaunchComposer).
    private static let line: CGFloat = 22
    private static let textFont = UIFont.monospacedSystemFont(ofSize: 16, weight: .regular)

    private var trimmed: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var agentName: String { frame.agent ?? "Agent" }

    var body: some View {
        VStack(spacing: 0) {
            if let dialog = frame.dialog {
                question(dialog)
            }
            edge
            DashedHairline(strong: isFocused, terminal: true)
            prompt
            DashedHairline(strong: isFocused, terminal: true)
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 4)
        .background(TerminalPalette.background)
        .overlay(alignment: .top) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(agentName) composer")
    }

    // MARK: Status edge

    private var statusColor: SwiftUI.Color {
        switch frame.status {
        case .idle: return Theme.statusSuccess
        case .working: return GizziCodeWordmark.coral
        case .needsYou: return Theme.statusWarning
        }
    }

    /// Agent · status, Stop while it works; the agent's own mode on the right.
    private var edge: some View {
        HStack(spacing: 6) {
            Text(agentName)
                .foregroundColor(ink.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
            Text("·").foregroundColor(ink.tertiary).accessibilityHidden(true)
            HStack(spacing: 4) {
                Image(systemName: "circle.fill")
                    .font(.system(size: 6))
                    .symbolEffect(.pulse, options: .repeating, isActive: frame.status == .working && !reduceMotion)
                    .accessibilityHidden(true)
                Text(AgentComposerText.status(frame.status))
            }
            .foregroundColor(statusColor)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(agentName): \(AgentComposerText.status(frame.status))")
            .accessibilityAddTraits(.updatesFrequently)
            if frame.status == .working {
                Button(action: { session.send(AgentKeys.escape) }) {
                    HStack(spacing: 6) {
                        Text("Stop").font(.system(size: 12, weight: .semibold, design: .monospaced))
                        Text("esc").font(.system(size: 10, design: .monospaced)).foregroundColor(ink.tertiary)
                    }
                    .foregroundColor(ink.primary)
                    .padding(.horizontal, 8)
                    .frame(height: 26)
                    .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(ink.border, lineWidth: 1))
                    .frame(minWidth: 44, minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop \(agentName)")
                .accessibilityHint("Sends Escape")
            }
            Spacer(minLength: 8)
            if let mode = frame.mode {
                Button(action: { session.send(AgentKeys.cycleMode) }) {
                    Text(AgentComposerText.shortMode(mode))
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundColor(ink.accent)
                        .lineLimit(1)
                        .padding(.horizontal, 6)
                        .frame(minWidth: 44, minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Mode: \(mode)")
                .accessibilityHint("Switches the agent's mode (shift+tab)")
            }
        }
        .font(.system(size: 11, design: .monospaced))
        .frame(minHeight: 44)
    }

    // MARK: Question

    /// A permission / choice question from the agent, one button per answer.
    private func question(_ dialog: AgentDialog) -> some View {
        let isPermission = AgentComposerText.isPermission(dialog.question)
        return VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: isPermission ? "lock.fill" : "questionmark.circle.fill")
                    .font(.system(size: 12))
                    .foregroundColor(ink.accent)
                    .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 2) {
                    Text(isPermission ? "Permission required" : "\(frame.agent ?? "The agent") asks")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(ink.primary)
                    if !dialog.question.isEmpty {
                        Text(dialog.question)
                            .font(.system(size: 13))
                            .foregroundColor(ink.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .accessibilityElement(children: .combine)
            if dialog.options.count > 4 {
                ScrollView { answers(dialog) }
                    .frame(maxHeight: 196)
            } else {
                answers(dialog)
            }
        }
        .padding(.top, 10)
        .padding(.bottom, 4)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(dialog.question.isEmpty ? "Agent question" : dialog.question)
    }

    private func answers(_ dialog: AgentDialog) -> some View {
        VStack(spacing: 4) {
            ForEach(dialog.options, id: \.index) { option in
                Button(action: { session.send(AgentKeys.dialogKeys(dialog.options, pick: option)) }) {
                    HStack(spacing: 8) {
                        Text(AgentComposerText.shortLabel(option.label))
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundColor(option.selected ? ink.primary : ink.secondary)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                        Spacer(minLength: 4)
                        if !option.key.isEmpty {
                            Text(option.key)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundColor(ink.tertiary)
                        }
                    }
                    .padding(.horizontal, 12)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .background(option.selected ? ink.accent.opacity(0.14) : ink.fill,
                                in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(option.selected ? ink.accent.opacity(0.36) : ink.border, lineWidth: 1))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(option.label)
                .accessibilityHint(option.key.isEmpty ? "Picks this answer" : "Picks this answer (\(option.key))")
                .accessibilityAddTraits(option.selected ? .isSelected : [])
            }
        }
    }

    // MARK: Prompt

    private var prompt: some View {
        HStack(alignment: .bottom, spacing: 4) {
            HStack(alignment: .top, spacing: 8) {
                Text("❯")
                    .font(.system(size: 15, weight: .bold, design: .monospaced))
                    .foregroundColor(GizziCodeWordmark.coral)
                    .frame(height: Self.line)
                    .accessibilityHidden(true)
                let placeholder = AgentComposerText.placeholder(frame)
                TextField(placeholder, text: $text, prompt: Text(placeholder).foregroundColor(ink.tertiary), axis: .vertical)
                    .font(Font(Self.textFont))
                    .foregroundColor(ink.primary)
                    .tint(GizziCodeWordmark.coral)
                    .lineSpacing(Self.line - Self.textFont.lineHeight)
                    .padding(.vertical, (Self.line - Self.textFont.lineHeight) / 2)
                    .lineLimit(1...4)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.send)
                    .focused($isFocused)
                    .onSubmit(send)
                    .accessibilityLabel(placeholder)
            }
            .padding(.vertical, 9)
            .padding(.leading, 8)
            .padding(.trailing, 4)
            .frame(maxWidth: .infinity, alignment: .leading)
            if !trimmed.isEmpty {
                Button(action: send) {
                    Image(systemName: "return")
                        .font(.system(size: 15, weight: .bold))
                        .foregroundColor(.white)
                        .frame(width: 40, height: 40)
                        .background(GizziCodeWordmark.coral, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                }
                .buttonStyle(.plain)
                .padding(.vertical, 6)
                .accessibilityLabel("Send to \(agentName)")
            }
        }
    }

    /// Bracketed paste, then Enter 60 ms later (CodeTerminalCanvas `send`), so a
    /// multi-line message stays one prompt.
    private func send() {
        let value = trimmed
        guard !value.isEmpty else { return }
        text = ""
        let keys = AgentKeys.promptInput(value)
        session.send(keys.paste)
        Task { @MainActor [weak session] in
            try? await Task.sleep(for: .milliseconds(60))
            session?.send(keys.enter)
        }
    }
}

// MARK: - Bot pane

/// A Terminal bot's live engine pane (lib/factory/panes.ts): the screen
/// arrives as ANSI over `GET /agents/:id/stream`; keystrokes go to
/// `POST /agents/:id/input`.
@MainActor
final class PaneMirror: ObservableObject {
    enum Status: Equatable { case connecting, live, gone(String?), error(FactoryError) }

    @Published private(set) var status: Status = .connecting
    @Published private(set) var hasScreen = false
    private weak var view: TerminalView?
    private var lastScreen: String?
    private var task: Task<Void, Never>?
    private let client = FactoryPaneClient()
    /// Keystrokes typed while a send is in flight; they go out together in
    /// the next one, so keys can't overtake each other on the way to the pane.
    private var pendingInput = ""
    private var isSendingInput = false
    let agentId: String

    init(agentId: String) {
        self.agentId = agentId
    }

    var label: String {
        switch status {
        case .connecting: return "Connecting"
        case .live: return "Live"
        case .gone: return "Pane closed"
        case .error: return "Not connected"
        }
    }

    func attach(_ view: TerminalView) {
        self.view = view
        if let lastScreen { draw(lastScreen) }
    }

    func start() {
        task?.cancel()
        status = .connecting
        task = Task { [weak self] in
            guard let self else { return }
            do {
                for try await event in self.client.screenStream(agentId: self.agentId) {
                    switch event {
                    case .screen(let ansi):
                        self.lastScreen = ansi
                        self.hasScreen = true
                        self.status = .live
                        self.draw(ansi)
                    case .gone(let reason):
                        self.status = .gone(reason)
                    }
                }
            } catch let error as FactoryError {
                self.status = .error(error)
            } catch {}
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func send(_ data: String) {
        pendingInput += data
        flushInput()
    }

    /// Sends what was typed, in order, one send at a time.
    private func flushInput() {
        guard !isSendingInput, !pendingInput.isEmpty else { return }
        let data = pendingInput
        pendingInput = ""
        isSendingInput = true
        let agentId = self.agentId
        let client = self.client
        Task { [weak self] in
            for batch in FactoryPaneClient.paneInput(from: data) {
                try? await client.sendInput(agentId: agentId, text: batch.text, keys: batch.keys)
            }
            guard let self else { return }
            self.isSendingInput = false
            self.flushInput()
        }
    }

    /// The pane's visible screen replaces what is drawn.
    private func draw(_ ansi: String) {
        let lines = ansi.replacingOccurrences(of: "\r\n", with: "\n").replacingOccurrences(of: "\n", with: "\r\n")
        view?.feed(text: "\u{1b}[0m\u{1b}[2J\u{1b}[H" + lines)
    }
}

/// A Terminal bot's pane on the phone: compact header, the mirrored screen,
/// the accessory keys (PhoneCodeTerminal.tsx `PhoneBotPane`).
private struct PhoneBotPane: View {
    let bot: TerminalBot
    @StateObject private var mirror: PaneMirror
    @StateObject private var latch = TerminalCtrlLatch()

    init(bot: TerminalBot) {
        self.bot = bot
        _mirror = StateObject(wrappedValue: PaneMirror(agentId: bot.id))
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            if let message {
                message
            } else {
                TerminalSurfaceView(
                    fontSize: 13,
                    attach: { view in mirror.attach(view) },
                    onInput: { [weak mirror, weak latch] text in mirror?.send(latch?.transform(text) ?? text) },
                    onResize: { _, _ in }
                )
                .padding(6)
                TerminalAccessoryKeys(latch: latch) { data in mirror.send(data) }
            }
        }
        .background(TerminalPalette.background)
        .onAppear { if bot.pane?.attachable == true { mirror.start() } }
        .onDisappear { mirror.stop() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(bot.address) terminal")
    }

    private var header: some View {
        HStack(spacing: 8) {
            Text(bot.initial)
                .font(.system(size: 9, weight: .semibold))
                .foregroundColor(TerminalPalette.secondary)
                .frame(width: 20, height: 20)
                .background(TerminalPalette.keyPressed)
                .clipShape(Circle())
            VStack(alignment: .leading, spacing: 0) {
                Text(bot.name)
                    .font(.system(size: 12.5, weight: .semibold))
                    .foregroundColor(TerminalPalette.foreground)
                Text(bot.address)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(TerminalPalette.secondary)
                if let machine = bot.remoteMachine {
                    Text("on \(machine)")
                        .font(.system(size: 11))
                        .foregroundColor(TerminalPalette.secondary)
                }
            }
            .lineLimit(1)
            Spacer(minLength: 4)
            HStack(spacing: 6) {
                Circle().fill(PhoneCodeTerminal.stateColor(bot.state)).frame(width: 8, height: 8)
                Text("\(bot.stateLabel) · \(mirror.label)")
            }
            .font(.system(size: 11))
            .foregroundColor(TerminalPalette.secondary)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .background(TerminalPalette.strip)
        .overlay(alignment: .bottom) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
    }

    /// PaneStateMessage: not attachable, gone, error, or connecting.
    private var message: AnyView? {
        if bot.pane?.attachable != true {
            return AnyView(messageShell {
                Text("\(bot.name)’s terminal pane isn’t running.")
                    .fontWeight(.medium)
                    .foregroundColor(TerminalPalette.foreground)
                (Text("Start it with ") + Text("gizzi agents up").font(.system(size: 12, design: .monospaced)) + Text("."))
            })
        }
        switch mirror.status {
        case .gone(let reason):
            return AnyView(messageShell {
                Text("The pane closed.").fontWeight(.medium).foregroundColor(TerminalPalette.foreground)
                if let reason { Text(reason) }
                reconnect
            })
        case .error(let error):
            return AnyView(messageShell {
                Text(error.engineMissing ? FactoryError.engineMissingText : error.fact)
                    .fontWeight(.medium)
                    .foregroundColor(TerminalPalette.foreground)
                Text(error.action ?? "Start Allternit Desktop or Gizzi on this computer, then try again.")
                reconnect
            })
        case .connecting where !mirror.hasScreen:
            return AnyView(messageShell { Text("Connecting to \(bot.address)…") })
        default:
            return nil
        }
    }

    private var reconnect: some View {
        Button(action: { mirror.start() }) {
            Label("Reconnect", systemImage: "arrow.clockwise")
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(TerminalPalette.foreground)
                .padding(.horizontal, 12)
                .frame(minHeight: 32)
                .background(TerminalPalette.key)
                .clipShape(Capsule())
                .overlay(Capsule().stroke(TerminalPalette.keyBorder, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }

    private func messageShell<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 8) { content() }
            .font(.system(size: 12.5))
            .foregroundColor(TerminalPalette.secondary)
            .multilineTextAlignment(.center)
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
