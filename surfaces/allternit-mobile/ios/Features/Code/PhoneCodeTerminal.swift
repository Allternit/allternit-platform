import SwiftUI
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
    private var nextNumber = 1

    var selected: Tile? { tiles.first { $0.id == selectedId } ?? tiles.first }

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

    /// Closes a shell: detach and kill its pty server-side.
    func closeTile(_ id: UUID) {
        guard let index = tiles.firstIndex(where: { $0.id == id }) else { return }
        let tile = tiles.remove(at: index)
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
    @StateObject private var botsStore = TerminalBotsStore()
    @State private var botId: String? = nil
    @State private var isConnectPresented = false

    private var bot: TerminalBot? { botId.flatMap { id in botsStore.bots.first { $0.id == id } } }

    var body: some View {
        VStack(spacing: 0) {
            botStrip
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
        .task { await botsStore.poll() }
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
                note(error.engineMissing ? "\(FactoryError.engineMissingText)." : error.fact)
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
                TerminalSessionView(session: tile.session)
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
                        tabLabel { Text(tile.name).font(.system(size: 15, design: .monospaced)) }
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
                    Button(action: { store.closeTile(selected.id) }) {
                        tabLabel { Image(systemName: "xmark").font(.system(size: 15)) }
                            .background(TerminalPalette.key)
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Close \(selected.name)")
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

    /// `.phone-terminal-offline`: the calm "not connected" prompt.
    private var offline: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("$ ").foregroundColor(TerminalPalette.secondary)
                + Text("Not connected — connect a computer or Allternit Cloud to run commands")
            Button(action: { isConnectPresented = true }) {
                Text("Connect")
                    .padding(.horizontal, 16)
                    .frame(minHeight: 44)
                    .background(TerminalPalette.key)
                    .clipShape(RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(TerminalPalette.keyBorder, lineWidth: 1))
            }
            .buttonStyle(.plain)
            .padding(.vertical, 10)
            HStack(spacing: 0) {
                Text("$ ")
                Rectangle()
                    .fill(TerminalPalette.foreground.opacity(0.7))
                    .frame(width: 8, height: 17)
            }
            .accessibilityHidden(true)
            Spacer()
        }
        .font(.system(size: 14, design: .monospaced))
        .lineSpacing(4)
        .foregroundColor(TerminalPalette.foreground)
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .task(id: isConnectPresented) {
            // Back from the computer picker: try again.
            if !isConnectPresented { await store.startIfNeeded() }
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
                Text(error.engineMissing ? "\(FactoryError.engineMissingText)." : error.fact)
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
