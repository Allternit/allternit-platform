import SwiftUI
import UIKit
import AllternitFilesKit

// The four custom actions on an Allternit file in Files (design 5B): Send
// to agent, Share with…, Summarize, Keep offline. Copy, Move, Rename and
// Delete stay Apple's own.

struct ActionSheet: View {
    let context: SheetContext
    let action: FilesUIAction
    let itemId: ItemId

    var body: some View {
        switch action {
        case .sendToAgent: AgentSheet(context: context, itemId: itemId)
        case .share: ShareSheet(context: context, itemId: itemId)
        case .summarize: SummarySheet(context: context, itemId: itemId)
        case .keepOffline: OfflineSheet(context: context, itemId: itemId, keep: true)
        case .stopOffline: OfflineSheet(context: context, itemId: itemId, keep: false)
        }
    }
}

/// Plain words for a failure, with the computer's name.
func describe(_ error: Error, computer: String) -> String {
    guard let files = error as? FilesError else {
        if error is URLError { return "\(computer) isn't reachable right now. Check your connection." }
        return error.localizedDescription
    }
    switch files.code {
    case .asleep, .offline: return "\(computer) is asleep or offline."
    case .featureDisabled: return "Files isn't turned on for this account."
    case .forbidden: return files.body?.message ?? "You can't do that here."
    case .unsupported: return files.body?.message ?? "\(computer) can't do that yet."
    case .protectedRootLocked: return "This folder is protected. Open it in Files and unlock it first."
    default: return files.localizedDescription
    }
}

// MARK: - Send to agent

/// Copies the file into the computer's Inbox. The drop makes a task that
/// waits for approval (D13, step 17): nothing reads the file until the user
/// taps Approve, here or later in the app.
struct AgentSheet: View {
    let context: SheetContext
    let itemId: ItemId

    enum Phase: Equatable {
        case loading
        case confirm
        case sending
        case wake(WakeHint?)
        case waiting(FilesInboxTask?)
        case approving(FilesInboxTask)
        case approved
        case failed(String)
    }

    @State private var phase: Phase = .loading
    @State private var item: Item?
    @State private var inbox: Root?
    @StateObject private var waker = WakeRunner()

    private var name: String { item?.name ?? "this file" }

    var body: some View {
        SheetScaffold(symbol: phase == .approved ? "checkmark" : "sparkles",
                      tint: phase == .approved ? SheetColor.success : SheetColor.info,
                      title: title, subtitle: context.computerName) {
            content
        } actions: {
            actions
        }
        .animation(.easeOut(duration: 0.18), value: phase)
        .task { await load() }
    }

