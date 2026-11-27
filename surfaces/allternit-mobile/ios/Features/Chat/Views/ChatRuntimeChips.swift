import SwiftUI
import UIKit

// Harness & Bots 2.0 in every chat (not only bot chats): the composer shows
// one Runtime·Brain chip in place of the model pill, and it opens the layered
// picker ("Runtime and brain", `BotBrainPickerSheet` chat target). Allternit
// is the default runtime. Picking another runtime starts a chat on it when a
// paired computer that has the runtime is online; otherwise the choice stays
// visible with what's missing ("runs on a computer" / "isn't installed on …").
//
// The new chat is created by ChatViewModel.ensureSessionId, which stamps the
// session with `metadata.runtime` + the legacy `brainMode`/`harness` (the
// same `native_harness` shape a bot's `botBrain` resolves to), so the
// agent-chat bridge runs the turn through that runtime on the computer.

extension Notification.Name {
    /// Settings › Connections › Runtimes "New chat on <Runtime>": the shell
    /// lands on Chats with a fresh chat (the runtime is already picked).
    static let shellNewChatOnRuntime = Notification.Name("com.allternit.shellNewChatOnRuntime")
}

/// Where a runtime can run right now, for this account.
enum ChatRuntimeAvailability: Equatable {
    case ready
    case checking
    /// No paired computer is online.
    case needsComputer
    /// The online computer doesn't have the runtime's CLI.
    case notInstalled(computer: String)

    func message(_ runtimeName: String) -> String? {
        switch self {
        case .ready, .checking: return nil
        case .needsComputer: return "\(runtimeName) runs on a computer. Pair a computer to use it."
        case .notInstalled(let computer): return "\(runtimeName) isn't installed on \(computer)."
        }
    }
}

/// The runtime the next plain chat starts on, plus what each runtime needs.
@MainActor
final class ChatRuntimeStore: ObservableObject {
    static let shared = ChatRuntimeStore()

    /// The picked runtime for new chats. "allternit" is the default.
    @Published var runtimeId = "allternit"
    /// Paired computers (nil until the first check; empty when signed out).
    @Published private(set) var computers: [RuntimeDevice]?
    /// The default computer's CLI list (nil when it couldn't be read).
    @Published private(set) var harnesses: [NativeHarnessInfo]?
    @Published private(set) var isChecking = false

    private var lastCheck: Date?

    var onlineComputer: RuntimeDevice? { computers?.first { $0.isOnline } }

    /// The harness id a runtime's CLI reports as (legacy harness, tool, id).
    static func harnessId(_ r: RuntimeEntry) -> String { r.legacy?.harness ?? r.tool ?? r.id }

    func refreshIfNeeded(force: Bool = false) {
        if !force, let lastCheck, Date().timeIntervalSince(lastCheck) < 30 { return }
        Task { await refresh() }
    }

    func refresh() async {
        guard !isChecking else { return }
        isChecking = true
        defer { isChecking = false; lastCheck = Date() }
        let devices = (try? await RuntimeDevicesClient().fetchRuntimes()) ?? []
        computers = devices
        if devices.contains(where: { $0.isOnline }) {
            harnesses = try? await NativeSessionsClient.harnesses()
        } else {
            harnesses = nil
        }
    }

    func availability(_ r: RuntimeEntry, installed: [String: RuntimeInstallState]?) -> ChatRuntimeAvailability {
        if r.id == "allternit" { return .ready }
        if computers == nil { return isChecking ? .checking : .needsComputer }
        guard let computer = onlineComputer else { return .needsComputer }
        let wanted = [Self.harnessId(r), r.id] + (r.aliases ?? [])
        if let h = harnesses?.first(where: { wanted.contains($0.id) }) {
            return h.present ? .ready : .notInstalled(computer: computer.name)
        }
        // Not in the computer's list (or no list): the registry's install
        // state when it has one, else let the computer decide on send.
        if let state = installed?[r.id], !state.installed { return .notInstalled(computer: computer.name) }
        return .ready
    }

