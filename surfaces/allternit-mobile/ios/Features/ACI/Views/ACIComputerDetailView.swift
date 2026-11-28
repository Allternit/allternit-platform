import SwiftUI
import AllternitFilesKit

/// A computer's detail (design 4A): Desktop, Files and Inbox requests rows,
/// then its status. Files is a row that pushes the Files screens; there is
/// no new rail tab. Shown only for computers with the Files drive on; a
/// computer without it still opens straight to its desktop.
///
/// iPhone: one navigation stack. iPad (regular width): a sidebar with
/// Desktop, the computer's folders and Inbox requests, and the folder in
/// the detail column. Files can be dragged out of a folder and dropped into
/// one (from other apps on iPad).
struct ACIComputerDetailCover: View {
    let computerId: ComputerId
    let name: String
    var startScan = false

    @StateObject private var model: FilesComputerModel
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @Environment(\.horizontalSizeClass) private var sizeClass
    @Environment(\.dismiss) private var dismiss

    @State private var path = NavigationPath()
    @State private var detailPath = NavigationPath()
    @State private var sidebar: Sidebar?
    @State private var desktop: ComputerRecord?
    @State private var isResolvingDesktop = false
    @State private var isScreenPickerOpen = false
    @State private var isScannerOpen = false

    enum Sidebar: Hashable {
        case desktop
        case root(RootId)
        case requests
    }

    init(computerId: ComputerId, name: String, startScan: Bool = false) {
        self.computerId = computerId
        self.name = name
        self.startScan = startScan
        _model = StateObject(wrappedValue: FilesComputerModel(computerId: computerId, name: name))
    }

    private var device: RelayRuntimeDevice? { relay.devices.first { $0.id == computerId } }

    var body: some View {
        Group {
            if sizeClass == .regular { split } else { stack }
        }
        .tint(Color("TextPrimary"))
        .sheet(item: $model.wakePrompt) { prompt in FilesWakeSheet(model: model, prompt: prompt) }
        .sheet(isPresented: $isScreenPickerOpen) { ACIComputerScreenSheet() }
        .fullScreenCover(isPresented: $isScannerOpen) {
            FilesDocumentScanner { pdf in
                isScannerOpen = false
                if let pdf { Task { await model.saveScan(pdf) } }
            }
            .ignoresSafeArea()
        }
        .overlay(alignment: .bottom) {
            VStack(spacing: 8) {
                FilesTransferBar(model: model)
                ACIToast(message: noticeBinding)
            }
        }
        .task {
            await model.load()
            if startScan, FilesDocumentScanner.isSupported { isScannerOpen = true }
        }
    }

    private var noticeBinding: Binding<ACIToast.Message?> {
        Binding(get: { model.notice.map { ACIToast.Message(kind: .info, title: $0) } },
                set: { if $0 == nil { model.notice = nil } })
    }

    private var backButton: some View {
        Button { dismiss() } label: {
            HStack(spacing: 4) {
                Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                Text("Computers")
            }
        }
        .accessibilityLabel("Back to Computers")
    }

    // MARK: iPhone

    private var stack: some View {
        NavigationStack(path: $path) {
            List {
                Section {
                    Button { Task { await openDesktop() } } label: {
                        FilesRowLabel(systemImage: "display", title: "Desktop", subtitle: "View and control",
                                      trailing: AnyView(trailingChevron(busy: isResolvingDesktop)))
                    }
                    .buttonStyle(.plain)
                    NavigationLink(value: FilesRoute.roots) {
                        FilesRowLabel(systemImage: "folder", title: "Files", subtitle: filesLine)
                    }
                    if !model.pendingTasks.isEmpty {
                        NavigationLink(value: FilesRoute.requests) {
                            FilesRowLabel(systemImage: "checkmark.shield", title: "Inbox requests",
                                          subtitle: FilesRootsScreen.requestsLine(model.pendingTasks.count))
                        }
                    }
                }
                .listRowBackground(Color("FillNeutral"))
                Section("Status") {
                    LabeledContent("State", value: stateText)
                    LabeledContent("Last seen", value: lastSeenText)
                    if let wake = model.wakeSetting {
                        LabeledContent("Wake for Files", value: FilesFormat.wakeLabel(wake))
                    }
                }
                .listRowBackground(Color("FillNeutral"))
            }
            .filesListStyle()
            .navigationTitle(name)
            .toolbar { ToolbarItem(placement: .topBarLeading) { backButton } }
            .refreshable { await model.load() }
            .navigationDestination(for: FilesRoute.self) { route in destination(route) }
            .navigationDestination(for: ComputerRecord.self) { computer in ComputerDriveView(computer: computer) }
        }
    }

    // MARK: iPad

