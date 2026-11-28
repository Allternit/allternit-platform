import SwiftUI

/// "Needs you" state for the drawer's Inbox button and sheet (the web's
/// src/lib/inbox/inbox-store.ts). An action removes the row at once; a
/// failure puts it back with the reason.
@MainActor
final class InboxStore: ObservableObject {
    static let shared = InboxStore()

    @Published private(set) var items: [InboxItem] = []
    @Published private(set) var hasLoaded = false
    /// false when the computer's runtime has no inbox yet.
    @Published private(set) var isAvailable = true
    @Published private(set) var error: String?
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var busy: Set<String> = []
    /// The item a link or notification tap asked for (`allternit://inbox/<id>`).
    @Published var focusedId: String?
    /// AllternitOS kernel approvals waiting on you (cloud relay).
    @Published private(set) var kernelItems: [KernelApproval] = []
    /// The kernel approval a `allternit://inbox/kernel/...` link asked for (`KernelApproval.id`).
    @Published var focusedKernel: String?

    /// Answers one kernel approval. The row leaves only after the relay accepts;
    /// a failure returns the plain reason to the detail sheet.
    func resolveKernel(_ approval: KernelApproval, approve: Bool) async -> String? {
        do {
            try await client.resolveKernel(approval, approve: approve)
            kernelItems.removeAll { $0.id == approval.id }
            return nil
        } catch is CancellationError {
            return nil
        } catch {
            if let known = error as? KernelResolveError, known == .tokenExpired {
                kernelItems.removeAll { $0.id == approval.id }
            }
            return (error as? KernelResolveError)?.errorDescription ?? FriendlyErrorMessage.from(error.localizedDescription)
        }
    }

    /// A notification button already answered this item in the background.
    func removeLocally(_ id: String) {
        guard !id.isEmpty else { return }
        items.removeAll { $0.id == id }
    }

    private let client = InboxClient.shared

    func refresh() async {
        // Kernel approvals come from the cloud, not the computer's runtime inbox;
        // a failure here leaves the last list rather than hiding the rest.
        if let kernel = try? await client.kernelApprovals() { kernelItems = kernel }
        do {
            if let list = try await client.list() {
                items = list
                isAvailable = true
                AppGroupStore.hasComputer = true
            } else {
                items = []
                isAvailable = false
            }
            error = nil
        } catch is CancellationError {
        } catch RuntimeRelayError.noComputer {
            items = []
            isAvailable = false
            error = nil
            AppGroupStore.hasComputer = false
        } catch {
            self.error = FriendlyErrorMessage.from(error.localizedDescription)
        }
        hasLoaded = true
    }

    func act(_ item: InboxItem, _ run: @escaping @Sendable (InboxClient) async throws -> Void) {
        guard !busy.contains(item.id) else { return }
        busy.insert(item.id)
        failures[item.id] = nil
        let index = items.firstIndex(of: item)
        items.removeAll { $0.id == item.id }
        let client = self.client
        Task {
            do {
                try await run(client)
                Task { @MainActor in await StatusSnapshotWriter.refreshFromCloud() }
            } catch {
                if let index { items.insert(item, at: min(index, items.count)) } else { items.append(item) }
                failures[item.id] = FriendlyErrorMessage.from(error.localizedDescription)
            }
            busy.remove(item.id)
        }
    }
}

/// The drawer's Inbox: everything across the bots that waits on you, with
/// the main action on the row. Swipe right for it, left to snooze or dismiss.
struct InboxSheet: View {
    @ObservedObject private var store = InboxStore.shared
    @Environment(\.dismiss) private var dismiss
    /// A phone hold open in its Review pane (Review only, never approved from the row).
    @State private var reviewing: InboxItem?
    @State private var isConnectOpen = false
    /// A kernel approval open in its detail (the exact action, whole).
    @State private var kernelDetail: KernelApproval?

