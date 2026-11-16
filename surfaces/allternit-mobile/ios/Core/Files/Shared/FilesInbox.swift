import Foundation
import AllternitFilesKit

// Compiled into the app (Core/) and the share extension (project.yml
// AllternitShare sources), so both send to a computer's Inbox the same way.

extension FilesClient {
    // Inbox task calls (`inboxTasks`, `approveInboxTask`, `rejectInboxTask`)
    // and `FilesInboxTask` live in AllternitFilesKit (FilesClient+Actions).

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
