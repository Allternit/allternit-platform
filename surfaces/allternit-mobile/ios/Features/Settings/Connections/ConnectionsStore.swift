import Combine
import Foundation

/// The live connect-kit list for Settings › Connections and first-run setup.
/// Reads cloud-api and the active computer, merges them (contract §3), and
/// layers on what only this iPhone knows: it is signed in here, its voice keys
/// sit in the Keychain, and "Let your bots use this iPhone" is paired or not.
@MainActor
final class ConnectionsStore: ObservableObject {
    static let shared = ConnectionsStore()

    @Published private(set) var rows: [String: ConnectionWire] = [:] {
        didSet { publishPersonModels() }
    }
    @Published private(set) var isLoading = false
    @Published private(set) var loaded = false
    /// Set when cloud-api could not answer; rows fall back to the catalog.
    @Published private(set) var loadError: String?
    /// The computer the runtime rows came from, nil when none answered.
    @Published private(set) var runtimeId: String?
    /// Rows mid-connect on this phone ("Connecting…") until the server says otherwise.
    @Published private var connecting: Set<String> = []

    var computerOnline: Bool { runtimeId != nil }

    private var deviceWatch: AnyCancellable?

    init() {
        // ChatGPT signs in on this iPhone (§9.9): its row and models follow
        // the device store.
        deviceWatch = ChatGPTDeviceStore.shared.objectWillChange.sink { [weak self] _ in
            Task { @MainActor in
                self?.objectWillChange.send()
                self?.publishPersonModels()
            }
        }
    }

    /// The chat picker's plan models (`ModelStore.personModels`).
    private func publishPersonModels() {
        ModelStore.shared.setPersonModels(PersonModels.build(rows: Array(rows.values), chatgpt: ChatGPTDeviceStore.shared.models))
    }