    private var split: some View {
        NavigationSplitView {
            List(selection: $sidebar) {
                Section {
                    Label("Desktop", systemImage: "display").tag(Sidebar.desktop)
                }
                Section("Files") {
                    ForEach(model.sortedRoots, id: \.id) { root in
                        Label(root.name, systemImage: FilesFormat.symbol(root.kind)).tag(Sidebar.root(root.id))
                            .onDrop(of: [.item], isTargeted: nil) { providers in
                                guard root.policy.write else { return false }
                                Task { await dropInto(root, providers) }
                                return true
                            }
                    }
                    if !model.pendingTasks.isEmpty {
                        Label("Inbox requests (\(model.pendingTasks.count))", systemImage: "checkmark.shield").tag(Sidebar.requests)
                    }
                }
                Section("Status") {
                    LabeledContent("State", value: stateText)
                    if let wake = model.wakeSetting { LabeledContent("Wake for Files", value: FilesFormat.wakeLabel(wake)) }
                }
            }
            .navigationTitle(name)
            .toolbar { ToolbarItem(placement: .topBarLeading) { backButton } }
            .refreshable { await model.load() }
        } detail: {
            NavigationStack(path: $detailPath) {
                detailRoot
                    .navigationDestination(for: FilesRoute.self) { route in destination(route) }
            }
        }
        .onChange(of: sidebar) { _, value in
            detailPath = NavigationPath()
            if value == .desktop, desktop == nil { Task { await resolveDesktop() } }
        }
        .onChange(of: model.roots) { _, roots in
            if sidebar == nil, let first = model.sortedRoots.first ?? roots.first { sidebar = .root(first.id) }
        }
    }

    @ViewBuilder
    private var detailRoot: some View {
        switch sidebar {
        case .desktop:
            if let desktop {
                ComputerDriveView(computer: desktop)
            } else if isResolvingDesktop {
                ProgressView()
            } else {
                FriendlyStateView(style: .empty, icon: "display", title: "Desktop isn't linked here",
                                  message: "Pick the computer's screen from the list.", actionTitle: "Choose a screen",
                                  action: { isScreenPickerOpen = true })
            }
        case .root(let id):
            if let root = model.root(id: id) {
                FilesFolderScreen(model: model, rootId: root.id, parentId: root.rootItemId, title: root.name)
                    .id(root.id)
            } else {
                FilesRootsScreen(model: model)
            }
        case .requests:
            FilesInboxRequestsView(model: model)
        case nil:
            FilesRootsScreen(model: model)
        }
    }

    // MARK: Shared

    @ViewBuilder
    private func destination(_ route: FilesRoute) -> some View {
        switch route {
        case .roots: FilesRootsScreen(model: model)
        case .folder(let rootId, let parentId, let title):
            FilesFolderScreen(model: model, rootId: rootId, parentId: parentId, title: title)
        case .requests: FilesInboxRequestsView(model: model)
        }
    }

    private func trailingChevron(busy: Bool) -> some View {
        Group {
            if busy { ProgressView() } else {
                Image(systemName: "chevron.right").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextTertiary"))
            }
        }
    }

    private var filesLine: String {
        let count = model.roots.count
        let folders = count == 1 ? "1 folder" : "\(count) folders"
        return count == 0 ? "Works while asleep" : "\(folders) · works while asleep"
    }

    private var stateText: String {
        if model.isAsleep { return "Asleep" }
        if let device { return device.isOnline ? "Online" : device.status.capitalized }
        return model.stateLine.isEmpty ? "Unknown" : model.stateLine
    }

    private var lastSeenText: String {
        guard let raw = device?.lastSeenAt, let date = ACIDates.parse(raw) else { return "Unknown" }
        return date.formatted(date: .abbreviated, time: .shortened)
    }

    private func dropInto(_ root: Root, _ providers: [NSItemProvider]) async {
        var urls: [URL] = []
        for provider in providers {
            if let url = await FilesComputerModel.copyToTemporary(provider) { urls.append(url) }
        }
        guard !urls.isEmpty else { return }
        await model.needingWake(.upload) { [model] in
            try await model.upload(urls, rootId: root.id, parentId: root.rootItemId)
            model.notice = urls.count == 1 ? "Saved to \(root.name)." : "\(urls.count) files saved to \(root.name)."
        }
    }

    /// The desktop viewer is keyed by the computers registry; find this
    /// runtime device's record there (same match as the Computers list).
    private func resolveDesktop() async {
        isResolvingDesktop = true
        defer { isResolvingDesktop = false }
        let computers = (try? await BotModeClient.shared.listComputers()) ?? []
        desktop = computers.first(where: { $0.id == computerId })
            ?? computers.first(where: { $0.name == name || ($0.host != nil && $0.host == device?.hostname) })
    }

    private func openDesktop() async {
        if desktop == nil { await resolveDesktop() }
        if let desktop { path.append(desktop) } else { isScreenPickerOpen = true }
    }
}

/// Opens what a Files shortcut asked for (Open Files on, Scan to): the
/// computer's detail, with the document camera when it was a scan.
struct FilesLaunchHost: ViewModifier {
    @ObservedObject private var files = FilesDrive.shared

