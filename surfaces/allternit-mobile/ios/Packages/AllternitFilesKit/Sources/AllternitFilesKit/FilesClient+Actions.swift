import Foundation

/// The calls behind the File Provider sheets and actions (plan step 20):
/// wake settings and shares on cloud-api; summaries, Inbox tasks, copies and
/// thumbnails on the runtime. None of them wakes a computer by itself:
/// content calls take a `FilesWakePolicy`, and the sheet asks the user first.
extension FilesClient {
    // MARK: Wake settings (cloud-api)

    /// `GET /api/v1/computer-files/:id/wake-settings`.
    public func wakeSettings() async throws -> WakeSettings {
        var request = URLRequest(url: cloudURL("api/v1/computer-files/\(Self.escape(computerId))/wake-settings"))
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = configuration.requestTimeout
        let (data, response) = try await cloud(request)
        return try decode(data, status: response.statusCode)
    }

    /// `PUT /api/v1/computer-files/:id/wake-settings` ("Wake automatically
    /// from now on" in the wake sheet sets `auto`).
    @discardableResult
    public func setWakeSetting(_ setting: WakeSetting) async throws -> WakeSettings {
        var request = URLRequest(url: cloudURL("api/v1/computer-files/\(Self.escape(computerId))/wake-settings"))
        request.httpMethod = "PUT"
        request.timeoutInterval = configuration.requestTimeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = try JSONEncoder().encode(UpdateWakeSettingsRequest(wake: setting))
        let (data, response) = try await cloud(request)
        return try decode(data, status: response.statusCode)
    }

    // MARK: Shares (cloud-api, SPEC section 8)

    /// `GET /api/v1/computer-files/:id/shares`: this computer's shares, so
    /// shared items get the Shared badge.
    public func shares() async throws -> SharesResponse {
        var request = URLRequest(url: cloudURL("api/v1/computer-files/\(Self.escape(computerId))/shares"))
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.timeoutInterval = configuration.requestTimeout
        let (data, response) = try await cloud(request)
        return try decode(data, status: response.statusCode)
    }

    /// `POST /api/v1/computer-files/:id/shares` ("Share with…"). cloud-api
    /// signs the link; the runtime re-checks scope on every guest request.
    public func createShare(_ share: CreateShareRequest) async throws -> ShareResponse {
        var request = URLRequest(url: cloudURL("api/v1/computer-files/\(Self.escape(computerId))/shares"))
        request.httpMethod = "POST"
        request.timeoutInterval = configuration.requestTimeout
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = try JSONEncoder().encode(share)
        let (data, response) = try await cloud(request)
        return try decode(data, status: response.statusCode)
    }

    // MARK: Runtime actions

    /// `POST /intel/actions` with `summarize` ("Summarize"). The summary is
    /// extractive (sentences from the file, no model) and labelled
    /// AI-generated. Needs the computer awake.
    public func summarize(itemId: ItemId, wake: FilesWakePolicy = .never) async throws -> FilesSummary {
        try await json(post("/intel/actions", FilesActionRequest(itemId: itemId, action: "summarize")), wake: wake)
    }

    /// `POST /copy` inside this computer ("Send to agent" copies the file into
    /// Inbox, which makes a task that waits for approval).
    public func copy(_ copy: CopyRequest, wake: FilesWakePolicy = .never) async throws -> CopyResponse {
        try await json(post("/copy", copy), wake: wake)
    }

    /// `GET /inbox/tasks` (owner and own devices only).
    public func inboxTasks(state: String? = nil, limit: Int = 100) async throws -> [FilesInboxTask] {
        var query = [URLQueryItem(name: "limit", value: String(min(max(limit, 1), 500)))]
        if let state { query.append(URLQueryItem(name: "state", value: state)) }
        let list: FilesInboxTaskList = try await json(FilesRequest(path: "/inbox/tasks", query: query))
        return list.tasks
    }

    /// `POST /inbox/tasks/:id/approve` with the default tools (read,
    /// summarize, write beside the file).
    public func approveInboxTask(_ taskId: String) async throws -> FilesInboxTask {
        try await json(FilesRequest(method: "POST", path: "/inbox/tasks/\(Self.escape(taskId))/approve",
                                    headers: ["Content-Type": "application/json"], body: .json(Data("{}".utf8))))
    }

    /// `POST /inbox/tasks/:id/reject`. Nothing reads the file.
    public func rejectInboxTask(_ taskId: String) async throws -> FilesInboxTask {
        try await json(FilesRequest(method: "POST", path: "/inbox/tasks/\(Self.escape(taskId))/reject",
                                    headers: ["Content-Type": "application/json"], body: .json(Data("{}".utf8))))
    }

    /// `GET /items/:id/preview?kind=thumbnail`: the 256 px PNG the computer
    /// made while it was awake. nil when there is none yet, when the computer
    /// is asleep (a preview never wakes it) or when the answer is bigger than
    /// `maxBytes`.
    public func thumbnail(itemId: ItemId, maxBytes: Int = 1_048_576) async throws -> Data? {
        let request = FilesRequest(path: "/items/\(Self.escape(itemId))/preview",
                                   query: [URLQueryItem(name: "kind", value: "thumbnail")],
                                   headers: ["Accept": "image/png, image/*"])
        let (data, response) = try await send(request)
        switch response.statusCode {
        case 200:
            let type = response.value(forHTTPHeaderField: "Content-Type") ?? ""
            guard type.hasPrefix("image/"), !data.isEmpty, data.count <= maxBytes else { return nil }
            return data
        case 404, 409, 501, 503:
            return nil
        default:
            throw ErrorDecoding.error(status: response.statusCode, data: data)
        }
    }
}

struct FilesActionRequest: Encodable {
    let itemId: ItemId
    let action: String

    enum CodingKeys: String, CodingKey {
        case itemId = "item_id"
        case action
    }
}

/// `POST /intel/actions` answer.
public struct FilesSummary: Decodable, Sendable, Hashable {
    public var itemId: ItemId
    public var action: String
    public var summary: String
    /// `extractive` today.
    public var method: String
    public var aiGenerated: Bool

    enum CodingKeys: String, CodingKey {
        case itemId = "item_id"
        case action, summary, method
        case aiGenerated = "ai_generated"
    }
}

/// One Inbox task on a computer (`GET /api/v1/files/v2/inbox/tasks`,
/// `surfaces/docs/api/files-drive-inbox.mdx`). A file that lands in Inbox is
/// untrusted: its task waits in `needs_approval` until the owner approves or
/// rejects it, and nothing reads the file before that. Shared by the app, the
/// share extension and the File Provider UI.
public struct FilesInboxTask: Codable, Sendable, Hashable, Identifiable {
    public var taskId: String
    public var rootId: RootId
    public var itemId: ItemId
    public var name: String
    public var size: UInt64?
    public var droppedBy: PrincipalRef?
    public var untrusted: Bool?
    /// `needs_approval`, `approved_by_rule`, `approved`, `running`,
    /// `succeeded`, `failed` or `rejected`.
    public var state: String
    public var output: String?
    public var ruleId: String?
    public var error: String?
    public var resultItemIds: [ItemId]?
    public var createdAt: String
    public var updatedAt: String?

    public var id: String { taskId }
    public var needsApproval: Bool { state == "needs_approval" }

    /// "Dana (guest)", "One of your devices", "You".
    public var droppedByLabel: String {
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

struct FilesInboxTaskList: Decodable {
    let tasks: [FilesInboxTask]
}
