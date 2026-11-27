import SwiftUI
import UIKit

// Start anywhere (plan D1): ONE device list for the + sheet's Computer page,
// the chip picker's Computer row and Code's computer list. Same rows and
// words as the web's useDeviceList / DeviceList.

enum StartDeviceState: Int, Sendable {
    case online = 0, cloud = 1, asleep = 2
}

/// One computer row: "Online now" / "Asleep · last seen …" / "Wakes when you send".
struct StartDeviceRow: Identifiable, Equatable, Sendable {
    let id: String
    let name: String
    let state: StartDeviceState
    let sub: String

    var icon: String {
        switch state {
        case .online: return "desktopcomputer"
        case .asleep: return "moon"
        case .cloud: return "cloud"
        }
    }
}

/// What to offer when an asleep computer is picked (D7).
enum StartAsleepOffer: Equatable, Sendable {
    /// Run the same git folder on this cloud computer.
    case cloud(cloudRuntimeId: String, folderName: String, git: StartFolderGit)
    /// Not movable; the plain reason why.
    case none(folderName: String, reason: String)
}

enum StartDeviceRows {
    static let noComputerTitle = "No computer"
    static let noComputerSub = "Plan chats work without one"

    /// Rows from the relay's runtime list plus presence (`kind`, `lastSeen`).
    static func build(_ devices: [RelayRuntimeDevice], presence: [String: StartPresence] = [:], now: Date = Date()) -> [StartDeviceRow] {
        let rows = devices.map { row(for: $0, presence: presence[$0.id], now: now) }
        return rows.enumerated()
            .sorted { ($0.element.state.rawValue, $0.offset) < ($1.element.state.rawValue, $1.offset) }
            .map(\.element)
    }

    static func row(for device: RelayRuntimeDevice, presence: StartPresence?, now: Date = Date()) -> StartDeviceRow {
        let cloudTypes: Set<String> = ["hosted", "provisioned", "vps", "cloud"]
        let isCloud = presence?.kind.map { $0 == "cloud" } ?? (device.isWakeable || cloudTypes.contains(device.runtimeType))
        if isCloud {
            return StartDeviceRow(id: device.id, name: device.displayName, state: .cloud, sub: "Wakes when you send")
        }
        if presence?.online ?? device.isOnline {
            return StartDeviceRow(id: device.id, name: device.displayName, state: .online, sub: "Online now")
        }
        let seen = lastSeen(presence?.lastSeen ?? device.lastSeenAt, now: now)
        return StartDeviceRow(id: device.id, name: device.displayName, state: .asleep,
                              sub: seen.map { "Asleep · last seen \($0)" } ?? "Asleep")
    }

    /// "9 hours ago" from an ISO date, nil when missing or unreadable.
    static func lastSeen(_ iso: String?, now: Date = Date()) -> String? {
        guard let iso, !iso.isEmpty else { return nil }
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = withFraction.date(from: iso) ?? ISO8601DateFormatter().date(from: iso) else { return nil }
        if now.timeIntervalSince(date) < 60 { return "just now" }
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .full
        return formatter.localizedString(for: date, relativeTo: now)
    }

    /// The asleep offer: the same git folder on a cloud computer, or why not.
    /// `folderPath` is a path on the asleep computer; default its most recent folder.
    static func asleepOffer(for row: StartDeviceRow, snapshot: StartSnapshotDevice?, folderPath: String?, cloudRuntimeId: String?) -> StartAsleepOffer {
        let folders = snapshot?.folders ?? []
        let found = folderPath.map { p in folders.first { $0.path == p } } ?? folders.first
        let folderName = found?.name
            ?? folderPath.flatMap { $0.split(separator: "/").last.map(String.init) }
            ?? "this folder"
        guard let found else {
            return .none(folderName: folderName, reason: "\(row.name) has not shared its folders, so a cloud computer cannot open \(folderName).")
        }
        guard let git = found.git, !git.remote.isEmpty else {
            return .none(folderName: folderName, reason: "\(folderName) has no git remote, so a cloud computer cannot open it.")
        }
        guard let cloudRuntimeId else {
            return .none(folderName: folderName, reason: "There is no cloud computer on this account to open \(folderName).")
        }
        return .cloud(cloudRuntimeId: cloudRuntimeId, folderName: folderName, git: git)
    }
}

