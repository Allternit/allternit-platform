import SwiftUI
import AllternitFilesKit

/// Computers console › Terminal and Files tabs, the Space view, snippets, and
/// the sheet an alert push opens. No new rail tab: these live in
/// `ACIConsoleDetailView` (ACIComputersPane.swift).

// MARK: - Terminal

/// A shell on this computer through the same terminal service the Code tab
/// uses (`TerminalServiceClient` over the relay placeholder
/// `allternit-relay://<runtimeId ?? row id>`), drawn by the existing SwiftTerm view.
/// Foreground only: leaving the tab or backgrounding the app detaches; the
/// shell keeps running on the computer and is picked up again from Open.
struct ACIConsoleTerminalTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    @Environment(\.scenePhase) private var scenePhase
    @State private var session: PtySession?
    @State private var snippets: ACIConsoleLoad<[ACIComputersClient.Snippet]> = .loading
    @State private var runSnippet: ACIComputersClient.Snippet?

    private var canOpen: Bool { computer.isOnline && computer.status.lowercased() != "starting" }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                if let session {
                    VStack(spacing: 8) {
                        TerminalSessionView(session: session)
                            .frame(height: 380)
                            .clipShape(RoundedRectangle(cornerRadius: 16))
                        Button("Close terminal") { self.session = nil }
                            .font(.system(size: 14)).foregroundColor(Color("TextSecondary")).frame(minHeight: 44)
                    }
                } else {
                    consoleCard {
                        Text("Terminal").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        if canOpen {
                            Text("A shell on \(computer.name). It closes when you leave this screen or the app goes to the background. The shell keeps running on the computer.")
                                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                            ACIPillButton(title: "Open terminal") { open() }
                        } else {
                            Text(computer.isAsleep
                                 ? "\(computer.name) is asleep. Opening a terminal would wake it, so it stays closed. Wake it from the Overview first."
                                 : "\(computer.name) isn't reachable right now.")
                                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        }
                    }
                }
                snippetsSection
            }
            .padding(16)
        }
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { session = nil }
        }
        .task { snippets = await .run { try await ACIComputersClient.shared.snippets() } }
        .sheet(item: $runSnippet) { snippet in
            ACIConsoleFixSheet(
                computer: computer,
                finding: .init(id: "snippet-\(snippet.id)", severity: "info", category: "config",
                               title: snippet.name, detail: snippet.description ?? snippet.command,
                               suggestedAction: .init(type: "snippet.run", target: snippet.id, label: snippet.command)),
                onChanged: {},
                heading: "Snippet",
                navTitle: "Run a snippet")
        }
    }

    private func open() {
        session = PtySession(
            service: TerminalServiceClient(baseURL: RuntimeRelayClient.relayBaseURL(runtimeId: computer.relayId)),
            attachedInstanceName: computer.name)
    }

    @ViewBuilder private var snippetsSection: some View {
        Text("Snippets").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary")).padding(.top, 4)
        switch snippets {
        case .loading: ProgressView().frame(maxWidth: .infinity).padding(.vertical, 16)
        case .unavailable: consoleNote("Snippets aren't available on your account yet.")
        case .asleep: consoleNote("Snippets can't load right now.")
        case .failed(let message): consoleNote(message)
        case .ready(let list):
            if list.isEmpty {
                consoleNote("No saved snippets. Add one from the web console or `gizzi computers snippets`.")
            } else {
                ForEach(list) { snippet in
                    consoleCard {
                        Text(snippet.name).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        Text(snippet.command)
                            .font(.system(size: 13, design: .monospaced)).foregroundColor(Color("TextPrimary"))
                            .lineLimit(3)
                            .padding(.horizontal, 12).padding(.vertical, 8)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Color("TextPrimary").opacity(0.08))
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                        if let d = snippet.description, !d.isEmpty {
                            Text(d).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        }
                        ACIPillButton(title: "Run on \(computer.name)") { runSnippet = snippet }
                            .disabled(!computer.isOnline)
                    }
                }
            }
        }
    }
}

// MARK: - Files

