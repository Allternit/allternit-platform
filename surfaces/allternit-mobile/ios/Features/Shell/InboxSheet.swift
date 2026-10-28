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

    private let client = InboxClient.shared

    func refresh() async {
        do {
            if let list = try await client.list() {
                items = list
                isAvailable = true
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

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Needs you")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Done") { dismiss() }
                    }
                }
                .background(Color("BgPrimary").ignoresSafeArea())
        }
        .task { await store.refresh() }
    }

    @ViewBuilder
    private var content: some View {
        if !store.hasLoaded {
            ProgressView("Checking what needs you…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if !store.isAvailable {
            FriendlyStateView(
                style: .offline,
                icon: "laptopcomputer",
                title: "No inbox on this computer yet",
                message: "The inbox comes from your computer. Connect one, or update Allternit on it, to see what needs you."
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let error = store.error, store.items.isEmpty {
            FriendlyStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load the inbox",
                message: error,
                actionTitle: "Retry",
                action: { Task { await store.refresh() } }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if store.items.isEmpty {
            FriendlyStateView(
                style: .empty,
                icon: "bell.slash",
                title: "You're all caught up",
                message: "Approvals, held messages, missed calls, unanswered people and anything that failed show up here."
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            List {
                ForEach(store.items) { item in
                    InboxRow(item: item, onOpen: { open(item) })
                        .listRowBackground(Color("BgPrimary"))
                        .swipeActions(edge: .leading) {
                            Button(item.primary.label) { primary(item) }
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
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .refreshable { await store.refresh() }
        }
    }

    private func primary(_ item: InboxItem) {
        if item.primary.action == "approve", item.canApprove {
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

    @ObservedObject private var store = InboxStore.shared

    private static let words: [String: String] = [
        "email_draft": "Email", "approval": "Approval", "held_send": "Held message", "thread": "Waiting",
        "missed_call": "Missed call", "unanswered": "Unanswered", "failed_send": "Not delivered",
        "vendor_result": "Vendor result", "memory_import": "Memory", "attention": "Needs fixing",
        "suggestion": "Suggested",
    ]

    private static let trust: [String: String] = [
        "owner": "you", "team": "your team", "contact": "a contact", "vendorbot": "a vendor bot",
        "vendor_bot": "a vendor bot", "tooloutput": "a tool result", "tool_output": "a tool result",
        "web": "a web page", "stranger": "someone you do not know",
    ]

    var body: some View {
        let busy = store.busy.contains(item.id)
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text((item.isPeerHeld ? "Other agent" : Self.words[item.kind] ?? item.kind).uppercased()
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
            Text(item.summary)
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
                Button(busy ? "Working…" : item.primary.label) { primaryTapped() }
                    .buttonStyle(.borderedProminent)
                    .tint(Color("AccentPrimary"))
                    .controlSize(.small)
                    .disabled(busy)
                if item.canApprove, item.primary.action == "approve" {
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
        if item.primary.action == "approve", item.canApprove {
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