/// The computer + folder a new chat runs on, the per-device options and
/// snapshots, and each thread's binding. The first send reads it
/// (`ChatViewModel.startOnPickedComputer`).
@MainActor
final class SessionStartStore: ObservableObject {
    static let shared = SessionStartStore()

    /// The computer picked for the next new chat (nil = no computer: today's chat).
    @Published private(set) var pickedRuntimeId: String?
    /// The folder picked for it (nil = its most recent folder).
    @Published private(set) var pickedFolder: StartFolderPick?
    @Published private(set) var options: [String: StartOptions] = [:]
    @Published private(set) var optionsError: [String: String] = [:]
    @Published private(set) var loadingOptions: Set<String> = []
    @Published private(set) var snapshots: [String: StartSnapshotDevice] = [:]
    @Published private(set) var presence: [String: StartPresence] = [:]
    /// The binding of the chat on screen (the chip's computer segment).
    @Published var currentBinding: StartBinding?
    @Published private(set) var bindings: [String: StartBinding]

    /// Reused on a retry of the same first send, so the device starts once.
    private(set) var idempotencyKey: String?
    private var optionsLoadedAt: [String: Date] = [:]
    private let defaults = UserDefaults.standard
    private static let bindingsKey = "allternit-start-bindings"

    private init() {
        if let data = defaults.data(forKey: Self.bindingsKey),
           let saved = try? JSONDecoder().decode([String: StartBinding].self, from: data) {
            bindings = saved
        } else {
            bindings = [:]
        }
    }

    // MARK: Picks

    /// A pick in the shared list. The relay follows it, so the chat engine
    /// and the terminal reach that computer.
    func pick(_ runtimeId: String?) {
        pickedRuntimeId = runtimeId
        pickedFolder = nil
        idempotencyKey = nil
        RuntimeRelayStore.shared.select(runtimeId)
        if let runtimeId { Task { await loadOptions(runtimeId) } }
    }

    /// "Use a cloud computer": the asleep Mac's git folder on a cloud computer.
    func useCloud(runtimeId: String, git: StartFolderGit) {
        pick(runtimeId)
        pickedFolder = .git(remote: git.remote, branch: git.branch, head: git.head)
    }

    func pickFolder(_ folder: StartFolderPick) {
        pickedFolder = folder
        idempotencyKey = nil
    }

    /// The folder the next start uses: the pick, else the computer's most recent.
    var effectiveFolder: StartFolderPick? {
        if let pickedFolder { return pickedFolder }
        guard let id = pickedRuntimeId else { return nil }
        if let first = options[id]?.folders.first ?? snapshots[id]?.folders.first { return .path(first.path) }
        return nil
    }

    /// The picked folder's path on the computer (nil for a git folder).
    var effectiveFolderPath: String? {
        if case .path(let path) = effectiveFolder { return path }
        return nil
    }

    /// Recent folders on the picked computer (live options, else the snapshot).
    var recentFolders: [StartFolder] {
        guard let id = pickedRuntimeId else { return [] }
        return options[id]?.folders ?? snapshots[id]?.folders ?? []
    }

    /// The picked computer's own permission default for `chipRuntimeId`.
    func computerDefaultMode(chipRuntimeId: String?) -> ChatPermissionMode? {
        guard let id = pickedRuntimeId else { return nil }
        let runtime = chipRuntimeId.map(SessionStartClient.contractRuntime)
        var raw = options[id]?.defaultMode(for: runtime)
        if raw == nil, let snap = snapshots[id], let modes = snap.modes {
            raw = (runtime.flatMap { modes[$0] } ?? snap.defaultRuntime.flatMap { modes[$0] })?.default
        }
        return raw.flatMap(ChatPermissionMode.init(rawValue:))
    }

    /// The picked computer's name for the chip and the + sheet.
    func name(of runtimeId: String?) -> String? {
        guard let runtimeId else { return nil }
        if let device = RuntimeRelayStore.shared.devices.first(where: { $0.id == runtimeId }) { return device.displayName }
        return options[runtimeId]?.device.name ?? snapshots[runtimeId]?.name
    }