/// This computer's Files drive roots. Two panes where width allows (iPad,
/// landscape), one on iPhone; Space is the disk breakdown with approved
/// clean-ups. The Files drive has no Trash yet, so there is no Trash view.
struct ACIConsoleFilesTab: View {
    let computer: ACIComputersClient.ConsoleComputer
    /// Switches the detail to its Settings tab (from a 403 `whole_disk_off`).
    var openSettings: (() -> Void)? = nil
    @ObservedObject private var drive = FilesDrive.shared
    @State private var view: Mode = .browse

    enum Mode: String, CaseIterable, Identifiable {
        case browse = "Browse", space = "Space"
        var id: String { rawValue }
    }

    var body: some View {
        VStack(spacing: 0) {
            Picker("View", selection: $view) {
                ForEach(Mode.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .padding(.horizontal, 16).padding(.bottom, 8)
            switch view {
            case .browse:
                if drive.hasFiles(computer.relayId) {
                    ACIConsoleFilesPanes(computerId: computer.relayId, name: computer.name,
                                         diskRoots: computer.wholeDiskState.showsRootsSwitcher ? computer.wholeDiskState.roots : [],
                                         openSettings: openSettings)
                } else {
                    ScrollView {
                        consoleNote("The Files drive isn't on for \(computer.name) yet. Turn it on in Settings, Files, and this computer's folders show here.")
                            .padding(16)
                    }
                }
            case .space:
                ScrollView { ACIConsoleSpaceView(computer: computer).padding(16) }
            }
        }
    }
}

private struct ACIConsoleFilesPanes: View {
    let computerId: String
    let name: String
    let diskRoots: [ACIComputersClient.WholeDiskRoot]
    let openSettings: (() -> Void)?
    @StateObject private var model: FilesComputerModel
    @Environment(\.horizontalSizeClass) private var sizeClass

    init(computerId: String, name: String, diskRoots: [ACIComputersClient.WholeDiskRoot], openSettings: (() -> Void)?) {
        self.computerId = computerId
        self.name = name
        self.diskRoots = diskRoots
        self.openSettings = openSettings
        _model = StateObject(wrappedValue: FilesComputerModel(computerId: computerId, name: name))
    }

    var body: some View {
        GeometryReader { geo in
            let wide = sizeClass == .regular || geo.size.width >= 700
            HStack(spacing: 0) {
                pane
                if wide {
                    Rectangle().fill(Color("BorderSubtle")).frame(width: 1)
                    pane
                }
            }
        }
        .task {
            model.diskRoots = diskRoots
            model.openSettings = openSettings
            await model.load()
        }
        .onChange(of: diskRoots) { _, new in model.diskRoots = new }
    }

    private var pane: some View {
        FilesPane(model: model)
    }
}

/// One browsing pane: its own navigation path over the shared computer model.
private struct FilesPane: View {
    @ObservedObject var model: FilesComputerModel
    @State private var path = NavigationPath()

    var body: some View {
        NavigationStack(path: $path) {
            FilesRootsScreen(model: model)
                .navigationBarTitleDisplayMode(.inline)
                .navigationDestination(for: FilesRoute.self) { route in
                    switch route {
                    case .roots: FilesRootsScreen(model: model)
                    case .folder(let rootId, let parentId, let title):
                        FilesFolderScreen(model: model, rootId: rootId, parentId: parentId, title: title)
                    case .requests: FilesInboxRequestsView(model: model)
                    }
                }
        }
        .tint(Color("TextPrimary"))
    }
}

// MARK: - Space

/// Disk use from the last host sample (neutral greys, never tan) and the
/// clean-up items the host's findings offer. Each clean-up goes through the
/// approval sheet (`space.clean`, ids: apt-cache, dnf-cache, journal-vacuum,
/// docker-prune, brew-cleanup).
struct ACIConsoleSpaceView: View {
    let computer: ACIComputersClient.ConsoleComputer
    @State private var snapshot: ACIConsoleLoad<ACIComputersClient.HostSnapshot> = .loading
    @State private var findings: ACIConsoleLoad<[ACIComputersClient.HostFinding]> = .loading
    @State private var cleaning: ACIComputersClient.HostFinding?
    @State private var attempt = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            disks
            cleanups
        }
        .task(id: attempt) {
            snapshot = await .run { try await ACIComputersClient.shared.hostSnapshot(id: computer.id) }
            findings = await .run { try await ACIComputersClient.shared.hostFindings(id: computer.id) }
        }
        .sheet(item: $cleaning) { finding in
            ACIConsoleFixSheet(computer: computer, finding: finding, onChanged: { attempt += 1 },
                               heading: "What this frees", navTitle: "Approve a clean-up")
        }
    }

