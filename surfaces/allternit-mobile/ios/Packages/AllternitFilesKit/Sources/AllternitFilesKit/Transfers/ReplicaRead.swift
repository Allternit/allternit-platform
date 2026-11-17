import Foundation

/// What a replica read served.
public struct ReplicaReadResult: Sendable, Equatable {
    public var hostDeviceId: DeviceId
    public var replicaId: String
    /// When the replica was last brought up to date (`Head.synced_at`).
    public var syncedAt: String
}

extension FilesClient {
    /// SPEC section 10, step 4: the computer is asleep or offline, so read
    /// the file from a replica on another of the user's computers. Never
    /// wakes anything: the host is asked over cloud-api's relay, which does
    /// not wake for `/api/v1/files/v2/*`, and an asleep host just answers 503.
    ///
    /// 1. Candidate hosts: `GET /api/v1/computer-files/:id/placement?root_id=`
    ///    (online first), else the other computers this device knows.
    /// 2. The replica id is derived, not looked up (`replica_id(source, root, host)`).
    /// 3. `GET /replicas/:id/head` on the host, opened with the Files key
    ///    (`FilesKeyService.rootKey`), then the catalog parts, every one re-hashed.
    /// 4. The file's sealed chunks one at a time (`GET /replicas/:id/blobs/:locator`),
    ///    each opened, re-hashed and appended to disk, so memory stays near
    ///    one chunk (4 MiB ciphertext + 4 MiB plaintext) whatever the file size.
    /// 5. The whole file is checked against `contentHash` before it moves to
    ///    `destination`.
    ///
    /// A replica whose copy of the item has another `rev` (and another
    /// content hash) is behind and skipped. Throws `keyUnavailable` when this
    /// device has no Files key, `replicaUnavailable` when no host could answer.
    public func downloadFromReplica(itemId: ItemId,
                                    rootId: RootId,
                                    rev: Rev,
                                    size: UInt64,
                                    contentHash: Blake3?,
                                    to destination: URL,
                                    maxCatalogBytes: UInt64 = 8 * 1_048_576,
                                    progress: (@Sendable (UInt64, UInt64) -> Void)? = nil) async throws -> ReplicaReadResult {
        guard let store = configuration.sharedStore, !store.filesKeyEnvelopes().isEmpty else {
            throw FilesError.keyUnavailable("This device has no Files key yet.")
        }
        var lastError: Error = FilesError.replicaUnavailable("No replica of this folder is reachable right now.")
        for host in await replicaHosts(rootId: rootId) {
            try Task.checkCancellation()
            let replicaId = FilesKeyCrypto.replicaId(sourceComputerId: computerId, rootId: rootId, hostDeviceId: host)
            let hostClient = FilesClient(computerId: host, configuration: configuration)
            do {
                guard let headBytes = try await hostClient.replicaGet("/replicas/\(Self.escape(replicaId))/head") else { continue }
                let frk = try FilesKeyService.rootKey(store: store, keyId: try ReplicaReader.headKeyId(headBytes))
                let head = try ReplicaReader.openHead(headBytes, frk: frk, replicaId: replicaId)
                let catalog = try await ReplicaReader.readCatalog(head: head, frk: frk, maxBytes: maxCatalogBytes) { locator in
                    try await hostClient.replicaGet("/replicas/\(Self.escape(replicaId))/blobs/\(locator)")
                }
                guard let entry = catalog.entry(itemId), entry.kind == .file else { continue }
                let sameContent = entry.rev == rev || (contentHash != nil && entry.contentHash == contentHash)
                guard sameContent, entry.size == size else {
                    lastError = FilesError.replicaUnavailable("The replica on this computer is behind. It catches up the next time both computers are on.")
                    continue
                }
                try await hostClient.writeReplicaFile(entry, replicaId: replicaId, sealer: ChunkSealer(frk: frk, replicaId: replicaId),
                                                      to: destination, progress: progress)
                return ReplicaReadResult(hostDeviceId: host, replicaId: replicaId, syncedAt: head.syncedAt)
            } catch let error as FilesError {
                // Another host may have a good copy; a bad one never reaches disk.
                lastError = error
            } catch let error as URLError where error.code != .cancelled {
                lastError = error
            }
        }
        throw lastError
    }