    /// The session metadata a new plain chat carries for the picked runtime.
    func sessionMetadata(in runtimes: [RuntimeEntry]) -> [String: String] {
        guard runtimeId != "allternit", let r = RuntimeRegistry.get(runtimeId, in: runtimes) else { return [:] }
        var brain: [String: JSONValue] = ["runtime": .string(r.id)]
        brain = RuntimeRegistry.resolveRuntime(brain, in: runtimes)
        var out: [String: String] = ["runtime": r.id]
        if let mode = brain["mode"]?.jString { out["brainMode"] = mode }
        if let harness = brain["harness"]?.jString { out["harness"] = harness }
        if let uhp = brain["uhpHarnessId"]?.jString { out["uhpHarnessId"] = uhp }
        if let computer = onlineComputer { out["runtimeComputerId"] = computer.id }
        return out
    }

    /// Picks a runtime for new chats. A chat already under way is left as it
    /// is and a fresh chat opens on the new runtime.
    func start(_ r: RuntimeEntry, chatHasSession: Bool) {
        runtimeId = r.id
        if chatHasSession {
            NotificationCenter.default.post(name: .shellNewChat, object: nil)
        }
    }
}

/// The plain chat's one Runtime·Brain chip (a bot chat has
/// `ComposerAgentChips`). It opens the layered picker on its chat target
/// ("Runtime and brain": Runtime and Brain, no Bot layer).
struct ComposerChatChips: View {
    /// Rest state: the brain name is shortened. The chip stays one chip.
    var iconOnly = false
    /// The chat already has a session (a runtime pick then opens a fresh chat).
    let hasActiveSession: Bool
    /// Kept for the composer hosts' signature. The model sheet now opens from
    /// the picker's "Model options, effort and limits" row.
    let onBrain: () -> Void

    @ObservedObject private var store = ChatRuntimeStore.shared
    @ObservedObject private var registry = RuntimeRegistryStore.shared
    @ObservedObject private var modelStore = ModelStore.shared
    @ObservedObject private var startStore = SessionStartStore.shared
    @State private var isPickerPresented = false

    /// Start anywhere: the computer this chat runs on (the pick for a new
    /// chat, the binding for one under way); nil = no segment.
    private var computer: (name: String, icon: String)? {
        if hasActiveSession {
            guard let binding = startStore.currentBinding else { return nil }
            return (binding.deviceName, startStore.row(for: binding.runtimeId)?.icon ?? "desktopcomputer")
        }
        guard let id = startStore.pickedRuntimeId, let name = startStore.name(of: id) else { return nil }
        return (name, startStore.row(for: id)?.icon ?? "desktopcomputer")
    }

    private var runtime: RuntimeEntry? { registry.runtime(store.runtimeId) }
    private var runtimeName: String { runtime?.name ?? "Allternit" }
    private var providerId: String? {
        guard let id = modelStore.selectedModelId, let slash = id.firstIndex(of: "/") else { return nil }
        return String(id[..<slash])
    }
    private var brainName: String {
        if modelStore.isLoading && modelStore.models.isEmpty { return "Loading…" }
        let name = ComposerModelLabel.label(modelStore)
        return iconOnly ? ComposerAgentChips.short(name, 16) : name
    }
    /// The picked brain can't run in the picked runtime.
    private var misfit: String? {
        guard let runtime, let providerId, !RuntimeRegistry.fits(runtime, providerId: providerId) else { return nil }
        return "brain can't run in \(runtimeName)"
    }
    /// Green when the runtime can run now; amber when it needs a step
    /// (pair a computer, not installed).
    private var state: RuntimeBrainChipState {
        guard let runtime else { return .ready }
        switch store.availability(runtime, installed: registry.installed) {
        case .ready, .checking: return .ready
        case .needsComputer, .notInstalled: return .needsStep
        }
    }

    var body: some View {
        RuntimeBrainChip(runtimeId: store.runtimeId, runtimeName: runtimeName, brainName: brainName,
                         state: state, warning: misfit,
                         computerName: computer?.name, computerIcon: computer?.icon ?? "desktopcomputer") {
            Haptics.selection()
            isPickerPresented = true
        }
        // With a computer segment the chip takes the line's width, so it can
        // drop the brain text instead of overflowing.
        .fixedSize(horizontal: computer == nil, vertical: true)
        .sheet(isPresented: $isPickerPresented) {
            BotBrainPickerSheet(chat: hasActiveSession)
        }
        .task {
            registry.refreshIfNeeded()
            modelStore.fetchModelsIfNeeded()
        }
        .task(id: store.runtimeId) {
            // Only a computer runtime needs the paired-computer check.
            if store.runtimeId != "allternit" { store.refreshIfNeeded() }
        }
    }
}