    private var title: String {
        switch phase {
        case .waiting, .approving: return "Waiting for your OK"
        case .approved: return "Approved"
        case .wake: return "Wake your cloud computer?"
        default: return "Send to agent"
        }
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .loading:
            ProgressView().frame(maxWidth: .infinity, minHeight: 80)
        case .confirm, .sending:
            SheetCard {
                Label(name, systemImage: "doc").font(.system(size: 15, weight: .medium)).foregroundStyle(SheetColor.text)
                SheetText("A copy goes to Inbox on \(context.computerName). Your agent reads it, and nothing runs until you approve.")
            }
        case .wake:
            WakeCard(computerName: context.computerName, remember: $waker.remember)
        case .waiting(let task):
            waiting(hasTask: task != nil)
        case .approving:
            waiting(hasTask: true)
        case .approved:
            StatusLine(text: "Running", color: SheetColor.info)
            SheetText("Your agent is reading \(name). The summary is saved next to it in Inbox, marked AI-generated.")
        case .failed(let message):
            ErrorLine(text: message)
        }
    }

    @ViewBuilder private func waiting(hasTask: Bool) -> some View {
        StatusLine(text: "In Inbox · needs approval", color: SheetColor.warning)
        SheetText(hasTask
                  ? "Nothing reads \(name) until you approve. Approving lets your agent read this one file and save a summary next to it, nothing else."
                  : "\(name) is in Inbox. It waits there until you approve it in Allternit.")
    }

    @ViewBuilder private var actions: some View {
        switch phase {
        case .confirm, .sending:
            SheetButton(title: "Send to agent", symbol: "paperplane.fill", busy: phase == .sending) { Task { await send() } }
            SheetButton(title: "Cancel", primary: false) { context.cancel() }
        case .wake(let hint):
            SheetButton(title: "Wake and send", symbol: "bolt.fill", busy: waker.phase == .waking) {
                Task {
                    if await waker.wake(context, hint: hint, reason: .sync, itemId: itemId) { await send() }
                    else if case .failed(let message) = waker.phase { phase = .failed(message) }
                }
            }
            SheetButton(title: "Not now", primary: false) { context.cancel() }
        case .waiting(let task):
            if let task {
                SheetButton(title: "Approve", symbol: "checkmark") { Task { await approve(task) } }
            }
            SheetButton(title: "Later", primary: false) { context.finish() }
        case .approving:
            SheetButton(title: "Approve", busy: true) {}
        case .approved, .failed:
            SheetButton(title: "Done") { context.finish() }
        case .loading:
            SheetButton(title: "Cancel", primary: false) { context.cancel() }
        }
    }

    private func load() async {
        guard let client = context.client else { phase = .failed("Sign in to the Allternit app first."); return }
        do {
            async let stat = client.stat(itemId)
            async let roots = client.roots()
            let (found, list) = try await (stat, roots)
            item = found.item
            inbox = list.roots.first { $0.kind == .inbox && $0.policy.agentProcess && $0.policy.write }
            guard let inbox else {
                phase = .failed("\(context.computerName) has no Inbox that agents may use.")
                return
            }
            if found.item.rootId == inbox.id {
                // Already in Inbox: it has a task of its own.
                phase = .waiting(await task(for: found.item.id))
            } else {
                phase = .confirm
            }
        } catch {
            phase = .failed(describe(error, computer: context.computerName))
        }
    }

    private func send() async {
        guard let client = context.client, let inbox else { return }
        phase = .sending
        do {
            let copied = try await client.copy(CopyRequest(itemId: itemId, destParentId: inbox.rootItemId, onConflict: .keepBoth))
            guard let copy = copied.item else {
                // A big copy runs as a job: the task appears when it lands.
                phase = .waiting(nil)
                return
            }
            phase = .waiting(await task(for: copy.id))
        } catch FilesError.wakeConsentRequired(let hint, _) {
            phase = .wake(hint)
        } catch {
            phase = .failed(describe(error, computer: context.computerName))
        }
    }

    /// The drop's task (the runtime records it right after the write; a few
    /// short tries cover the watcher's delay).
    private func task(for copyId: ItemId) async -> FilesInboxTask? {
        guard let client = context.client else { return nil }
        for attempt in 0..<4 {
            if let tasks = try? await client.inboxTasks(limit: 50),
               let match = tasks.first(where: { $0.itemId == copyId }) {
                return match.needsApproval ? match : nil
            }
            if attempt < 3 { try? await Task.sleep(nanoseconds: 800_000_000) }
        }
        return nil
    }

    private func approve(_ task: FilesInboxTask) async {
        guard let client = context.client else { return }
        phase = .approving(task)
        do {
            _ = try await client.approveInboxTask(task.taskId)
            phase = .approved
        } catch {
            phase = .failed(describe(error, computer: context.computerName))
        }
    }
}

// MARK: - Share with…

/// One form (design 4A/5B): guest email, role, expiry, guest wake, then a
/// link to copy or send. cloud-api signs the link; the computer re-checks
/// the share on every guest request, and a revoke applies at once.
struct ShareSheet: View {
    let context: SheetContext
    let itemId: ItemId

    enum Expiry: Hashable {
        case day, week, month, open