    var body: some View {
        NavigationStack {
            content
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Done") { dismiss() }
                    }
                }
                .background(Color("BgPrimary").ignoresSafeArea())
        }
        .task {
            await store.refresh()
            // A hold's notification Review action lands here: open its pane.
            if let id = store.focusedId, let item = store.items.first(where: { $0.id == id }), item.isPhoneAction {
                reviewing = item
            }
            // A KERNEL_APPROVAL push lands here: open its detail to read the action.
            if let key = store.focusedKernel {
                kernelDetail = store.kernelItems.first { $0.id == key }
            }
        }
        .sheet(item: $kernelDetail) { KernelApprovalSheet(approval: $0) }
        .sheet(item: $reviewing) { PhoneHoldReviewSheet(item: $0) }
        .sheet(isPresented: $isConnectOpen) { ConnectComputerSheet() }
        .onDisappear { store.focusedId = nil; store.focusedKernel = nil }
    }

    /// The linked item is no longer waiting (answered elsewhere, or gone).
    private var focusedIsGone: Bool {
        guard store.hasLoaded, store.error == nil else { return false }
        if let key = store.focusedKernel { return !store.kernelItems.contains { $0.id == key } }
        guard let id = store.focusedId else { return false }
        return !store.items.contains { $0.id == id }
    }

    @ViewBuilder
    private var content: some View {
        if store.hasLoaded, store.isAvailable, !(store.error != nil && store.items.isEmpty && store.kernelItems.isEmpty), !(store.items.isEmpty && store.kernelItems.isEmpty) {
            list
        } else {
            DockingTitleScrollView(title: "Needs you", chrome: .navigationBar) { state }
        }
    }

    /// Loading, no computer, error and all-caught-up under the docking title.
    @ViewBuilder
    private var state: some View {
        if !store.hasLoaded {
            ProgressView("Checking what needs you…")
                .frame(maxWidth: .infinity, minHeight: 360)
        } else if !store.isAvailable, store.kernelItems.isEmpty {
            FriendlyStateView(
                style: .offline,
                icon: "laptopcomputer",
                title: "No inbox on this computer yet",
                message: "The inbox comes from your computer. Connect one, or update Allternit on it, to see what needs you.",
                actionTitle: "Connect",
                action: { isConnectOpen = true },
                scene: .asleep
            )
            .frame(maxWidth: .infinity, minHeight: 360)
        } else if let error = store.error, store.items.isEmpty, store.kernelItems.isEmpty {
            FriendlyStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load the inbox",
                message: error,
                actionTitle: "Retry",
                action: { Task { await store.refresh() } }
            )
            .frame(maxWidth: .infinity, minHeight: 360)
        } else if store.items.isEmpty, store.kernelItems.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: "bell.slash",
                title: focusedIsGone ? NotificationStrings.alreadyHandled : "You're all caught up",
                message: "Approvals, held messages, missed calls, unanswered people and anything that failed show up here.",
                art: .bell
            )
            .frame(maxWidth: .infinity, minHeight: 360)
        }
    }

    private var list: some View {
        Group {
            ScrollViewReader { proxy in
            DockingTitleList(title: "Needs you") {
                if focusedIsGone { alreadyHandledNote }
                ForEach(store.kernelItems) { approval in
                    KernelApprovalRow(approval: approval) { kernelDetail = approval }
                        .id(approval.id)
                        .listRowBackground(approval.id == store.focusedKernel ? Color("AccentPrimary").opacity(0.12) : Color("BgPrimary"))
                }
                ForEach(store.items) { item in
                    InboxRow(item: item, onOpen: { open(item) }, onReview: { reviewing = item })
                        .id(item.id)
                        .listRowBackground(item.id == store.focusedId ? Color("AccentPrimary").opacity(0.12) : Color("BgPrimary"))
                        .swipeActions(edge: .leading) {
                            Button(item.isPhoneAction ? "Review" : item.primary.label) { primary(item) }
                                .tint(Color("AccentPrimary"))
                        }
                        .swipeActions(edge: .trailing) {
                            Button("Dismiss", role: .destructive) {
                                store.act(item) { try await $0.dismiss(item.id) }
                            }
                            Button("Snooze") {
                                store.act(item) { try await $0.snooze(item.id) }
                            }
                            .tint(.gray)
                        }
                }
            }
            .refreshable { await store.refresh() }
            .onAppear {
                if let id = store.focusedId, store.items.contains(where: { $0.id == id }) {
                    proxy.scrollTo(id, anchor: .center)
                }
            }
            }
        }
    }

    private var alreadyHandledNote: some View {
        Text(NotificationStrings.alreadyHandled)
            .font(.footnote)
            .foregroundColor(Color("TextSecondary"))
            .frame(maxWidth: .infinity, alignment: .center)
            .listRowBackground(Color("BgPrimary"))
            .listRowSeparator(.hidden)
            .accessibilityLabel("That item was already handled")
    }

    private func primary(_ item: InboxItem) {
        if item.isPhoneAction {
            reviewing = item
        } else if item.primary.action == "approve", item.canApprove {
            store.act(item) { try await $0.approve(item.id) }
        } else if item.primary.action == "retry", item.canRetry {
            store.act(item) { try await $0.retry(item.id) }
        } else {
            open(item)
        }
    }

    /// Opens where the item lives: the bot's page (its threads and channels),
    /// or Settings for memories waiting to move to the Memory Drive. The
    /// sheet closes first so the link's own sheet can present.
    private func open(_ item: InboxItem) {
        let link: DeepLink = item.kind == "memory_import" ? .settings("memory") : .bot(item.botId)
        dismiss()
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) {
            DeepLinkRouter.shared.enqueue(link)
        }
    }
}

