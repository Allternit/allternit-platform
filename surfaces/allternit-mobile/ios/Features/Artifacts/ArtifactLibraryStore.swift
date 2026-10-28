import Foundation

/// One library entry: a v2 artifact summary plus the chat-card record the
/// in-chat sheets use. `sessionId` is the artifact's `origin.session_id`
/// (owner-only on the server), which bot pages filter on.
struct SavedArtifact: Identifiable, Hashable, Sendable {
    let record: ArtifactRecord
    let sessionId: String?
    let savedAt: Date
    let summary: ArtifactV2Summary?

    var id: String { record.id }

    init(summary: ArtifactV2Summary) {
        self.summary = summary
        self.sessionId = summary.origin?.sessionId
        self.savedAt = summary.updatedDate ?? Date()
        self.record = ArtifactRecord(
            id: summary.id,
            title: summary.title,
            fileType: summary.kind,
            artifactId: summary.id,
            artifactType: summary.kind,
            version: summary.currentVersion
        )
    }
}

/// The Artifacts Library's data source: Artifacts v2 (`/api/v2/artifacts`),
/// the one account-level store Desktop, ai.allternit.com and the PWA share.
///
/// Artifact frames seen in chat streams are saved to v2 as they arrive
/// (`record`), under a deterministic id — the same `art_t<hash>` the web's
/// `selectedToDraft` uses — so a frame replayed on reload, or seen on two
/// devices, never makes a second record. Artifacts the agent makes with the
/// `artifact_create` tool are already in v2 and show up on refresh.
/// Nothing is kept on disk: the server is the library.
@MainActor
final class ArtifactLibraryStore: ObservableObject {
    static let shared = ArtifactLibraryStore()

    enum Phase: Equatable {
        case idle
        case loading
        case loaded
        case signedOut
        case failed(String)
    }

    enum Scope: String, CaseIterable, Identifiable {
        case all, mine, shared
        var id: String { rawValue }
        var label: String {
            switch self {
            case .all: return "Everything"
            case .mine: return "Mine"
            case .shared: return "Shared with me"
            }
        }
    }

    @Published private(set) var artifacts: [SavedArtifact] = []
    @Published private(set) var phase: Phase = .idle
    @Published private(set) var isRefreshing = false
    @Published private(set) var isLoadingMore = false
    @Published private(set) var nextCursor: String?
    @Published private(set) var orgSettings: ArtifactV2OrgSettings?

    @Published var scope: Scope = .all
    @Published var kind: ArtifactV2Kind? = nil
    @Published var searchText: String = ""

    private let client: ArtifactsV2Client
    /// Draft ids already saved (or being saved) this launch.
    private var savedDraftIds: Set<String> = []
    private var loadGeneration = 0

    init(client: ArtifactsV2Client = .shared) {
        self.client = client
    }

    private var query: ArtifactV2ListQuery {
        ArtifactV2ListQuery(scope: scope.rawValue, kind: kind?.rawValue, query: searchText)
    }

    // MARK: - Loading

    /// Reloads the first page for the current filters. Kept under its old
    /// name — bot pages call it.
    func refreshFromBackend() async {
        await reload()
    }

    func reload() async {
        loadGeneration += 1
        let generation = loadGeneration
        isRefreshing = true
        if artifacts.isEmpty { phase = .loading }
        defer { if generation == loadGeneration { isRefreshing = false } }
        do {
            let page = try await client.list(query)
            guard generation == loadGeneration else { return }
            artifacts = page.items.map(SavedArtifact.init(summary:))
            nextCursor = page.nextCursor
            phase = .loaded
        } catch is CancellationError {
            return
        } catch {
            guard generation == loadGeneration else { return }
            if (error as? APIError)?.httpStatus == 401 {
                artifacts = []
                phase = .signedOut
            } else if artifacts.isEmpty {
                phase = .failed(error.localizedDescription)
            } else {
                // Keep what's on screen; a pull to refresh retries.
                phase = .loaded
            }
        }
        if orgSettings == nil, phase == .loaded {
            orgSettings = try? await client.orgSettings()
        }
    }

    func loadMoreIfNeeded(current: SavedArtifact) async {
        guard let cursor = nextCursor, !isLoadingMore, current.id == artifacts.last?.id else { return }
        isLoadingMore = true
        defer { isLoadingMore = false }
        let generation = loadGeneration
        var next = query
        next.cursor = cursor
        guard let page = try? await client.list(next), generation == loadGeneration else { return }
        let known = Set(artifacts.map(\.id))
        artifacts.append(contentsOf: page.items.filter { !known.contains($0.id) }.map(SavedArtifact.init(summary:)))
        nextCursor = page.nextCursor
    }

    // MARK: - Library actions

    /// Moves an artifact out of Saved work (per-viewer archive; reversible
    /// on the web's Archive view). Kept under its old name.
    func remove(id: String) {
        artifacts.removeAll { $0.id == id }
        Task {
            do {
                try await client.setArchived(id: id, archived: true)
            } catch {
                await reload()
            }
        }
    }

    /// Drops a row already archived elsewhere (the detail view).
    func dropLocally(id: String) {
        artifacts.removeAll { $0.id == id }
    }