        var seconds: TimeInterval? {
            switch self {
            case .day: return 86_400
            case .week: return 7 * 86_400
            case .month: return 30 * 86_400
            case .open: return nil
            }
        }

        var phrase: String {
            switch self {
            case .day: return "24 hours"
            case .week: return "7 days"
            case .month: return "30 days"
            case .open: return "as long as you allow"
            }
        }
    }

    @State private var item: Item?
    @State private var email = ""
    @State private var role: ShareRole = .viewDownload
    @State private var expiry: Expiry = .week
    @State private var guestWake = false
    @State private var busy = false
    @State private var share: Share?
    @State private var failure: String?
    @State private var copied = false

    private var trimmedEmail: String { email.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var emailValid: Bool {
        trimmedEmail.isEmpty || trimmedEmail.range(of: #"^[^@\s]+@[^@\s]+\.[^@\s]+$"#, options: .regularExpression) != nil
    }
    private var isFolder: Bool { item?.kind == .folder }

    var body: some View {
        SheetScaffold(symbol: share == nil ? "person.2.fill" : "link", tint: SheetColor.text,
                      title: "Share “\(item?.name ?? "…")”", subtitle: context.computerName) {
            if let share {
                StatusLine(text: trimmedEmail.isEmpty ? "Link ready" : "Invite sent to \(trimmedEmail)", color: SheetColor.success)
                SheetCard {
                    Text(share.linkUrl)
                        .font(.system(size: 14, design: .monospaced))
                        .foregroundStyle(SheetColor.text)
                        .lineLimit(2)
                        .textSelection(.enabled)
                    SheetText(summary)
                }
            } else if item == nil && failure == nil {
                ProgressView().frame(maxWidth: .infinity, minHeight: 80)
            } else if item != nil {
                form
            }
            if let failure { ErrorLine(text: failure) }
        } actions: {
            if let share, let url = URL(string: share.linkUrl) {
                SheetButton(title: copied ? "Copied" : "Copy link", symbol: copied ? "checkmark" : "doc.on.doc") {
                    UIPasteboard.general.url = url
                    copied = true
                }
                ShareLink(item: url) {
                    Text("Send link…").font(.system(size: 17, weight: .semibold))
                        .frame(maxWidth: .infinity, minHeight: 50)
                        .foregroundStyle(SheetColor.text)
                        .background(SheetColor.fill, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                }
                SheetButton(title: "Done", primary: false) { context.finish() }
            } else {
                SheetButton(title: trimmedEmail.isEmpty ? "Create link" : "Send invite", busy: busy,
                            enabled: item != nil && emailValid) { Task { await create() } }
                SheetButton(title: "Cancel", primary: false) { context.cancel() }
            }
        }
        .task { await load() }
        .onChange(of: isFolder) { _, folder in if !folder && role == .uploadOnly { role = .viewDownload } }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Guest email (optional)").font(.system(size: 13, weight: .medium)).foregroundStyle(SheetColor.secondary)
                TextField("name@example.com", text: $email)
                    .textContentType(.emailAddress)
                    .keyboardType(.emailAddress)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(size: 16))
                    .padding(.horizontal, 12)
                    .frame(minHeight: 44)
                    .background(SheetColor.fill, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .stroke(emailValid ? .clear : SheetColor.error, lineWidth: 1))
            }
            SheetSegment(label: "Role", options: roles, value: $role)
            SheetSegment(label: "Link works for", options: [(.day, "24h"), (.week, "7d"), (.month, "30d"), (.open, "No end")], value: $expiry)
            Toggle(isOn: $guestWake) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Guests can wake \(context.computerName)").font(.system(size: 15)).foregroundStyle(SheetColor.text)
                    Text("Waking bills your credits. Off: guests see the saved list while it sleeps.")
                        .font(.system(size: 13)).foregroundStyle(SheetColor.secondary)
                }
            }
            .tint(SheetColor.text)
            SheetText(summary)
        }
    }

    private var roles: [(ShareRole, String)] {
        isFolder ? [(.viewDownload, "View"), (.uploadOnly, "Upload"), (.comment, "Comment"), (.edit, "Edit")]
                 : [(.viewDownload, "View"), (.comment, "Comment"), (.edit, "Edit")]
    }

    private var summary: String {
        let who = trimmedEmail.isEmpty ? "Anyone with the link" : trimmedEmail
        let verb: String
        switch role {
        case .viewDownload: verb = "see and download"
        case .uploadOnly: verb = "drop files into"
        case .comment: verb = "view and comment on"
        case .edit: verb = "edit"
        }
        return "\(who) can \(verb) “\(item?.name ?? "this")” for \(expiry.phrase). Sharing never lets an agent act on these files."
    }

    private func load() async {
        guard let client = context.client else { failure = "Sign in to the Allternit app first."; return }
        do {
            item = try await client.stat(itemId).item
        } catch {
            failure = describe(error, computer: context.computerName)
        }
    }

    private func create() async {
        guard let client = context.client, let item else { return }
        busy = true
        failure = nil
        defer { busy = false }
        let expiresAt = expiry.seconds.map { ISO8601DateFormatter().string(from: Date().addingTimeInterval($0)) }
        let request = CreateShareRequest(rootId: item.rootId, itemId: item.id, role: role, expiresAt: expiresAt,
                                         guestWake: guestWake, inviteEmails: trimmedEmail.isEmpty ? nil : [trimmedEmail])
        do {
            share = try await client.createShare(request).share
            context.signalChanges()
        } catch let error as FilesError where error.code == .forbidden {
            failure = "This folder can't be shared. Turn on sharing for it in Allternit first."
        } catch {
            failure = describe(error, computer: context.computerName)
        }
    }
}