    func row(for runtimeId: String?) -> StartDeviceRow? {
        guard let runtimeId, let device = RuntimeRelayStore.shared.devices.first(where: { $0.id == runtimeId }) else { return nil }
        return StartDeviceRows.row(for: device, presence: presence[runtimeId])
    }

    func rows(_ devices: [RelayRuntimeDevice]) -> [StartDeviceRow] {
        StartDeviceRows.build(devices, presence: presence)
    }

    // MARK: Loading

    /// Presence + snapshots + the relay's runtime list. Each part is optional:
    /// without presence the relay status decides, without snapshots the
    /// asleep offer gives a plain reason.
    func refresh() async {
        await RuntimeRelayStore.shared.refresh()
        if let rows = try? await SessionStartClient.presence() {
            presence = Dictionary(rows.map { ($0.runtimeId, $0) }, uniquingKeysWith: { a, _ in a })
        }
        if let snap = try? await SessionStartClient.snapshots() {
            snapshots = Dictionary(snap.devices.map { ($0.runtimeId, $0) }, uniquingKeysWith: { a, _ in a })
        }
    }

    /// The device's folders and modes (30 s cache, like the web hook).
    func loadOptions(_ runtimeId: String, force: Bool = false) async {
        if !force, let at = optionsLoadedAt[runtimeId], Date().timeIntervalSince(at) < 30, options[runtimeId] != nil { return }
        guard !loadingOptions.contains(runtimeId) else { return }
        loadingOptions.insert(runtimeId)
        defer { loadingOptions.remove(runtimeId) }
        do {
            let loaded = try await SessionStartClient(runtimeId: runtimeId).options()
            options[runtimeId] = loaded
            optionsError[runtimeId] = nil
            optionsLoadedAt[runtimeId] = Date()
        } catch let error as StartSessionError {
            optionsError[runtimeId] = error.inline
        } catch {
            optionsError[runtimeId] = plainErrorText(error)
        }
    }

    // MARK: Start + bindings

    /// The first send on a picked computer: starts the session there and
    /// returns its binding. The pick is kept on failure so a retry resends
    /// with the same idempotency key.
    func startPicked(chipRuntimeId: String, mode: ChatPermissionMode, prompt: String?) async throws -> StartBinding {
        guard let runtimeId = pickedRuntimeId else { throw StartSessionError(status: 400, code: "no_computer", message: "Pick a computer first.", action: nil) }
        if options[runtimeId] == nil && snapshots[runtimeId] == nil { await loadOptions(runtimeId) }
        guard let folder = effectiveFolder else {
            throw StartSessionError(status: 404, code: "folder_missing",
                                    message: optionsError[runtimeId] ?? "No folder is picked on this computer.",
                                    action: "Pick a folder in + › Folder.")
        }
        let key = idempotencyKey ?? UUID().uuidString
        idempotencyKey = key
        let runtime = SessionStartClient.contractRuntime(chipRuntimeId)
        let origin = StartOrigin(surface: "ios", deviceName: UIDevice.current.name, depth: 0)
        let request = StartRequest(folder: folder, runtime: runtime, mode: mode.rawValue, prompt: prompt, origin: origin)
        let result = try await SessionStartClient(runtimeId: runtimeId).start(request, idempotencyKey: key)
        let deviceName = name(of: runtimeId) ?? "Computer"
        switch result {
        case .engine(let sessionId, let directory):
            return StartBinding(runtimeId: runtimeId, deviceName: deviceName, folder: directory.isEmpty ? folder.bindingText : directory,
                                runtime: runtime, mode: mode.rawValue, kind: .engine, sessionId: sessionId, origin: origin)
        case .terminal(let terminalId, let cwd, let agent):
            return StartBinding(runtimeId: runtimeId, deviceName: deviceName, folder: cwd.isEmpty ? folder.bindingText : cwd,
                                runtime: agent.isEmpty ? runtime : agent, mode: mode.rawValue, kind: .terminal, terminalId: terminalId, origin: origin)
        }
    }

    /// Saves a thread's binding (local copy; the session metadata holds the truth)
    /// and clears the pick, so the next new chat starts fresh.
    func bind(_ binding: StartBinding, chatSessionId: String) {
        bindings[chatSessionId] = binding
        currentBinding = binding
        if let data = try? JSONEncoder().encode(bindings) { defaults.set(data, forKey: Self.bindingsKey) }
        pickedRuntimeId = nil
        pickedFolder = nil
        idempotencyKey = nil
    }

