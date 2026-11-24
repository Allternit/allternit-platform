import Foundation

// `FilesClient.download` returns the contract's `ServedBy` (FilesDriveContract.gen.swift):
// `.live` from the computer itself (awake, or woken), `.replica` from a sealed replica on
// another of the user's computers (SPEC section 10, step 4), opened with this device's Files key.

extension FilesClient {
    /// Downloads an item's content to `destination`, streaming to disk in
    /// ranged segments (`GET /items/:id/content`, `Range`, 206). Never holds
    /// the file in memory.
    ///
    /// - Resumes: bytes already in `<scratch>/<item>.<rev>.partial` are kept,
    ///   and the next segment starts after them.
    /// - Every segment's `ETag` must be `"<rev>"`. If the file changed on the
    ///   computer meanwhile, this throws `revisionChanged`: stat again and
    ///   start over.
    /// - When `contentHash` is known the finished file is checked against it
    ///   (`integrity` on mismatch).
    /// - The finished file is protected `complete`.
    ///
    /// Needs the computer awake: throws `wakeConsentRequired` unless a ticket
    /// is held, or `wake` is `.whenAutomatic` and the setting is `auto`.
    /// `progress` gets bytes done and total.
    ///
    /// With `rootId` and a Files key on this device, a computer that is
    /// asleep or offline is read from a replica first (SPEC section 10,
    /// step 4), so opening a file needs no wake when a current replica
    /// exists; only when no replica answers does the wake path above run.
    /// A live read that fails because the computer is unavailable also tries
    /// the replica before giving up. Returns where the bytes came from.
    @discardableResult
    public func download(itemId: ItemId,
                         rev: Rev,
                         size: UInt64,
                         contentHash: Blake3?,
                         rootId: RootId? = nil,
                         to destination: URL,
                         wake: FilesWakePolicy = .whenAutomatic,
                         segmentBytes: UInt64 = 32 * 1_048_576,
                         progress: (@Sendable (UInt64, UInt64) -> Void)? = nil) async throws -> ServedBy {
        let canUseReplica = rootId != nil && !(configuration.sharedStore?.filesKeyEnvelopes().isEmpty ?? true)
        func fromReplica() async throws {
            guard let rootId else { return }
            _ = try await downloadFromReplica(itemId: itemId, rootId: rootId, rev: rev, size: size, contentHash: contentHash,
                                              to: destination, progress: progress)
        }
        var triedReplica = false
        if canUseReplica, currentWakeTicket() == nil,
           let state = try? await endpoints().computerState, state == .asleep || state == .offline {
            triedReplica = true
            if (try? await fromReplica()) != nil { return .replica }
        }
        do {
            try await downloadLive(itemId: itemId, rev: rev, size: size, contentHash: contentHash, to: destination,
                                   wake: wake, segmentBytes: segmentBytes, progress: progress)
            return .live
        } catch {
            guard canUseReplica, !triedReplica, Self.replicaMayHelp(error) else { throw error }
            // The original error stays the answer when no replica has the file.
            let original = error
            do { try await fromReplica() } catch { throw original }
            return .replica
        }
    }

    /// Failures where the computer itself is the problem, not the file.
    static func replicaMayHelp(_ error: Error) -> Bool {
        if let error = error as? URLError { return error.code != .cancelled }
        guard let error = error as? FilesError else { return false }
        if error.isComputerUnavailable { return true }
        if case .noTransport = error { return true }
        return error.code == .wakeConsentRequired || error.code == .wakeDenied
    }

    private func downloadLive(itemId: ItemId, rev: Rev, size: UInt64, contentHash: Blake3?, to destination: URL,
                              wake: FilesWakePolicy, segmentBytes: UInt64,
                              progress: (@Sendable (UInt64, UInt64) -> Void)?) async throws {
        let fm = FileManager.default
        let scratch = configuration.scratchDirectory
        try fm.createDirectory(at: scratch, withIntermediateDirectories: true)
        let partial = scratch.appendingPathComponent("\(Self.fileSafe(itemId)).\(Self.fileSafe(rev)).partial")
        if !fm.fileExists(atPath: partial.path) {
            fm.createFile(atPath: partial.path, contents: nil, attributes: FilesProtection.inProgressAttributes)
        }
        var done = (try? fm.attributesOfItem(atPath: partial.path)[.size] as? UInt64) ?? 0
        if done > size { try Data().write(to: partial); done = 0 }
        progress?(done, size)

        while done < size {
            try Task.checkCancellation()
            let end = min(done + segmentBytes, size) - 1
            let request = FilesRequest(path: "/items/\(Self.escape(itemId))/content",
                                       headers: ["Range": "bytes=\(done)-\(end)", "Accept": "*/*"])
            let (file, response) = try await withWake(policy: wake, reason: .openFile, itemId: itemId) { () -> (URL, HTTPURLResponse) in
                let prepared = try await self.prepare(request)
                let (file, response) = try await self.configuration.transferHTTP.download(for: prepared.request)
                guard (200..<300).contains(response.statusCode) else {
                    let body = (try? Data(contentsOf: file)) ?? Data()
                    try? fm.removeItem(at: file)
                    throw ErrorDecoding.error(status: response.statusCode, data: body)
                }
                return (file, response)
            }
            defer { try? fm.removeItem(at: file) }
            if let served = Self.etagRev(response), served != rev {
                try? fm.removeItem(at: partial)
                throw FilesError.revisionChanged(expected: rev, served: served)
            }
            if response.statusCode == 200 {
                // The runtime ignored Range and sent the whole file.
                try? fm.removeItem(at: partial)
                try fm.moveItem(at: file, to: partial)
                done = (try? fm.attributesOfItem(atPath: partial.path)[.size] as? UInt64) ?? 0
                progress?(done, size)
                break
            }
            done += try Self.append(file, to: partial)
            progress?(done, size)
        }

        if let contentHash {
            let actual = try FileChunker.hashFile(at: partial)
            guard actual == contentHash else {
                try? fm.removeItem(at: partial)
                throw FilesError.integrity(expected: contentHash, actual: actual)
            }
        }
        if fm.fileExists(atPath: destination.path) { try fm.removeItem(at: destination) }
        try fm.moveItem(at: partial, to: destination)
        FilesProtection.protectFinished(destination)
    }

    /// `"<rev>"` from the ETag header, nil when absent.
    static func etagRev(_ response: HTTPURLResponse) -> Rev? {
        guard var tag = response.value(forHTTPHeaderField: "ETag") else { return nil }
        if tag.hasPrefix("W/") { tag.removeFirst(2) }
        return tag.trimmingCharacters(in: CharacterSet(charactersIn: "\""))
    }

    /// Appends `file` to `target` in 1 MiB pieces; returns the bytes added.
    static func append(_ file: URL, to target: URL) throws -> UInt64 {
        let reader = try FileHandle(forReadingFrom: file)
        defer { try? reader.close() }
        let writer = try FileHandle(forWritingTo: target)
        defer { try? writer.close() }
        try writer.seekToEnd()
        var added: UInt64 = 0
        while let data = try reader.read(upToCount: 1_048_576), !data.isEmpty {
            try writer.write(contentsOf: data)
            added += UInt64(data.count)
        }
        return added
    }

    static func fileSafe(_ value: String) -> String {
        String(value.map { $0.isLetter || $0.isNumber || $0 == "-" || $0 == "_" ? $0 : "_" })
    }
}