private struct InboxRow: View {
    let item: InboxItem
    let onOpen: () -> Void
    var onReview: () -> Void = {}

    @ObservedObject private var store = InboxStore.shared

    private static let words: [String: String] = [
        "email_draft": "Email", "approval": "Approval", "held_send": "Held message", "thread": "Waiting",
        "missed_call": "Missed call", "unanswered": "Unanswered", "failed_send": "Not delivered",
        "vendor_result": "Vendor result", "memory_import": "Memory", "attention": "Needs fixing",
        "suggestion": "Suggested",
    ]

    private static let trust: [String: String] = [
        "owner": "you", "team": "your team", "contact": "a contact", "vendorbot": "a Connected bot",
        "vendor_bot": "a Connected bot", "tooloutput": "a tool result", "tool_output": "a tool result",
        "web": "a web page", "stranger": "someone you do not know",
    ]

    var body: some View {
        let busy = store.busy.contains(item.id)
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text((item.isPeerHeld ? "Other agent" : item.isPhoneAction ? "Approval · Phone" : Self.words[item.kind] ?? item.kind).uppercased()
                     + (item.isCritical ? " · URGENT" : ""))
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundColor(item.isCritical ? .red : Color("TextSecondary"))
                Text(item.botName + (item.person.map { " · \($0)" } ?? ""))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
                Spacer(minLength: 4)
                Text(Self.ago(item.at))
                    .font(.system(size: 11))
                    .foregroundColor(Color("TextSecondary"))
            }
            Text(AutomationScheduleLogic.displayName(item.summary))
                .font(.subheadline)
                .foregroundColor(Color("TextPrimary"))
            if item.isPeerHeld, let text = item.text, !text.isEmpty {
                Text(text)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.leading, 8)
                    .overlay(alignment: .leading) {
                        Rectangle().fill(Color("BorderSubtle")).frame(width: 2)
                    }
            }
            if let why = whyLine {
                Text(why)
                    .font(.footnote)
                    .foregroundColor(Color("TextSecondary"))
            }
            if let failure = store.failures[item.id] {
                Text(failure)
                    .font(.footnote)
                    .foregroundColor(.red)
            }
            HStack(spacing: 8) {
                Button(busy ? "Working…" : (item.isPhoneAction ? "Review" : item.primary.label)) { primaryTapped() }
                    .buttonStyle(.borderedProminent)
                    .tint(Color("AccentPrimary"))
                    .controlSize(.small)
                    .disabled(busy)
                if item.isPhoneAction || (item.canApprove && item.primary.action == "approve") {
                    Button(denyLabel) {
                        store.act(item) { client in
                            if item.isPeerHeld { try await client.dismiss(item.id) } else { try await client.deny(item.id) }
                        }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(busy)
                }
            }
            .padding(.top, 2)
        }
        .padding(.vertical, 6)
    }

    private var denyLabel: String {
        switch item.kind {
        case "email_draft": return "Don't send"
        case "suggestion": return "Not now"
        default: return item.isPeerHeld ? "Refuse" : "Deny"
        }
    }

    private var whyLine: String? {
        guard let trust = item.provenance?.trust else { return nil }
        let who = Self.trust[trust.lowercased()] ?? trust
        let kind = item.provenance?.sourceKind.map { " (\($0))" } ?? ""
        return "Why: this came from \(who)\(kind)."
    }

    private func primaryTapped() {
        if item.isPhoneAction {
            onReview()
        } else if item.primary.action == "approve", item.canApprove {
            store.act(item) { try await $0.approve(item.id) }
        } else if item.primary.action == "retry", item.canRetry {
            store.act(item) { try await $0.retry(item.id) }
        } else {
            onOpen()
        }
    }

    static func ago(_ iso: String, now: Date = Date()) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        guard let date = formatter.date(from: iso) ?? ISO8601DateFormatter().date(from: iso) else { return "" }
        let minutes = max(0, Int(now.timeIntervalSince(date) / 60))
        if minutes < 1 { return "now" }
        if minutes < 60 { return "\(minutes)m" }
        let hours = Int((Double(minutes) / 60).rounded())
        return hours < 24 ? "\(hours)h" : "\(Int((Double(hours) / 24).rounded()))d"
    }
}


// MARK: - AllternitOS kernel approvals

