import Foundation

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
    public func download(itemId: ItemId,
                         rev: Rev,
                         size: UInt64,
                         contentHash: Blake3?,
                         to destination: URL,
                         wake: FilesWakePolicy = .whenAutomatic,
                         segmentBytes: UInt64 = 32 * 1_048_576,
                         progress: (@Sendable (UInt64, UInt64) -> Void)? = nil) async throws {
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
