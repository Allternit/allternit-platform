import Foundation
import AllternitFilesKit

// Compiled into the app (Core/) and the share extension (project.yml
// AllternitShare sources), so both send to a computer's Inbox the same way.

/// One Inbox task on a computer (`GET /api/v1/files/v2/inbox/tasks`,
/// surfaces/docs/api/files-drive-inbox.mdx). A file that lands in Inbox is
/// untrusted: its task waits in `needs_approval` until the owner approves or
/// rejects it, and nothing reads the file before that.
struct FilesInboxTask: Codable, Sendable, Hashable, Identifiable {
    var taskId: String
    var rootId: String
    var itemId: String
    var name: String
    var size: UInt64?
    var droppedBy: PrincipalRef?
    var untrusted: Bool?
    var state: String
    var output: String?
    var ruleId: String?
    var error: String?
    var resultItemIds: [String]?
    var createdAt: String
    var updatedAt: String?

    var id: String { taskId }
    var needsApproval: Bool { state == "needs_approval" }

    /// "Dana (guest)", "your iPhone", "you".
    var droppedByLabel: String {
        guard let by = droppedBy else { return "Someone" }
        let name = by.displayName?.isEmpty == false ? by.displayName! : nil
        switch by.kind {
        case .owner: return "You"
        case .ownDevice: return name ?? "One of your devices"
        case .guest: return (name ?? "A guest") + " (guest)"
        case .agent: return (name ?? "An agent") + " (agent)"
        }
    }

    enum CodingKeys: String, CodingKey {
        case taskId = "task_id", rootId = "root_id", itemId = "item_id", name, size
        case droppedBy = "dropped_by", untrusted, state, output, ruleId = "rule_id", error
        case resultItemIds = "result_item_ids", createdAt = "created_at", updatedAt = "updated_at"
    }
}

private struct FilesInboxTasksResponse: Decodable {
    var tasks: [FilesInboxTask]
}

extension FilesClient {
    /// `GET /inbox/tasks?state=…`, newest first. Owner and own devices only.
    func inboxTasks(state: String? = "needs_approval", limit: Int = 100) async throws -> [FilesInboxTask] {
        var query = [URLQueryItem(name: "limit", value: String(min(max(limit, 1), 500)))]
        if let state { query.append(URLQueryItem(name: "state", value: state)) }
        let response: FilesInboxTasksResponse = try await json(FilesRequest(path: "/inbox/tasks", query: query))
        return response.tasks
    }

    /// `POST /inbox/tasks/:id/approve` with `{}`: the default tools
    /// (`files.read`, `summarize`, `files.write_beside`), result beside the file.
    func approveInboxTask(_ taskId: String) async throws -> FilesInboxTask {
        try await json(FilesRequest(method: "POST", path: "/inbox/tasks/\(Self.pathSegment(taskId))/approve",
                                    headers: ["Content-Type": "application/json"], body: .json(Data("{}".utf8))))
    }

    /// `POST /inbox/tasks/:id/reject`. Nothing reads the file.
    func rejectInboxTask(_ taskId: String) async throws -> FilesInboxTask {
        try await json(FilesRequest(method: "POST", path: "/inbox/tasks/\(Self.pathSegment(taskId))/reject",
                                    headers: ["Content-Type": "application/json"], body: .json(Data("{}".utf8))))
    }

    static func pathSegment(_ segment: String) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-._~")
        return segment.addingPercentEncoding(withAllowedCharacters: allowed) ?? segment
    }
}

/// Sends files to a computer's Inbox: the share sheet, the Save File to
/// Computer shortcut and the app's own upload all go through here.
struct FilesInboxDrop: Sendable {
    let client: FilesClient

    /// What happened to "Ask my agent to read it".
    enum AgentRequest: Sendable, Equatable {
        case notAsked
        /// The task was approved; the agent reads the file now.
        case approved
        /// No task showed up in time (or Inbox has no agent access): the file
        /// is in Inbox, and the owner can approve it in the app later.
        case pending
    }

    /// The computer's Inbox root.
    func inboxRoot() async throws -> Root {
        let roots = try await client.roots().roots
        guard let inbox = roots.first(where: { $0.kind == .inbox }) else {
            throw FilesError.invalidResponse("This computer has no Inbox folder.")
        }
        return inbox
    }

    /// Uploads `url` as `name` into Inbox (a taken name becomes `name (2)`).
    /// `wake` decides what happens when the computer is asleep: `.never`
    /// throws `wakeConsentRequired` so the caller can ask the user.
    func send(fileAt url: URL, name: String, to inbox: Root, wake: FilesWakePolicy,
              progress: (@Sendable (UInt64, UInt64) -> Void)? = nil) async throws -> FilesWriteResult {
        try await client.upload(fileAt: url, to: .create(rootId: inbox.id, parentId: inbox.rootItemId, name: name),
                                onConflict: .keepBoth, wake: wake, progress: progress)
    }

    /// Approves the Inbox task the drop of `itemId` created. The runtime
    /// creates it right after the write, so this looks a few times.
    func askAgent(itemId: ItemId, attempts: Int = 4) async -> AgentRequest {
        for attempt in 0..<attempts {
            if attempt > 0 { try? await Task.sleep(nanoseconds: 1_000_000_000) }
            guard let tasks = try? await client.inboxTasks(state: "needs_approval") else { continue }
            if let task = tasks.first(where: { $0.itemId == itemId }) {
                return (try? await client.approveInboxTask(task.taskId)) != nil ? .approved : .pending
            }
        }
        return .pending
    }
}
