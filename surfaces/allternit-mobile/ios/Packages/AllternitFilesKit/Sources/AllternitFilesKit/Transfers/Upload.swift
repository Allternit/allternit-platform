import Foundation

/// Where an upload lands.
public enum FilesUploadTarget: Sendable, Hashable {
    /// A new file named `name` under `parentId`. A taken name lands as
    /// `<stem> (2).<ext>` with `keepBoth` (not a conflict, SPEC section 4).
    case create(rootId: RootId, parentId: ItemId, name: String)
    /// New content for `itemId`, which the client last saw at `baseRev`.
    /// A stale `baseRev` with `keepBoth` lands as a conflict copy
    /// (`Lease (conflict iPhone 2026-11-15 1042).pdf`) and the original stays.
    case replace(rootId: RootId, parentId: ItemId, itemId: ItemId, name: String, baseRev: Rev)

    var rootId: RootId {
        switch self { case .create(let r, _, _), .replace(let r, _, _, _, _): return r }
    }
    var parentId: ItemId {
        switch self { case .create(_, let p, _), .replace(_, let p, _, _, _): return p }
    }
    var name: String {
        switch self { case .create(_, _, let n), .replace(_, _, _, let n, _): return n }
    }
    var itemId: ItemId? {
        if case .replace(_, _, let id, _, _) = self { return id }
        return nil
    }
    var baseRev: Rev? {
        if case .replace(_, _, _, _, let rev) = self { return rev }
        return nil
    }
}

/// What a write did. `conflict` is set when the write landed as a conflict
/// copy: `item` is then the copy, and the original is unchanged.
public struct FilesWriteResult: Sendable {
    public let item: Item
    public let conflict: ConflictInfo?
    public let cursor: Cursor
}

extension FilesClient {
    /// Files up to this size use the one-request small write.
    public static let smallWriteLimit: UInt64 = 4 * 1_048_576

    /// Uploads the file at `url`, conflict-safe (SPEC sections 4 and 5).
    ///
    /// - 4 MiB or less: `PUT /roots/:root_id/content` (small write).
    /// - Bigger: a resumable upload session. The file is cut with the pinned
    ///   chunker, `POST /uploads` names the chunks, only the chunks the
    ///   computer `need`s are sent (dedup), then `commit`. The session id is
    ///   journaled in the scratch directory, so sending the same file to the
    ///   same target again resumes it (`GET /uploads/:id`) instead of
    ///   starting over. An expired session starts a new one.
    /// - Every write sends `device_name` and `utc_offset_minutes`, so a
    ///   conflict copy is named in this device's local time.
    ///
    /// Reads the file from disk piece by piece; never holds it in memory.
    public func upload(fileAt url: URL,
                       to target: FilesUploadTarget,
                       onConflict: OnConflict = .keepBoth,
                       modifiedAt: Date? = nil,
                       wake: FilesWakePolicy = .whenAutomatic,
                       progress: (@Sendable (UInt64, UInt64) -> Void)? = nil) async throws -> FilesWriteResult {
        let size = (try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? UInt64) ?? 0
        if size <= Self.smallWriteLimit {
            return try await smallWrite(fileAt: url, size: size, target: target, onConflict: onConflict, wake: wake, progress: progress)
        }
        let manifest = try FileChunker().chunk(fileAt: url)
        return try await withWake(policy: wake, reason: .upload, itemId: target.itemId) {
            try await self.sessionUpload(fileAt: url, manifest: manifest, target: target, onConflict: onConflict,
                                         modifiedAt: modifiedAt, progress: progress)
        }
    }

    private func smallWrite(fileAt url: URL, size: UInt64, target: FilesUploadTarget, onConflict: OnConflict,
                            wake: FilesWakePolicy, progress: (@Sendable (UInt64, UInt64) -> Void)?) async throws -> FilesWriteResult {
        var query = [
            URLQueryItem(name: "parent_id", value: target.parentId),
            URLQueryItem(name: "name", value: target.name),
            URLQueryItem(name: "on_conflict", value: onConflict.rawValue),
            URLQueryItem(name: "device_name", value: configuration.deviceName),
            URLQueryItem(name: "utc_offset_minutes", value: String(Self.utcOffsetMinutes())),
        ]
        if let baseRev = target.baseRev { query.append(URLQueryItem(name: "base_rev", value: baseRev)) }
        let request = FilesRequest(method: "PUT", path: "/roots/\(Self.escape(target.rootId))/content", query: query,
                                   headers: ["Content-Type": "application/octet-stream"],
                                   body: .file(url, range: 0..<size))
        let response: MutationResponse = try await withWake(policy: wake, reason: .upload, itemId: target.itemId) {
            let (data, http) = try await self.send(request, http: self.configuration.transferHTTP)
            return try self.decode(data, status: http.statusCode)
        }
        progress?(size, size)
        return FilesWriteResult(item: response.item, conflict: response.conflict, cursor: response.cursor)
    }