private enum KernelText {
    static func classLabel(_ raw: String?) -> String {
        guard let raw, !raw.isEmpty else { return "Action" }
        let spaced = raw.replacingOccurrences(of: "_", with: " ").replacingOccurrences(of: "-", with: " ")
        return spaced.prefix(1).uppercased() + spaced.dropFirst()
    }

    /// "12 min left", "Ran out of time", or nil when the kernel gave no deadline.
    static func timeLeft(_ deadline: Date?, now: Date = Date()) -> String? {
        guard let deadline else { return nil }
        let seconds = Int(deadline.timeIntervalSince(now))
        if seconds <= 0 { return "Ran out of time" }
        if seconds < 90 { return "\(seconds) sec left" }
        let minutes = Int((Double(seconds) / 60).rounded())
        if minutes < 60 { return "\(minutes) min left" }
        let hours = Int((Double(minutes) / 60).rounded())
        return hours < 24 ? "\(hours) hr left" : "\(Int((Double(hours) / 24).rounded())) d left"
    }
}

/// A kernel approval in the Needs-you list: the action in a few lines; the
/// whole text and the buttons are in the detail.
private struct KernelApprovalRow: View {
    let approval: KernelApproval
    let onOpen: () -> Void

    var body: some View {
        let k = approval.kernel
        Button(action: onOpen) {
            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text("APPROVAL · \(KernelText.classLabel(k.actionClass).uppercased())")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                    if let who = k.assignee, !who.isEmpty {
                        Text(who)
                            .font(.system(size: 11))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if let left = KernelText.timeLeft(k.deadline) {
                        Text(left)
                            .font(.system(size: 11))
                            .foregroundColor(Color("TextSecondary"))
                    }
                }
                Text(k.actionSummary ?? "An action is waiting for your answer.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(3)
                    .multilineTextAlignment(.leading)
                HStack(spacing: 8) {
                    if k.humanOnly {
                        Label("Only you can approve", systemImage: "person.fill.checkmark")
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    Spacer(minLength: 4)
                    Text("Review")
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(Color("AccentPrimary"))
                }
            }
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityHint("Opens the full action to approve or deny")
    }
}

/// The exact action, never truncated, with Approve and Deny. Both call the
/// relay with the signed-in session; neither is offered on a lock screen.
private struct KernelApprovalSheet: View {
    let approval: KernelApproval

    @ObservedObject private var store = InboxStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var working: Bool?
    @State private var failure: String?

    private var stillPending: Bool { store.kernelItems.contains { $0.id == approval.id } }

    var body: some View {
        let k = approval.kernel
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Exactly what will happen")
                        .font(.footnote.weight(.semibold))
                        .foregroundColor(Color("TextSecondary"))
                    Text(k.actionSummary ?? "No description came with this request. Deny it unless you know what it is.")
                        .font(.body)
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    VStack(alignment: .leading, spacing: 8) {
                        fact("Kind", KernelText.classLabel(k.actionClass))
                        if let who = k.assignee, !who.isEmpty { fact("Asked by", who) }
                        if let left = KernelText.timeLeft(k.deadline) { fact("Time", left) }
                        if let hash = k.actionHash, !hash.isEmpty { fact("Action ID", String(hash.prefix(12))) }
                    }
                    if k.humanOnly {
                        Label("Only you can approve this. An agent cannot approve it for you.", systemImage: "person.fill.checkmark")
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    if let failure {
                        Text(failure)
                            .font(.footnote)
                            .foregroundColor(.red)
                            .accessibilityLabel("Could not answer: \(failure)")
                    }
                    if !stillPending && working == nil {
                        Text(NotificationStrings.alreadyHandled)
                            .font(.footnote)
                            .foregroundColor(Color("TextSecondary"))
                    }
                    HStack(spacing: 10) {
                        Button(working == false ? "Denying…" : "Deny") { answer(approve: false) }
                            .buttonStyle(.bordered)
                        Button(working == true ? "Approving…" : "Approve") { answer(approve: true) }
                            .buttonStyle(.borderedProminent)
                            .tint(Color("AccentPrimary"))
                    }
                    .controlSize(.large)
                    .disabled(working != nil || !stillPending)
                    .padding(.top, 4)
                }
                .padding(20)
            }
            .background(Color("BgPrimary").ignoresSafeArea())
            .navigationTitle("Approval")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Close") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func fact(_ label: String, _ value: String) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(label)
                .font(.footnote)
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 76, alignment: .leading)
            Text(value)
                .font(.footnote)
                .foregroundColor(Color("TextPrimary"))
        }
    }

    private func answer(approve: Bool) {
        guard working == nil else { return }
        working = approve
        failure = nil
        Task {
            let error = await store.resolveKernel(approval, approve: approve)
            working = nil
            if let error { failure = error } else { dismiss() }
        }
    }
}