// MARK: - Summarize

/// An extractive summary made on the computer (sentences picked from the
/// file, no model, step 16), labelled AI-generated.
struct SummarySheet: View {
    let context: SheetContext
    let itemId: ItemId

    enum Phase: Equatable { case working, wake(WakeHint?), done(FilesSummary), failed(String) }

    @State private var phase: Phase = .working
    @State private var copied = false
    @StateObject private var waker = WakeRunner()

    var body: some View {
        SheetScaffold(symbol: "text.append", tint: SheetColor.info,
                      title: { if case .wake = phase { return "Wake your cloud computer?" } else { return "Summary" } }(),
                      subtitle: context.computerName) {
            switch phase {
            case .working:
                HStack(spacing: 10) {
                    ProgressView()
                    SheetText("Reading the file on \(context.computerName)…")
                }
            case .wake:
                WakeCard(computerName: context.computerName, remember: $waker.remember)
            case .done(let summary):
                SheetCard {
                    Text(summary.summary)
                        .font(.system(size: 16))
                        .foregroundStyle(SheetColor.text)
                        .lineSpacing(3)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Label("AI-generated · sentences picked from the file. Check it before you rely on it.", systemImage: "sparkles")
                    .font(.system(size: 13)).foregroundStyle(SheetColor.secondary)
            case .failed(let message):
                ErrorLine(text: message)
            }
        } actions: {
            switch phase {
            case .done(let summary):
                SheetButton(title: copied ? "Copied" : "Copy summary", symbol: copied ? "checkmark" : "doc.on.doc") {
                    UIPasteboard.general.string = summary.summary
                    copied = true
                }
                SheetButton(title: "Done", primary: false) { context.finish() }
            case .wake(let hint):
                SheetButton(title: "Wake and summarize", symbol: "bolt.fill", busy: waker.phase == .waking) {
                    Task {
                        if await waker.wake(context, hint: hint, reason: .openFile, itemId: itemId) { await run() }
                        else if case .failed(let message) = waker.phase { phase = .failed(message) }
                    }
                }
                SheetButton(title: "Not now", primary: false) { context.cancel() }
            case .working:
                SheetButton(title: "Cancel", primary: false) { context.cancel() }
            case .failed:
                SheetButton(title: "Done") { context.finish() }
            }
        }
        .animation(.easeOut(duration: 0.18), value: phase)
        .task { await run() }
    }

    private func run() async {
        guard let client = context.client else { phase = .failed("Sign in to the Allternit app first."); return }
        phase = .working
        do {
            phase = .done(try await client.summarize(itemId: itemId))
        } catch FilesError.wakeConsentRequired(let hint, _) {
            phase = .wake(hint)
        } catch let error as FilesError where error.code == .unsupported {
            phase = .failed("Only text, PDF and Word files can be summarized so far.")
        } catch {
            phase = .failed(describe(error, computer: context.computerName))
        }
    }
}

// MARK: - Keep offline

/// Pins the file on this iPhone (`POST /pin` for this device plus the
/// shared pin list) and downloads it now. iOS has no keep-downloaded policy,
/// so the app re-reads pinned files that iOS evicts (`FilesPinKeeper`). A
/// pin never wakes a sleeping computer: it is kept on the phone and sent
/// with the next request that reaches it.
struct OfflineSheet: View {
    let context: SheetContext
    let itemId: ItemId
    let keep: Bool

