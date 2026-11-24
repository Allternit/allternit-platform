import SwiftUI
import UIKit

// Harness & Bots 2.0 in every chat (not only bot chats): the composer shows a
// Runtime chip and a Brain chip in place of the model pill. Allternit is the
// default runtime. Picking another runtime starts a chat on it when a paired
// computer that has the runtime is online; otherwise the choice stays visible
// with what's missing ("runs on a computer" / "isn't installed on …").
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

/// Runtime + Brain chips for a chat without a bot (the bot chat keeps
/// `ComposerAgentChips`, which adds the Bot chip).
struct ComposerChatChips: View {
    /// The chat already has a session (a pick then opens a fresh chat).
    let hasActiveSession: Bool
    /// Opens the brain (model) picker.
    let onBrain: () -> Void

    @ObservedObject private var store = ChatRuntimeStore.shared
    @ObservedObject private var registry = RuntimeRegistryStore.shared
    @ObservedObject private var modelStore = ModelStore.shared
    @State private var isRuntimePickerPresented = false

    private var runtime: RuntimeEntry? { registry.runtime(store.runtimeId) }
    private var runtimeName: String { runtime?.name ?? "Allternit" }
    private var providerId: String? {
        guard let id = modelStore.selectedModelId, let slash = id.firstIndex(of: "/") else { return nil }
        return String(id[..<slash])
    }
    private var brainName: String {
        if modelStore.isLoading && modelStore.models.isEmpty { return "Loading…" }
        return ComposerModelLabel.label(modelStore)
    }
    /// The picked brain can't run in the picked runtime.
    private var brainMisfit: Bool {
        guard let runtime, let providerId else { return false }
        return !RuntimeRegistry.fits(runtime, providerId: providerId)
    }

    var body: some View {
        ViewThatFits(in: .horizontal) {
            row(namedRuntime: true, namedBrain: true)
            row(namedRuntime: false, namedBrain: true)
            row(namedRuntime: false, namedBrain: false)
        }
        .sheet(isPresented: $isRuntimePickerPresented) {
            ChatRuntimePickerSheet(hasActiveSession: hasActiveSession)
        }
        .task {
            registry.refreshIfNeeded()
            modelStore.fetchModelsIfNeeded()
        }
    }

    private func row(namedRuntime: Bool, namedBrain: Bool) -> some View {
        HStack(spacing: 4) {
            chip(
                label: runtimeName, named: namedRuntime, maxChars: 13,
                mark: AnyView(ModelProviderMark(providerId: store.runtimeId, size: 18)),
                a11y: "Runtime: \(runtimeName)", hint: "Shows the runtime choices", id: "composerChip-runtime"
            ) {
                isRuntimePickerPresented = true
            }
            chip(
                label: brainName, named: namedBrain, maxChars: 16,
                mark: AnyView(ModelProviderMark(providerId: providerId ?? store.runtimeId, size: 18)),
                a11y: "Brain: \(brainName)\(brainMisfit ? ", can't run in \(runtimeName)" : "")",
                hint: "Shows the brain choices", id: "composerChip-brain", warn: brainMisfit
            ) {
                onBrain()
            }
        }
        .fixedSize()
    }

    private func chip(label: String, named: Bool, maxChars: Int, mark: AnyView, a11y: String, hint: String,
                      id: String, warn: Bool = false, action: @escaping () -> Void) -> some View {
        Button {
            Haptics.selection()
            action()
        } label: {
            HStack(spacing: 4) {
                mark.accessibilityHidden(true)
                if named {
                    Text(ComposerAgentChips.short(label, maxChars))
                        .font(.system(size: 13, weight: .medium))
                        .lineLimit(1)
                }
                if warn {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundColor(Theme.statusWarning)
                        .accessibilityHidden(true)
                }
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .opacity(0.7)
            }
            .foregroundColor(Color("TextSecondary"))
            .padding(.horizontal, 4)
            .frame(minWidth: 44, minHeight: 44)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(a11y)
        .accessibilityHint(hint)
        .accessibilityIdentifier(id)
    }
}

/// The plain chat's runtime choices. Every runtime stays listed; one that
/// can't run yet says why, with the way to fix it.
struct ChatRuntimePickerSheet: View {
    let hasActiveSession: Bool

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var store = ChatRuntimeStore.shared
    @ObservedObject private var registry = RuntimeRegistryStore.shared
    @State private var query = ""
    @State private var focused: String?
    @State private var isPairingPresented = false

