import Foundation

/// Streaming `.tar.bz2` extraction for voice model packs: bzip2 via the
/// SDK's libbz2, a minimal ustar reader on top. No temp copy of the
/// decompressed tar, and every entry path is confined to the destination.
enum Bz2TarError: Error, LocalizedError, Equatable {
    case bzip2(Int32)
    case badTar(String)
    case unsafePath(String)

    var errorDescription: String? {
        switch self {
        case .bzip2(let code): return "The voice model download is corrupt (bzip2 \(code))."
        case .badTar(let why): return "The voice model download is corrupt (\(why))."
        case .unsafePath(let path): return "Refusing to unpack an unsafe path: \(path)"
        }
    }
}

/// Incremental tar reader: `feed` decompressed bytes, files appear under
/// `destination`. Handles regular files, directories and GNU long names;
/// other entry types (links, devices, pax headers) are skipped.
final class TarStreamExtractor {
    private let destination: URL
    private var buffer = Data()
    private var current: FileHandle?
    private var remaining = 0
    private var padding = 0
    private var skipBytes = 0
    private var pendingLongName: String?
    private var collectingLongName = false
    private var longNameBytes = Data()
    private(set) var extractedFiles: [String] = []
    private var finished = false

    init(destination: URL) {
        self.destination = destination.standardizedFileURL
    }

    func feed(_ data: Data) throws {
        guard !finished else { return }
        buffer.append(data)
        try pump()
    }

    func finish() throws {
        try current?.close()
        current = nil
    }

    private func pump() throws {
        while true {
            if remaining > 0 || skipBytes > 0 {
                let take = min(buffer.count, remaining > 0 ? remaining : skipBytes)
                guard take > 0 else { return }
                let chunk = buffer.prefix(take)
                if remaining > 0 {
                    if collectingLongName { longNameBytes.append(chunk) } else { try current?.write(contentsOf: chunk) }
                    remaining -= take
                    if remaining == 0 {
                        if collectingLongName {
                            collectingLongName = false
                            pendingLongName = String(decoding: longNameBytes.prefix { $0 != 0 }, as: UTF8.self)
                            longNameBytes = Data()
                        } else {
                            try current?.close()
                            current = nil
                        }
                        skipBytes = padding
                        padding = 0
                    }
                } else {
                    skipBytes -= take
                }
                buffer.removeFirst(take)
                continue
            }
            guard buffer.count >= 512 else { return }
            let header = buffer.prefix(512)
            buffer.removeFirst(512)
            if header.allSatisfy({ $0 == 0 }) { finished = true; return }
            try handleHeader(Data(header))
        }
    }

    private func handleHeader(_ header: Data) throws {
        func field(_ start: Int, _ length: Int) -> String {
            String(decoding: header[start..<start + length].prefix { $0 != 0 }, as: UTF8.self)
        }
        let size = Int(field(124, 12).trimmingCharacters(in: .whitespaces), radix: 8) ?? -1
        guard size >= 0 else { throw Bz2TarError.badTar("bad entry size") }
        let typeflag = header[156]
        var name = field(0, 100)
        let prefix = field(345, 155)
        if !prefix.isEmpty { name = prefix + "/" + name }
        if let long = pendingLongName { name = long; pendingLongName = nil }
        let padded = (512 - size % 512) % 512

        switch typeflag {
        case UInt8(ascii: "L"):
            collectingLongName = true
            remaining = size
            padding = padded
            longNameBytes = Data()
        case UInt8(ascii: "0"), 0:
            let target = try safeURL(name)
            try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
            FileManager.default.createFile(atPath: target.path, contents: nil)
            current = try FileHandle(forWritingTo: target)
            extractedFiles.append(name)
            remaining = size
            padding = padded
            if size == 0 { try current?.close(); current = nil; skipBytes = 0 }
        case UInt8(ascii: "5"):
            try FileManager.default.createDirectory(at: try safeURL(name), withIntermediateDirectories: true)
        default:
            skipBytes = size + padded
        }
    }

    private func safeURL(_ name: String) throws -> URL {
        let parts = name.split(separator: "/", omittingEmptySubsequences: true)
        guard !name.hasPrefix("/"), !parts.contains(".."), !parts.isEmpty else { throw Bz2TarError.unsafePath(name) }
        let url = destination.appendingPathComponent(parts.joined(separator: "/")).standardizedFileURL
        guard url.path.hasPrefix(destination.path + "/") else { throw Bz2TarError.unsafePath(name) }
        return url
    }
}

enum Bz2Tar {
    /// Decompresses `archive` (`.tar.bz2`) into `destination`.
    static func extract(archive: URL, to destination: URL) throws {
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        let input = try FileHandle(forReadingFrom: archive)
        defer { try? input.close() }
        let extractor = TarStreamExtractor(destination: destination)

        var stream = bz_stream()
        var rc = BZ2_bzDecompressInit(&stream, 0, 0)
        guard rc == BZ_OK else { throw Bz2TarError.bzip2(rc) }
        defer { BZ2_bzDecompressEnd(&stream) }

        let outSize = 1 << 16
        var out = [CChar](repeating: 0, count: outSize)
        var finished = false
        while !finished {
            guard let chunk = try input.read(upToCount: 1 << 16), !chunk.isEmpty else { break }
            var inBytes = [CChar](repeating: 0, count: chunk.count)
            chunk.withUnsafeBytes { raw in
                for index in 0..<chunk.count { inBytes[index] = CChar(bitPattern: raw[index]) }
            }
            try inBytes.withUnsafeMutableBufferPointer { inPtr in
                stream.next_in = inPtr.baseAddress
                stream.avail_in = UInt32(inPtr.count)
                while stream.avail_in > 0 {
                    try out.withUnsafeMutableBufferPointer { outPtr in
                        stream.next_out = outPtr.baseAddress
                        stream.avail_out = UInt32(outSize)
                        rc = BZ2_bzDecompress(&stream)
                        guard rc == BZ_OK || rc == BZ_STREAM_END else { throw Bz2TarError.bzip2(rc) }
                        let produced = outSize - Int(stream.avail_out)
                        if produced > 0 {
                            let bytes = outPtr.baseAddress!.withMemoryRebound(to: UInt8.self, capacity: produced) {
                                Data(bytes: $0, count: produced)
                            }
                            try extractor.feed(bytes)
                        }
                    }
                    if rc == BZ_STREAM_END { finished = true; break }
                }
            }
        }
        try extractor.finish()
        guard finished else { throw Bz2TarError.bzip2(BZ_UNEXPECTED_EOF) }
    }
}