    func binding(for chatSessionId: String?) -> StartBinding? {
        chatSessionId.flatMap { bindings[$0] }
    }
}

/// The shared list: computers by state, then "No computer", and the asleep
/// offer under them when an asleep computer is tapped.
struct StartDeviceList: View {
    let selected: String?
    /// The folder on the computer, named in the asleep offer.
    var folderPath: String? = nil
    var showsNoComputer = true
    let onSelect: (String?) -> Void
    var onUseCloud: ((String, StartFolderGit) -> Void)? = nil

    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var store = SessionStartStore.shared
    @State private var asleepId: String? = nil

    var body: some View {
        let rows = store.rows(relay.devices)
        VStack(alignment: .leading, spacing: 10) {
            PlusGroup {
                if rows.isEmpty && relay.isLoading {
                    ProgressView().frame(maxWidth: .infinity, minHeight: 52)
                }
                ForEach(Array(rows.enumerated()), id: \.element.id) { index, row in
                    if index > 0 { PlusDivider() }
                    PlusPickRow(icon: row.icon, title: row.name, subtitle: row.sub, isSelected: selected == row.id) {
                        Haptics.selection()
                        if row.state == .asleep && selected != row.id {
                            asleepId = row.id
                        } else {
                            asleepId = nil
                            onSelect(row.id)
                        }
                    }
                    .accessibilityIdentifier("startDevice-\(row.id)")
                }
                if showsNoComputer {
                    if !rows.isEmpty { PlusDivider() }
                    PlusPickRow(icon: "desktopcomputer", title: StartDeviceRows.noComputerTitle,
                                subtitle: StartDeviceRows.noComputerSub, isSelected: selected == nil) {
                        Haptics.selection()
                        asleepId = nil
                        onSelect(nil)
                    }
                    .accessibilityIdentifier("startDevice-none")
                }
            }

            if let row = rows.first(where: { $0.id == asleepId && $0.state == .asleep }) {
                asleepOffer(row, cloudId: rows.first(where: { $0.state == .cloud })?.id)
            } else {
                Text("Your next message runs on the computer you pick: a real session with your files and tools.")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 6)
            }
        }
        .task { await store.refresh() }
    }

    @ViewBuilder
    private func asleepOffer(_ row: StartDeviceRow, cloudId: String?) -> some View {
        let offer = StartDeviceRows.asleepOffer(for: row, snapshot: store.snapshots[row.id], folderPath: folderPath, cloudRuntimeId: cloudId)
        PlusGroup {
            Group {
                switch offer {
                case .cloud(_, let folderName, _):
                    Text("\(row.name) is asleep. Run in **\(folderName)** on a cloud computer instead?")
                case .none(_, let reason):
                    Text("\(row.name) is asleep. \(reason)")
                }
            }
            .font(.system(size: 14))
            .foregroundColor(Color("TextPrimary"))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .accessibilityIdentifier("startDevice-asleepOffer")
            if case .cloud(let cloudRuntimeId, _, let git) = offer, let onUseCloud {
                PlusDivider()
                PlusRow(icon: "cloud", title: "Use a cloud computer", chevron: true) {
                    asleepId = nil
                    onUseCloud(cloudRuntimeId, git)
                }
            }
            PlusDivider()
            PlusPickRow(icon: "moon", title: "Pick \(row.name) anyway", subtitle: "Runs when it wakes", isSelected: false) {
                Haptics.selection()
                asleepId = nil
                onSelect(row.id)
            }
        }
    }
}

/// The Computer page pushed from the chip picker (its "Computer" row).
struct StartComputerPage: View {
    @ObservedObject private var store = SessionStartStore.shared
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        ScrollView {
            StartDeviceList(
                selected: store.pickedRuntimeId,
                folderPath: store.effectiveFolderPath,
                onSelect: { id in store.pick(id); dismiss() },
                onUseCloud: { id, git in store.useCloud(runtimeId: id, git: git); dismiss() }
            )
            .padding(16)
        }
        .background(Color("BgPanel"))
        .navigationTitle("Computer")
        .navigationBarTitleDisplayMode(.inline)
    }
}