    private func sessionUpload(fileAt url: URL, manifest: FileChunker.Result, target: FilesUploadTarget,
                               onConflict: OnConflict, modifiedAt: Date?,
                               progress: (@Sendable (UInt64, UInt64) -> Void)?) async throws -> FilesWriteResult {
        let journal = UploadJournal(directory: configuration.scratchDirectory)
        let key = UploadJournal.key(computerId: computerId, target: target, contentHash: manifest.contentHash)
        var status: UploadStatus
        if let sessionId = journal.session(for: key), let resumed = try? await uploadStatus(sessionId), resumed.state == .open {
            status = resumed
        } else {
            status = try await createSession(manifest: manifest, target: target, onConflict: onConflict, modifiedAt: modifiedAt)
            journal.set(status.sessionId, for: key)
        }

        let sizes = Dictionary(manifest.chunks.map { ($0.hash, $0) }, uniquingKeysWith: { first, _ in first })
        var attempts = 0
        while true {
            var sent = Set<Blake3>()
            for hash in status.need where !sent.contains(hash) {
                try Task.checkCancellation()
                guard let chunk = sizes[hash] else { throw FilesError.invalidResponse("computer needs unknown chunk \(hash)") }
                try await putChunk(sessionId: status.sessionId, chunk: chunk, fileAt: url)
                sent.insert(hash)
                progress?(min(status.receivedBytes + sent.reduce(0) { $0 + (sizes[$1]?.size ?? 0) }, manifest.size), manifest.size)
            }
            do {
                let commit: UploadCommitResponse = try await json(
                    FilesRequest(method: "POST", path: "/uploads/\(Self.escape(status.sessionId))/commit")
                )
                journal.set(nil, for: key)
                progress?(manifest.size, manifest.size)
                return FilesWriteResult(item: commit.item, conflict: commit.conflict, cursor: commit.cursor)
            } catch let error as FilesError where error.code == .chunkMissing && attempts < 2 {
                attempts += 1
                status = try await uploadStatus(status.sessionId)
                if let missing = error.body?.missingChunks, !missing.isEmpty { status.need = missing }
            } catch let error as FilesError where [.uploadSessionExpired, .uploadSessionNotFound].contains(error.code) && attempts < 2 {
                attempts += 1
                status = try await createSession(manifest: manifest, target: target, onConflict: onConflict, modifiedAt: modifiedAt)
                journal.set(status.sessionId, for: key)
            }
        }
    }

    /// `GET /uploads/:session_id` (what is still needed).
    public func uploadStatus(_ sessionId: String) async throws -> UploadStatus {
        try await json(FilesRequest(path: "/uploads/\(Self.escape(sessionId))"))
    }

    /// `DELETE /uploads/:session_id`.
    public func cancelUpload(_ sessionId: String) async throws {
        let (data, response) = try await send(FilesRequest(method: "DELETE", path: "/uploads/\(Self.escape(sessionId))"))
        guard (200..<300).contains(response.statusCode) else { throw ErrorDecoding.error(status: response.statusCode, data: data) }
    }

    private func createSession(manifest: FileChunker.Result, target: FilesUploadTarget, onConflict: OnConflict,
                               modifiedAt: Date?) async throws -> UploadStatus {
        let page = 10_000
        let first = Array(manifest.chunks.prefix(page))
        let create = UploadCreateRequest(
            rootId: target.rootId, parentId: target.parentId, name: target.name,
            itemId: target.itemId, baseRev: target.baseRev,
            size: manifest.size, contentHash: manifest.contentHash, chunker: FastCDC.params,
            chunks: first, totalChunks: UInt64(manifest.chunks.count), onConflict: onConflict,
            modifiedAt: modifiedAt.map(Self.formatDate), deviceName: configuration.deviceName,
            utcOffsetMinutes: Int64(Self.utcOffsetMinutes())
        )
        var status: UploadStatus = try await json(
            FilesRequest(method: "POST", path: "/uploads", headers: ["Content-Type": "application/json"],
                         body: .json(try JSONEncoder().encode(create)))
        )
        var index = page
        while index < manifest.chunks.count {
            let slice = Array(manifest.chunks[index..<min(index + page, manifest.chunks.count)])
            status = try await json(
                FilesRequest(method: "POST", path: "/uploads/\(Self.escape(status.sessionId))/chunks",
                             headers: ["Content-Type": "application/json"],
                             body: .json(try JSONEncoder().encode(UploadChunksAppend(chunks: slice))))
            )
            index += page
        }
        return status
    }

    private func putChunk(sessionId: String, chunk: ChunkRef, fileAt url: URL) async throws {
        let request = FilesRequest(method: "PUT",
                                   path: "/uploads/\(Self.escape(sessionId))/chunks/\(Self.escape(chunk.hash))",
                                   headers: ["Content-Type": "application/octet-stream"],
                                   body: .file(url, range: chunk.offset..<(chunk.offset + chunk.size)))
        let (data, response) = try await send(request, http: configuration.transferHTTP)
        let _: ChunkPutResponse = try decode(data, status: response.statusCode)
    }

    /// This device's UTC offset in minutes (for example -360 for CST).
    static func utcOffsetMinutes(_ zone: TimeZone = .current, at date: Date = Date()) -> Int {
        zone.secondsFromGMT(for: date) / 60
    }

    static func formatDate(_ date: Date) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.string(from: date)
    }
}

/// Upload session ids by target and content, so a retried upload resumes.
struct UploadJournal {
    let file: URL

    init(directory: URL) {
        file = directory.appendingPathComponent("uploads.json")
    }

    static func key(computerId: ComputerId, target: FilesUploadTarget, contentHash: Blake3) -> String {
        [computerId, target.rootId, target.parentId, target.name, target.itemId ?? "", target.baseRev ?? "", contentHash].joined(separator: "|")
    }

    func session(for key: String) -> String? { read()[key] }

    func set(_ sessionId: String?, for key: String) {
        var all = read()
        all[key] = sessionId
        if let data = try? JSONEncoder().encode(all) { try? data.write(to: file, options: .atomic) }
    }

    private func read() -> [String: String] {
        guard let data = try? Data(contentsOf: file) else { return [:] }
        return (try? JSONDecoder().decode([String: String].self, from: data)) ?? [:]
    }
}
