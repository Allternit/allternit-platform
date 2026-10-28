import SwiftUI

// -----------------------------------------------------------------------------
// Live computer drive for ANY computer (allternit-ai src/components/
// computers/ComputerViewport.tsx + lib/computers-control.ts): the control
// lease decides who drives (`/api/v1/computers/:id/control[/take|/release]`),
// a short-lived VNC token is minted per connect (`…/ws-token`, read-only
// unless this device holds the lease — server-enforced), and the stream is
// `/ws/computers/:id/vnc` rendered by the existing native RFB stack
// (Features/Agents/Desktop: BotDesktopSession + DesktopInteractionOverlay).
// A hand-off reconnects, because control is decided when the stream opens.
// -----------------------------------------------------------------------------

struct ComputersListView: View {
    @State private var computers: [ComputerRecord] = []
    @State private var isLoading = true
    @State private var error: String? = nil

    var body: some View {
        List {
            if isLoading && computers.isEmpty {
                ProgressView().frame(maxWidth: .infinity)
            } else if let error {
                FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load computers",
                                  message: error, actionTitle: "Retry", action: { Task { await load() } })
            } else if computers.isEmpty {
                FriendlyStateView(style: .empty, icon: "desktopcomputer", title: "No computers yet",
                                  message: "Pair a Mac with Allternit Desktop or add a cloud computer in Settings → Computers.")
            } else {
                ForEach(computers) { computer in
                    NavigationLink(value: BotRoute.computer(computer)) {
                        ComputerRowLabel(computer: computer)
                    }
                }
            }
        }
        .navigationTitle("Computers")
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        do {
            computers = try await BotModeClient.shared.listComputers()
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }
}

struct ComputerRowLabel: View {
    let computer: ComputerRecord

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: computer.os?.lowercased().contains("mac") == true ? "laptopcomputer" : "desktopcomputer")
                .frame(width: 26)
                .foregroundColor(Color("TextSecondary"))
            VStack(alignment: .leading, spacing: 2) {
                Text(computer.name).lineLimit(1)
                Text([computer.kindLabel, computer.os].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Spacer()
            Circle()
                .fill(computer.isRunning ? Theme.statusSuccess : Color("TextTertiary"))
                .frame(width: 8, height: 8)
                .accessibilityLabel(computer.isRunning ? "Running" : computer.status)
        }
    }
}

struct ComputerDriveView: View {
    let computer: ComputerRecord

    @State private var running: Bool
    @State private var lease: ComputerControlLease? = nil
    @State private var wsPath: String? = nil
    @State private var error: String? = nil
    @State private var isBusy = false
    @State private var isFullScreen = false

    init(computer: ComputerRecord) {
        self.computer = computer
        _running = State(initialValue: computer.isRunning)
    }

    private var holdsControl: Bool {
        guard let lease else { return false }
        return lease.holder.kind == "user" && lease.holder.deviceId == BotModeClient.deviceId
    }