    enum Phase: Equatable { case working, downloading, done(String), failed(String) }

    @State private var phase: Phase = .working
    @State private var name = "This file"

    var body: some View {
        SheetScaffold(symbol: keep ? "pin.fill" : "pin.slash", tint: keep ? SheetColor.text : SheetColor.secondary,
                      title: keep ? "Keep offline" : "Don’t keep offline", subtitle: context.computerName) {
            switch phase {
            case .working:
                ProgressView().frame(maxWidth: .infinity, minHeight: 60)
            case .downloading:
                HStack(spacing: 10) {
                    ProgressView()
                    SheetText("Downloading \(name) to this iPhone…")
                }
            case .done(let message):
                StatusLine(text: keep ? "On this iPhone" : "Not kept offline", color: keep ? SheetColor.success : SheetColor.secondary)
                SheetText(message)
            case .failed(let message):
                ErrorLine(text: message)
            }
        } actions: {
            switch phase {
            case .working, .downloading:
                SheetButton(title: "Done", primary: false) { context.finish() }
            default:
                SheetButton(title: "Done") { context.finish() }
            }
        }
        .animation(.easeOut(duration: 0.18), value: phase)
        .task { await run() }
    }

    private func run() async {
        guard let client = context.client, let computerId = context.computerId else {
            phase = .failed("Sign in to the Allternit app first.")
            return
        }
        if let stat = try? await client.stat(itemId) { name = stat.item.name }
        var reachedComputer = true
        do {
            _ = try await client.pin(itemId: itemId, pinned: keep)
        } catch let error as FilesError where error.isComputerUnavailable || error.code == .wakeConsentRequired {
            // Asleep: keep the pin on this phone; never wake for a pin.
            context.store.setPinned(keep, computerId: computerId, itemId: itemId)
            context.store.clearPendingWake(computerId: computerId)
            reachedComputer = false
        } catch {
            phase = .failed(describe(error, computer: context.computerName))
            return
        }
        context.signalChanges()
        guard keep else {
            phase = .done("\(name) stays in Files. iOS may remove the copy on this iPhone when space runs low.")
            return
        }
        phase = .downloading
        let downloaded = await FilesPinKeeper.materialize(computerId: computerId, itemId: itemId)
        if !reachedComputer {
            phase = .done("\(name) stays on this iPhone. \(context.computerName) is asleep, so it downloads the next time it is awake and Allternit is open.")
        } else if downloaded {
            phase = .done("\(name) stays on this iPhone, and Allternit downloads it again if iOS removes it.")
        } else {
            phase = .done("\(name) downloads the next time you open Allternit, then stays on this iPhone.")
        }
    }
}