    private var choices: [RuntimeEntry] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        return registry.runtimes
            .filter { !$0.more || $0.id == store.runtimeId || !q.isEmpty }
            .filter { q.isEmpty || [$0.name, $0.vendor, $0.categoryLabel].joined(separator: " ").lowercased().contains(q) }
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Where this chat runs. Allternit runs anywhere; the others run on your computer.")
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .fixedSize(horizontal: false, vertical: true)
                    searchField
                    switch registry.status {
                    case .error: notice("Couldn't load the latest runtimes. Showing the built-in list.")
                    case .stale: notice("You're offline. Showing the saved list of runtimes.")
                    default: EmptyView()
                    }
                    if choices.isEmpty {
                        Text("No runtimes match “\(query)”.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        VStack(spacing: 0) {
                            ForEach(Array(choices.enumerated()), id: \.element.id) { i, r in
                                if i > 0 { PhoneCardDivider() }
                                row(r)
                            }
                        }
                        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).stroke(Color("BorderSubtle")))
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .scrollDismissesKeyboard(.interactively)
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Runtime")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }.fontWeight(.semibold)
                }
            }
            .refreshable { await store.refresh() }
            .task {
                registry.refreshIfNeeded()
                store.refreshIfNeeded()
            }
            .sheet(isPresented: $isPairingPresented, onDismiss: { store.refreshIfNeeded(force: true) }) {
                RuntimePairingView()
            }
        }
    }

    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .foregroundColor(Color("TextTertiary"))
                .accessibilityHidden(true)
            TextField("Search runtimes", text: $query)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
        }
        .padding(.horizontal, 12)
        .frame(minHeight: 44)
        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("FillNeutral")))
    }

    private func notice(_ text: String) -> some View {
        Label(text, systemImage: "info.circle")
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
    }

    private func row(_ r: RuntimeEntry) -> some View {
        let availability = store.availability(r, installed: registry.installed)
        let selected = r.id == store.runtimeId
        let showNote = focused == r.id || (selected && availability != .ready)
        return VStack(alignment: .leading, spacing: 8) {
            Button {
                Haptics.selection()
                if availability == .ready {
                    store.start(r, chatHasSession: hasActiveSession && !selected)
                    dismiss()
                } else {
                    focused = r.id
                }
            } label: {
                HStack(spacing: 12) {
                    ModelProviderMark(providerId: r.id, size: 28)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(r.name)
                            .font(.body.weight(.medium))
                            .foregroundColor(Color("TextPrimary"))
                        Text(detail(r, availability))
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: 8)
                    if availability == .checking {
                        ProgressView().controlSize(.small)
                    } else if selected {
                        Image(systemName: "checkmark")
                            .font(.body.weight(.semibold))
                            .foregroundColor(Color("TextPrimary"))
                            .accessibilityHidden(true)
                    }
                }
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(r.name), \(detail(r, availability))")
            .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
            .accessibilityIdentifier("chatRuntime-\(r.id)")

            if showNote, let message = availability.message(r.name) {
                VStack(alignment: .leading, spacing: 8) {
                    Label(message, systemImage: availability == .needsComputer ? "desktopcomputer" : "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundColor(Color("TextPrimary"))
                        .fixedSize(horizontal: false, vertical: true)
                    HStack(spacing: 8) {
                        if availability == .needsComputer {
                            actionButton("Pair a computer") { isPairingPresented = true }
                        }
                        actionButton("Check again") { store.refreshIfNeeded(force: true) }
                    }
                }
                .padding(.leading, 40)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
    }

    private func detail(_ r: RuntimeEntry, _ a: ChatRuntimeAvailability) -> String {
        switch a {
        case .ready: return r.id == "allternit" ? "Runs anywhere · \(r.brainsLabel)" : "\(r.categoryLabel) · \(r.brainsLabel)"
        case .checking: return "Checking your computer…"
        case .needsComputer: return "Needs a computer"
        case .notInstalled(let computer): return "Not installed on \(computer)"
        }
    }

    private func actionButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.footnote.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 14)
                .frame(minHeight: 44)
                .background(Capsule().fill(Color("FillNeutral")))
                .overlay(Capsule().stroke(Color("BorderSubtle")))
        }
        .buttonStyle(.plain)
    }
}