    var body: some View {
        VStack(spacing: 0) {
            controlBar
            ZStack {
                Color.black
                if !running {
                    FriendlyStateView(style: .offline, icon: "power", title: "\(computer.name) is off",
                                      message: "Start it to see its screen.",
                                      actionTitle: isBusy ? nil : "Start", action: isBusy ? nil : { start() })
                } else if let wsPath {
                    ComputerScreenCanvas(wsPath: wsPath, controls: holdsControl)
                        .id(wsPath)
                } else if let error {
                    FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't connect",
                                      message: error, actionTitle: "Try again", action: { Task { await connect() } })
                } else {
                    ProgressView().tint(.white)
                }
            }
        }
        .navigationTitle(computer.name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button { isFullScreen = true } label: { Image(systemName: "arrow.up.left.and.arrow.down.right") }
                    .disabled(wsPath == nil)
                    .accessibilityLabel("Full screen")
            }
        }
        .fullScreenCover(isPresented: $isFullScreen) {
            if let wsPath {
                NavigationStack {
                    ComputerScreenCanvas(wsPath: wsPath, controls: holdsControl)
                        .ignoresSafeArea(edges: .bottom)
                        .navigationTitle(computer.name)
                        .navigationBarTitleDisplayMode(.inline)
                        .toolbar {
                            ToolbarItem(placement: .topBarLeading) { Button("Close") { isFullScreen = false } }
                        }
                }
            }
        }
        .task { await connect() }
        .task {
            // Lease changes (someone took control, ours expired) reconnect
            // the stream with the right access.
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                guard running, let state = try? await BotModeClient.shared.controlState(computerId: computer.id) else { continue }
                if state.control?.holder != lease?.holder {
                    lease = state.control
                    await connect(refreshLease: false)
                }
            }
        }
    }

    private var controlBar: some View {
        HStack(spacing: 10) {
            Image(systemName: holdsControl ? "hand.point.up.left.fill" : "eye")
                .foregroundColor(holdsControl ? Color("AccentPrimary") : Color("TextSecondary"))
            Text(controlLabel)
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(1)
            Spacer()
            if running {
                if holdsControl {
                    Button("Release") { release() }.buttonStyle(.bordered)
                } else {
                    Button("Take control") { take() }.buttonStyle(.borderedProminent)
                }
            }
        }
        .font(.subheadline)
        .disabled(isBusy)
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(Color("BgSecondary"))
    }

    private var controlLabel: String {
        guard running else { return "Off" }
        guard let lease else { return "Nobody is driving — view only" }
        if holdsControl { return "You're driving" }
        return "\(lease.holder.displayLabel) is driving — view only"
    }

    private func connect(refreshLease: Bool = true) async {
        guard running else { return }
        error = nil
        do {
            if refreshLease {
                lease = try await BotModeClient.shared.controlState(computerId: computer.id).control
            }
            let status = try await BotModeClient.shared.viewStatus(computerId: computer.id)
            var token: String
            do {
                token = try await BotModeClient.shared.vncToken(computerId: computer.id, readOnly: !holdsControl)
            } catch where holdsControl {
                // Control moved between the lease read and now: watch instead.
                token = try await BotModeClient.shared.vncToken(computerId: computer.id, readOnly: true)
            }
            let encoded = BotModeClient.query(token)
            let separator = status.ws.vnc.contains("?") ? "&" : "?"
            wsPath = "\(status.ws.vnc)\(separator)token=\(encoded)"
        } catch is CancellationError {
        } catch {
            wsPath = nil
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    private func take() {
        isBusy = true
        Task {
            defer { isBusy = false }
            do {
                try await BotModeClient.shared.takeControl(computerId: computer.id)
                await connect()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }

    private func release() {
        isBusy = true
        Task {
            defer { isBusy = false }
            try? await BotModeClient.shared.releaseControl(computerId: computer.id)
            await connect()
        }
    }

    private func start() {
        isBusy = true
        Task {
            defer { isBusy = false }
            do {
                try await BotModeClient.shared.startComputer(id: computer.id)
                running = true
                await connect()
            } catch {
                self.error = FriendlyErrorMessage.from(error.localizedDescription)
            }
        }
    }
}

/// The VNC canvas: aspect-fit framebuffer, pinch zoom + pan, tap = click,
/// two-finger tap / long press = right click, drag = pointer drag, and a
/// keyboard while driving. Same mapping as BotDesktopView.
struct ComputerScreenCanvas: View {
    let controls: Bool

    @StateObject private var session: BotDesktopSession
    @State private var userScale: CGFloat = 1
    @GestureState private var magnifyDelta: CGFloat = 1
    @State private var pan: CGSize = .zero
    @State private var panAtDragStart: CGSize? = nil
    @State private var isPointerDragging = false
    @State private var isKeyboardVisible = false

    init(wsPath: String, controls: Bool) {
        self.controls = controls
        _session = StateObject(wrappedValue: BotDesktopSession(
            wsPath: wsPath,
            controlState: controls ? .humanControls : .humanObserving
        ))
    }

    var body: some View {
        GeometryReader { geometry in
            ZStack {
                Color.black
                if let frame = session.frame {
                    let rect = displayedRect(in: geometry.size)
                    Image(decorative: frame, scale: 1)
                        .resizable()
                        .frame(width: rect.width, height: rect.height)
                        .position(x: rect.midX, y: rect.midY)
                }
                DesktopInteractionOverlay(
                    isKeyboardVisible: isKeyboardVisible && session.allowsInput,
                    onTap: { point in tap(point, mask: 1, size: geometry.size) },
                    onRightClick: { point in tap(point, mask: 4, size: geometry.size) },
                    onKeysym: { keysym, down in session.sendKey(keysym: keysym, down: down) }
                )
                overlay
                if session.allowsInput && session.state == .live {
                    VStack {
                        Spacer()
                        HStack {
                            Spacer()
                            Button { isKeyboardVisible.toggle() } label: {
                                Image(systemName: isKeyboardVisible ? "keyboard.chevron.compact.down" : "keyboard")
                                    .padding(10)
                                    .background(.ultraThinMaterial)
                                    .clipShape(Circle())
                            }
                            .accessibilityLabel(isKeyboardVisible ? "Hide keyboard" : "Show keyboard")
                        }
                    }
                    .padding(14)
                }
            }
            .contentShape(Rectangle())
            .gesture(magnification)
            .gesture(drag(in: geometry.size))
        }
        .onChange(of: controls) { _, newValue in
            session.controlState = newValue ? .humanControls : .humanObserving
        }
        .task { session.start() }
        .onDisappear { session.stop() }
    }

    @ViewBuilder
    private var overlay: some View {
        switch session.state {
        case .connecting:
            status { ProgressView().tint(.white); Text("Connecting…") }
        case .reconnecting(let attempt):
            status { ProgressView().tint(.white); Text("Reconnecting (\(attempt)/\(BotDesktopSession.maxReconnectAttempts))…") }
        case .failed(let message):
            status {
                Text("Couldn't show the screen").font(.headline)
                Text(message).font(.caption).multilineTextAlignment(.center)
                Button("Retry") { session.retry() }.buttonStyle(.borderedProminent)
            }
        case .live:
            if session.frame == nil {
                status { ProgressView().tint(.white); Text("Waiting for the first frame…") }
            }
        }
    }

    private func status<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        VStack(spacing: 10) { content() }
            .foregroundColor(.white)
            .padding(28)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Color.black.opacity(0.85))
    }

    private func displayedRect(in size: CGSize) -> CGRect {
        let fb = session.framebufferSize
        guard fb.width > 0, fb.height > 0 else { return .zero }
        let scale = min(size.width / fb.width, size.height / fb.height) * userScale * magnifyDelta
        let w = fb.width * scale, h = fb.height * scale
        return CGRect(x: (size.width - w) / 2 + pan.width, y: (size.height - h) / 2 + pan.height, width: w, height: h)
    }

    private func framebufferPoint(_ p: CGPoint, size: CGSize) -> CGPoint? {
        let rect = displayedRect(in: size)
        let fb = session.framebufferSize
        guard rect.width > 0, rect.height > 0 else { return nil }
        let x = (p.x - rect.minX) / rect.width * fb.width
        let y = (p.y - rect.minY) / rect.height * fb.height
        guard x >= 0, y >= 0, x < fb.width, y < fb.height else { return nil }
        return CGPoint(x: x, y: y)
    }

    private func tap(_ p: CGPoint, mask: UInt8, size: CGSize) {
        guard let point = framebufferPoint(p, size: size) else { return }
        session.tap(x: Int(point.x), y: Int(point.y), buttonMask: mask)
    }

    private var magnification: some Gesture {
        MagnifyGesture()
            .updating($magnifyDelta) { value, state, _ in state = value.magnification }
            .onEnded { value in
                userScale = min(8, max(1, userScale * value.magnification))
                if userScale <= 1.02 { userScale = 1; pan = .zero }
            }
    }

    private func drag(in size: CGSize) -> some Gesture {
        DragGesture()
            .onChanged { value in
                if userScale * magnifyDelta > 1.02 {
                    if panAtDragStart == nil { panAtDragStart = pan }
                    if let base = panAtDragStart {
                        pan = CGSize(width: base.width + value.translation.width, height: base.height + value.translation.height)
                    }
                } else if session.allowsInput {
                    if !isPointerDragging {
                        isPointerDragging = true
                        if let start = framebufferPoint(value.startLocation, size: size) {
                            session.sendPointer(x: Int(start.x), y: Int(start.y), buttonMask: 1)
                        }
                    }
                    if let point = framebufferPoint(value.location, size: size) {
                        session.sendPointer(x: Int(point.x), y: Int(point.y), buttonMask: 1)
                    }
                }
            }
            .onEnded { value in
                if isPointerDragging, let point = framebufferPoint(value.location, size: size) {
                    session.sendPointer(x: Int(point.x), y: Int(point.y), buttonMask: 0)
                }
                isPointerDragging = false
                panAtDragStart = nil
            }
    }
}

/// A bot's own computer from its page: the bot desktop sandbox
/// (BotDesktopStore — observe / take over / hand back, existing native VNC
/// viewer) plus any `/api/v1/computers?bot_id=` computers it is assigned.
struct BotComputerView: View {
    let bot: AgentRecord

    @ObservedObject private var desktop = BotDesktopStore.shared
    @State private var computers: [ComputerRecord] = []
    @State private var viewing: BotDesktopStatus? = nil

    var body: some View {
        let entry = desktop.entry(for: bot.id)
        List {
            Section("\(bot.botDisplayName)'s desktop") {
                if entry?.isLoading == true && entry?.status == nil {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let status = entry?.status, status.status == .running {
                    HStack {
                        Label("Running", systemImage: "circle.fill")
                            .foregroundColor(Theme.statusSuccess)
                        Spacer()
                        Text(status.controlState.label)
                            .font(.caption)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    if status.wsURL != nil {
                        Button("View desktop") { viewing = status }
                    }
                    switch status.controlState {
                    case .humanControls:
                        Button("Hand back to \(bot.botDisplayName)") { Task { await desktop.handBack(botId: bot.id) } }
                    default:
                        Button("Take over") { Task { await desktop.takeOver(botId: bot.id) } }
                        if status.controlState != .humanObserving {
                            Button("Watch without pausing") { Task { await desktop.observe(botId: bot.id) } }
                        }
                    }
                } else {
                    Text("\(bot.botDisplayName) has no desktop running.")
                        .foregroundColor(Color("TextSecondary"))
                    Button(entry?.isProvisioning == true ? "Starting…" : "Start a desktop") {
                        Task { await desktop.provision(botId: bot.id) }
                    }
                    .disabled(entry?.isProvisioning == true)
                }
                if let error = entry?.error {
                    Text(error).font(.caption).foregroundColor(Theme.statusError)
                }
            }
            .disabled(entry?.isControlActionInFlight == true)

            if !computers.isEmpty {
                Section("Assigned computers") {
                    ForEach(computers) { computer in
                        NavigationLink(value: BotRoute.computer(computer)) { ComputerRowLabel(computer: computer) }
                    }
                }
            }
            Section {
                NavigationLink(value: BotRoute.computers) {
                    Label("All computers", systemImage: "desktopcomputer")
                }
            }
        }
        .navigationTitle("Computer")
        .refreshable { await reload() }
        .task { await reload() }
        .fullScreenCover(item: Binding(
            get: { viewing.map(IdentifiedDesktopStatus.init) },
            set: { viewing = $0?.status }
        )) { item in
            BotDesktopView(agent: bot, status: item.status)
        }
    }

    private func reload() async {
        desktop.refresh(botId: bot.id)
        computers = (try? await BotModeClient.shared.listComputers(botId: bot.id)) ?? []
    }
}

private struct IdentifiedDesktopStatus: Identifiable {
    let status: BotDesktopStatus
    var id: String { status.sandboxId }
}