    /// Owner delete — removes the artifact for everyone it's shared with.
    func delete(id: String) async throws {
        try await client.delete(id: id)
        artifacts.removeAll { $0.id == id }
    }

    // MARK: - Chat-stream frames → v2

    /// Saves an artifact frame seen in a chat stream to v2. Idempotent: the
    /// draft id is deterministic and the server returns the existing record
    /// for an id the caller already owns.
    func record(_ record: ArtifactRecord, sessionId: String?) {
        guard let draft = Self.draft(from: record, sessionId: sessionId) else { return }
        guard !savedDraftIds.contains(draft.id ?? "") else { return }
        savedDraftIds.insert(draft.id ?? "")
        Task {
            do {
                let saved = try await client.create(draft)
                insertIfVisible(saved)
            } catch {
                // Signed out or offline: allow a later re-see to retry.
                savedDraftIds.remove(draft.id ?? "")
            }
        }
    }

    private func insertIfVisible(_ artifact: ArtifactV2) {
        guard scope != .shared, kind == nil || kind?.rawValue == artifact.kind,
              searchText.trimmingCharacters(in: .whitespaces).isEmpty,
              !artifacts.contains(where: { $0.id == artifact.id }) else { return }
        let summary = ArtifactV2Summary(
            id: artifact.id, kind: artifact.kind, title: artifact.title, icon: artifact.icon,
            owner: artifact.owner, updatedAt: artifact.updatedAt, currentVersion: artifact.currentVersion,
            visibility: artifact.visibility, myAccess: artifact.myAccess, origin: artifact.origin,
            runtimeVersion: artifact.runtimeVersion, thumbnailURL: artifact.thumbnailURL
        )
        artifacts.insert(SavedArtifact(summary: summary), at: 0)
    }

    /// Frame → v2 draft, the native port of the web's `selectedToDraft`
    /// (allternit-ai src/lib/artifacts/from-transcript.ts). Kinds the web
    /// doesn't save from a frame (slides, audio, podcast, video) return nil.
    nonisolated static func draft(from record: ArtifactRecord, sessionId: String?) -> ArtifactV2CreateInput? {
        let frameKind = (record.artifactType ?? record.fileType).lowercased()
        let content = record.inlinePreview ?? ""
        var body = content
        var meta: [String: JSONValue]? = nil
        let kind: String
        let format: String
        switch frameKind {
        case "html":
            kind = "page"; format = "text/html"
        case "document":
            kind = "doc"; format = "text/markdown"
        case "mermaid":
            kind = "diagram"; format = "text/vnd.mermaid"
        case "svg":
            kind = "diagram"; format = "image/svg+xml"
        case "sheet":
            kind = "sheet"; format = "application/vnd.allternit.sheet+json"
            let trimmed = content.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.hasPrefix("[") {
                let rows = (try? JSONSerialization.jsonObject(with: Data(trimmed.utf8))) ?? []
                let wrapped = (try? JSONSerialization.data(withJSONObject: ["rows": rows])) ?? Data("{\"rows\":[]}".utf8)
                body = String(decoding: wrapped, as: UTF8.self)
            }
        case "jsx":
            kind = "code"; format = "text/plain"; meta = ["language": .string("tsx")]
        case "code":
            kind = "code"; format = "text/plain"; meta = ["language": .string("text")]
        case "image":
            let url = record.url ?? (content.hasPrefix("data:") ? content : "")
            guard !url.isEmpty else { return nil }
            kind = "image"; format = "text/uri-list"; body = url
        default:
            return nil
        }
        guard !body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        let title = record.title.isEmpty ? (ArtifactV2Kind(rawValue: kind)?.noun ?? "Artifact") : record.title
        return ArtifactV2CreateInput(
            id: draftId([sessionId, kind, record.title, body]),
            kind: kind,
            title: title,
            body: body,
            bodyFormat: format,
            meta: meta,
            origin: ArtifactV2Origin(surface: "chat", sessionId: sessionId)
        )
    }

    /// `art_t` + the web's stableHash over the parts joined by U+0001.
    nonisolated static func draftId(_ parts: [String?]) -> String {
        "art_t" + stableHash(parts.map { $0 ?? "" }.joined(separator: "\u{0001}"))
    }

    /// Port of allternit-ai src/lib/artifacts/stable-hash.ts (two FNV-1a
    /// style passes over UTF-16 code units, base36), so both surfaces mint
    /// the same id for the same frame.
    nonisolated static func stableHash(_ input: String) -> String {
        let units = Array(input.utf16)
        var h1: UInt32 = 0x811c9dc5
        var h2: UInt32 = 0x01000193 ^ UInt32(truncatingIfNeeded: units.count)
        for unit in units {
            let c = UInt32(unit)
            h1 = (h1 ^ c) &* 0x01000193
            h2 = (h2 ^ c) &* 0x5bd1e995
            h2 = h2 ^ (h2 >> 13)
        }
        func pad(_ value: UInt32) -> String {
            let s = String(value, radix: 36)
            return String(repeating: "0", count: max(0, 7 - s.count)) + s
        }
        return pad(h1) + pad(h2)
    }
}