    @ViewBuilder private var disks: some View {
        switch snapshot {
        case .loading: ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24)
        case .unavailable: consoleNote("Disk use isn't available on your account yet.")
        case .failed(let m): consoleNote(m)
        case .asleep(let last):
            if let last { diskCard(last.disk.map { ($0.mount, $0.usedBytes, $0.totalBytes) }, note: "Asleep, not woken. Last sample.") }
            else { consoleNote("Asleep, not woken. No disk sample yet.") }
        case .ready(let s):
            diskCard(s.disk.map { ($0.mount, $0.usedBytes, $0.totalBytes) }, note: nil)
        }
    }

    private func diskCard(_ disks: [(String, Double, Double)], note: String?) -> some View {
        consoleCard {
            Text("Disk use").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            if disks.isEmpty { Text("No disks reported.").font(.system(size: 13)).foregroundColor(Color("TextSecondary")) }
            ForEach(Array(disks.enumerated()), id: \.offset) { _, d in
                let frac = d.2 > 0 ? min(max(d.1 / d.2, 0), 1) : 0
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(d.0).font(.system(size: 13, design: .monospaced)).foregroundColor(Color("TextPrimary"))
                        Spacer()
                        Text("\(consoleBytes(d.1)) of \(consoleBytes(d.2))").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    }
                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            Capsule().fill(Color("TextPrimary").opacity(0.08))
                            Capsule().fill(Color("TextPrimary").opacity(frac >= 0.9 ? 0.75 : 0.45))
                                .frame(width: geo.size.width * frac)
                        }
                    }
                    .frame(height: 10)
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(d.0), \(Int(frac * 100)) percent used, \(consoleBytes(d.1)) of \(consoleBytes(d.2))")
            }
            if let note { Text(note).font(.system(size: 12)).foregroundColor(Color("TextTertiary")) }
        }
    }

    @ViewBuilder private var cleanups: some View {
        Text("Clean up").font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary")).padding(.top, 4)
        switch findings {
        case .loading: EmptyView()
        case .unavailable: consoleNote("Clean-ups aren't available on your account yet.")
        case .asleep: consoleNote("Asleep, not woken. Clean-ups wait until it is awake.")
        case .failed(let m): consoleNote(m)
        case .ready(let list):
            let items = list.filter { $0.suggestedAction?.type == "space.clean" }
            if items.isEmpty {
                consoleNote("Nothing to clean right now.")
            } else {
                ForEach(items) { item in
                    consoleCard {
                        Text(item.title).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                        Text(item.detail).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
                        ACIPillButton(title: "Clean") { cleaning = item }
                    }
                }
            }
        }
    }
}

// MARK: - Alert push

/// What a `computer_alert` push (or `/computers/<id>`) opens: that computer's
/// console detail, looked up from the console list.
struct ConsoleComputerLink: Identifiable, Equatable { let id: String }

struct ACIConsoleDeepLinkSheet: View {
    let computerId: String
    @State private var state: ACIConsoleLoad<ACIComputersClient.ConsoleComputer> = .loading
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        switch state {
        case .ready(let computer):
            ACIConsoleDetailView(computer: computer)
        default:
            NavigationStack {
                Group {
                    switch state {
                    case .loading: ProgressView()
                    case .failed(let m): consoleNote(m)
                    default: consoleNote("This computer isn't in your console. It may have been removed.")
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color("BgPrimary"))
                .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            }
            .task {
                state = await .run {
                    let all = try await ACIComputersClient.shared.listConsole()
                    guard let found = all.first(where: { $0.matches(computerId) }) else {
                        throw ACIComputersClient.Failure.notAvailable
                    }
                    return found
                }
            }
        }
    }
}