    /// Hosts to try, best first. Placement ranks the user's other computers
    /// that sent a storage report; any of them may hold a replica.
    func replicaHosts(rootId: RootId) async -> [DeviceId] {
        struct Option: Decodable { var device_id: String; var computer_state: ComputerState? }
        struct Placement: Decodable { var options: [Option] }
        var hosts: [DeviceId] = []
        if let token = try? await configuration.tokenProvider(), !token.isEmpty {
            let url = cloudURL("api/v1/computer-files/\(Self.escape(computerId))/placement?root_id=\(Self.escape(rootId))")
            var request = URLRequest(url: url)
            request.timeoutInterval = configuration.requestTimeout
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
            request.setValue("application/json", forHTTPHeaderField: "Accept")
            request.setValue(configuration.deviceId, forHTTPHeaderField: "X-Allternit-Device-Id")
            if let (data, response) = try? await configuration.http.data(for: request), response.statusCode == 200,
               let placement = try? JSONDecoder().decode(Placement.self, from: data) {
                let rank: (ComputerState?) -> Int = { $0 == .awake ? 0 : $0 == .offline ? 2 : 1 }
                hosts = placement.options.enumerated()
                    .sorted { (rank($0.element.computer_state), $0.offset) < (rank($1.element.computer_state), $1.offset) }
                    .map(\.element.device_id)
            }
        }
        if hosts.isEmpty, let known = configuration.sharedStore?.computers.map(\.id) { hosts = known }
        var seen = Set<String>()
        return hosts.filter { $0 != computerId && seen.insert($0).inserted }
    }

    /// One replica host GET over this client's transport: the body for 200,
    /// nil for 404 (`not_found`: no such replica or blob on this host).
    func replicaGet(_ path: String) async throws -> Data? {
        let (data, response) = try await send(FilesRequest(path: path, headers: ["Accept": "application/octet-stream"]))
        if response.statusCode == 200 { return data }
        if response.statusCode == 404 { return nil }
        throw ErrorDecoding.error(status: response.statusCode, data: data)
    }

    /// Streams one file's sealed chunks to `destination`, checking each chunk
    /// and the whole file. Nothing is left behind on failure.
    func writeReplicaFile(_ entry: ReplicaCatalogEntry, replicaId: String, sealer: ChunkSealer, to destination: URL,
                          progress: (@Sendable (UInt64, UInt64) -> Void)?) async throws {
        let fm = FileManager.default
        try fm.createDirectory(at: configuration.scratchDirectory, withIntermediateDirectories: true)
        let partial = configuration.scratchDirectory.appendingPathComponent("replica-\(UUID().uuidString).partial")
        guard fm.createFile(atPath: partial.path, contents: nil, attributes: FilesProtection.inProgressAttributes) else {
            throw FilesError.invalidResponse("could not create \(partial.lastPathComponent)")
        }
        var finished = false
        defer { if !finished { try? fm.removeItem(at: partial) } }
        let writer = try FileHandle(forWritingTo: partial)
        var whole = Blake3Hasher()
        var done: UInt64 = 0
        do {
            for chunk in entry.chunks {
                try Task.checkCancellation()
                let path = "/replicas/\(Self.escape(replicaId))/blobs/\(try sealer.locator(plaintextHash: chunk.hash))"
                let prepared = try await prepare(FilesRequest(path: path, headers: ["Accept": "application/octet-stream"]))
                let (file, response) = try await configuration.transferHTTP.download(for: prepared.request)
                defer { try? fm.removeItem(at: file) }
                guard response.statusCode == 200 else {
                    let body = (try? Data(contentsOf: file)) ?? Data()
                    if response.statusCode == 404 { throw FilesError.replicaUnavailable("A part of this file is missing on the replica.") }
                    throw ErrorDecoding.error(status: response.statusCode, data: body)
                }
                // One chunk in memory at a time; the pool frees it before the next.
                let added: UInt64 = try autoreleasepool {
                    let plain = try sealer.open(blob: try Data(contentsOf: file), plaintextHash: chunk.hash)
                    whole.update(plain)
                    try writer.write(contentsOf: plain)
                    return UInt64(plain.count)
                }
                done += added
                progress?(done, entry.size)
            }
            try writer.close()
        } catch {
            try? writer.close()
            throw error
        }
        let actual = whole.finalizeHex()
        let want = entry.contentHash ?? Blake3Hasher.hashHex(Data())
        guard done == entry.size, actual == want else { throw FilesError.integrity(expected: want, actual: actual) }
        if fm.fileExists(atPath: destination.path) { try fm.removeItem(at: destination) }
        try fm.moveItem(at: partial, to: destination)
        finished = true
        FilesProtection.protectFinished(destination)
    }
}