    func load() async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false; loaded = true }
        async let cloudTask: Result<[ConnectionWire], Error> = Self.capture { try await ConnectionsClient.list(.cloud) }
        let runtime = await RuntimeRelayStore.shared.resolveActiveRuntimeId()
        var runtimeRows: [ConnectionWire] = []
        var answered: String?
        if let runtime {
            if let list = try? await ConnectionsClient.list(.runtime(runtime)) {
                runtimeRows = list
                answered = runtime
            }
        }
        let cloud = await cloudTask
        switch cloud {
        case .success(let list):
            loadError = nil
            rows = ConnectionsMerge.merge(cloud: list, runtime: runtimeRows, known: ConnectionsCatalog.knownIds)
        case .failure(let error):
            loadError = ConnectionsClient.describe(error)
            rows = ConnectionsMerge.merge(cloud: [], runtime: runtimeRows, known: ConnectionsCatalog.knownIds)
        }
        runtimeId = answered
    }

    private static func capture<T>(_ work: @escaping @Sendable () async throws -> T) async -> Result<T, Error> {
        do { return .success(try await work()) } catch { return .failure(error) }
    }

    // MARK: - Reading a row

    /// The merged row, or the catalog's fallback when the server sent none.
    func row(_ id: String) -> ConnectionWire {
        var row = rows[id] ?? ConnectionsCatalog.fallbackRow(id, computerOnline: computerOnline)
        applyDeviceFacts(&row)
        if connecting.contains(id), row.state != .on { row.state = .connecting }
        return row
    }

    func state(_ id: String) -> ConnState { row(id).state }

    /// Can this account use it now? A connected row always can.
    func isAvailable(_ id: String) -> Bool {
        let r = row(id)
        return r.available || r.state == .on
    }

    /// "An API key" stands for every key provider: on when any key works,
    /// needs you when any key was rejected.
    private func keysSummary() -> ConnectionWire {
        let keys = ConnectionsCatalog.keyProviders.map { (provider: $0, row: rows[$0.id]) }
        let on = keys.filter { $0.row?.state == .on }.map(\.provider.name)
        if let needing = keys.first(where: { $0.row?.state == .needs }) {
            return ConnectionWire(id: "api-key", state: .needs,
                                  reason: needing.row?.reason ?? ConnectionsCatalog.reasonText(needing.row?.reasonCode, name: needing.provider.name) ?? "\(needing.provider.name) key needs you")
        }
        if !on.isEmpty {
            return ConnectionWire(id: "api-key", state: .on, found: on.count <= 2 ? on.joined(separator: ", ") : "\(on.count) keys")
        }
        return ConnectionWire(id: "api-key")
    }

    private func applyDeviceFacts(_ row: inout ConnectionWire) {
        switch row.id {
        case "api-key":
            let summary = keysSummary()
            row.state = summary.state
            row.found = summary.found
            row.reason = summary.reason
        case "gemini":
            // Alias of google-key (contract §3).
            if let key = rows["google-key"], rows["gemini"] == nil {
                row.state = key.state
                row.found = key.found
                row.reason = key.reason
            }
        case "phone":
            // This app is signed in on this iPhone.
            row.state = .on
            row.found = "This iPhone"
            row.available = true
        case "phone-control":
            if AppPhoneService.shared.credentials != nil {
                row.state = .on
                row.found = row.found ?? "This iPhone"
            }
        case "chatgpt":
            // On iOS ChatGPT signs in on the phone (contract §9.9). A
            // computer's ChatGPT that is on still counts when the phone isn't
            // signed in.
            let device = ChatGPTDeviceStore.shared
            if device.signedIn {
                row = ConnectionWire(id: "chatgpt", state: device.needs == nil ? .on : .needs,
                                     found: device.needs == nil ? (device.plan.map { "ChatGPT \($0.capitalized) · this iPhone" } ?? device.email.map { "\($0) · this iPhone" } ?? "This iPhone") : nil,
                                     reason: device.needs?.reason, reasonCode: device.needs?.code, source: "device")
                row.runsOn = "device"
                row.personOnly = true
                row.models = device.models.map(\.id)
            } else if !(row.state == .on && row.runtimeId != nil) {
                if device.needs == .loginExpired {
                    row = ConnectionWire(id: "chatgpt", state: .needs, reason: device.needs?.reason, reasonCode: device.needs?.code, source: "device")
                } else {
                    row = ConnectionWire(id: "chatgpt", source: "device")
                }
                row.runsOn = "device"
            }
        case "openai-voice", "elevenlabs":
            let vendor: VoiceVendor = row.id == "elevenlabs" ? .elevenlabs : .openai
            if VoiceProviderKeys.value(vendor.keyField) != nil {
                row.state = .on
                row.found = VoiceProviderKeys.label(vendor) ?? "Saved on this iPhone"
            }
        default:
            break
        }
    }

    /// The one status line: the four words and nothing else.
    func statusLine(_ entry: ConnEntry) -> String {
        let r = row(entry.id)
        switch r.state {
        case .off: return entry.blurb.map { "Not connected · \($0)" } ?? "Not connected"
        case .connecting: return "Connecting…"
        case .on: return r.found.map { "Connected · \($0)" } ?? "Connected"
        case .needs:
            let reason = r.reason ?? ConnectionsCatalog.reasonText(r.reasonCode, name: entry.name)
            return reason.map { "Needs you · \($0)" } ?? "Needs you"
        }
    }

    /// The plain reason an unavailable row shows instead of a button.
    func unavailableReason(_ id: String) -> String {
        let r = row(id)
        let name = ConnectionsCatalog.entry(id)?.name ?? id
        return r.unavailableReason ?? ConnectionsCatalog.reasonText(r.reasonCode, name: name) ?? "Not available yet."
    }

    // MARK: - Targets and changes

    /// Rows that live on a computer go to that computer; everything else to cloud-api.
    func target(for id: String) -> ConnTarget {
        if let rid = rows[id]?.runtimeId { return .runtime(rid) }
        if let rid = runtimeId, ConnectionsCatalog.cliLoginIds.contains(id) || id == "local" || (id == "chatgpt" && rows[id]?.source == nil) {
            return .runtime(rid)
        }
        return .cloud
    }

    func setConnecting(_ id: String, _ on: Bool) {
        if on { connecting.insert(id) } else { connecting.remove(id) }
    }

    /// A fresh row from `/submit` or `/login`.
    func apply(_ row: ConnectionWire) {
        guard ConnectionsCatalog.knownIds.contains(row.id) else { return }
        connecting.remove(row.id)
        rows[row.id] = row
    }

    func markOn(_ id: String) {
        connecting.remove(id)
        var r = rows[id] ?? ConnectionWire(id: id)
        r.state = .on
        r.reason = nil
        r.reasonCode = nil
        rows[id] = r
    }

    /// ChatGPT signed in on this phone (not a computer's ChatGPT).
    var chatgptOnDevice: Bool { ChatGPTDeviceStore.shared.signedIn }

    func disconnect(_ id: String) async throws {
        if id == "chatgpt", chatgptOnDevice {
            let confirmed = await ChatGPTDeviceStore.shared.signOut()
            if ModelStore.shared.selectedModelId.map({ $0.hasPrefix("chatgpt/") }) == true { ModelStore.shared.selectedModelId = nil }
            if !confirmed { throw ChatGPTSignOutUnconfirmed() }
            return
        }
        try await ConnectionsClient.disconnect(id, target: target(for: id))
        // That plan's models stop working: don't leave the composer on one.
        if let selected = ModelStore.shared.selectedModelId, rows[id]?.models.contains(selected) == true {
            ModelStore.shared.selectedModelId = nil
        }
        var r = rows[id] ?? ConnectionWire(id: id)
        r.state = .off
        r.found = nil
        rows[id] = r
    }

    #if DEBUG
    /// Tests and previews only.
    func seed(_ rows: [String: ConnectionWire], runtimeId: String?) {
        self.rows = rows
        self.runtimeId = runtimeId
        loaded = true
    }
    #endif
}

/// Signed out on this iPhone, but OpenAI didn't confirm the revoke
/// (Accounts and sessions, "Signing out").
struct ChatGPTSignOutUnconfirmed: Error, LocalizedError {
    var errorDescription: String? {
        "Signed out on this iPhone. OpenAI didn't confirm it, so you can also remove Allternit in ChatGPT's settings."
    }
}