    func body(content: Content) -> some View {
        content.fullScreenCover(item: $files.launch) { launch in
            ACIComputerDetailCover(computerId: launch.computerId, name: files.name(of: launch.computerId),
                                   startScan: { if case .scan = launch { return true } else { return false } }())
        }
    }
}

// MARK: - Browser computer

/// The Browser computer's detail: status, a plain terminal, and Reset disk.
/// The VM runs in a web view inside the app (see BrowserComputerWebView.swift),
/// so it needs iOS 27 or later; a capability probe decides before any boot.
/// Terminal: a monospaced text view fed by the pty stream plus an input field
/// (smaller than embedding the box page's own terminal, which the contract
/// does not define).
struct ACIBrowserComputerCover: View {
    @StateObject private var model = BrowserComputerModel()
    @Environment(\.dismiss) private var dismiss
    @State private var input = ""
    @State private var confirmReset = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    content
                }
                .listRowBackground(Color("FillNeutral"))
                if showsTerminal {
                    Section("Terminal") { terminal }
                        .listRowBackground(Color("FillNeutral"))
                }
                if showsReset {
                    Section {
                        Button(role: .destructive) { confirmReset = true } label: {
                            Text(model.isResetting ? "Resetting…" : "Reset disk")
                        }
                        .disabled(model.isResetting)
                    } footer: {
                        Text("Deletes everything on this computer's disk.")
                    }
                    .listRowBackground(Color("FillNeutral"))
                }
            }
            .filesListStyle()
            .navigationTitle("Browser computer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button { model.stop(); dismiss() } label: {
                        HStack(spacing: 4) {
                            Image(systemName: "chevron.left").font(.system(size: 15, weight: .bold))
                            Text("Computers")
                        }
                    }
                    .accessibilityLabel("Back to Computers")
                }
            }
            .background(
                BrowserComputerWebView(model: model)
                    .frame(width: 1, height: 1)
                    .opacity(0.01)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            )
            .confirmationDialog("Reset this computer's disk?", isPresented: $confirmReset, titleVisibility: .visible) {
                Button("Reset disk", role: .destructive) { Task { await model.resetDisk() } }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Everything on its disk is deleted. This can't be undone.")
            }
        }
        .tint(Color("TextPrimary"))
        .task { await model.probe() }
    }

    private var showsTerminal: Bool { model.phase == .running && model.status.state == "ready" }
    private var showsReset: Bool {
        switch model.phase {
        case .idle, .running, .stopped, .failed: return true
        default: return false
        }
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .probing:
            LabeledContent("State", value: "Checking this iPhone…")
        case .unsupported:
            Text(BrowserComputerModel.unsupportedLine)
                .font(.system(size: 15))
                .foregroundColor(Color("TextSecondary"))
        case .idle:
            LabeledContent("State", value: "Stopped")
            startButton("Start")
        case .starting:
            LabeledContent("State", value: "Starting…")
        case .stopped:
            Text(BrowserComputerModel.memoryLine)
                .font(.system(size: 15))
                .foregroundColor(Color("TextPrimary"))
            startButton("Start again")
        case .failed(let message):
            Text(message)
                .font(.system(size: 15))
                .foregroundColor(Theme.statusError)
            startButton("Try again")
        case .running:
            LabeledContent("State", value: Self.stateText(model.status))
            if let used = model.status.diskBytesUsed {
                LabeledContent("Disk used", value: ByteCountFormatter.string(fromByteCount: Int64(used), countStyle: .file))
            }
            if let message = model.status.errorMessage {
                Text(message).font(.system(size: 13)).foregroundColor(Theme.statusError)
            }
            Button("Stop") { model.stop() }
        }
    }

    private func startButton(_ title: String) -> some View {
        Button(title) { Task { await model.start() } }
            .disabled(model.isResetting)
    }

    private var terminal: some View {
        VStack(alignment: .leading, spacing: 8) {
            ScrollViewReader { proxy in
                ScrollView {
                    Text(model.output.isEmpty ? " " : model.output)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                    Color.clear.frame(height: 1).id("end")
                }
                .frame(height: 220)
                .onChange(of: model.output) { _, _ in proxy.scrollTo("end") }
            }
            HStack {
                TextField("Type a command", text: $input)
                    .font(.system(size: 14, design: .monospaced))
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.send)
                    .onSubmit(sendLine)
                Button("Send", action: sendLine)
                    .disabled(input.isEmpty)
            }
        }
    }

    private func sendLine() {
        let line = input
        input = ""
        Task { await model.send(line) }
    }

    private static func stateText(_ s: BrowserComputerModel.Status) -> String {
        switch s.state {
        case "loading":
            if let p = s.progress, p.total > 0 { return "Downloading \(p.loaded * 100 / p.total)%" }
            return "Downloading…"
        case "booting": return "Starting…"
        case "ready": return "Running"
        case "resetting": return "Resetting…"
        case "error": return "Problem"
        default: return "Stopped"
        }
    }
}
