import Foundation

/// What a `run_code` tool result carries beyond text: the files the run
/// produced and its run summary. The live SSE `tool_result` frame and the
/// transcript's `metadata.parts[]` tool part both carry the same top-level
/// `files` / `run` keys, and both decode into this one model.
struct RunCodeResult: Equatable, Sendable {
    struct File: Decodable, Equatable, Identifiable, Sendable {
        let name: String
        let mediaType: String
        let uploadId: String
        let size: Int
        let url: String?
        var id: String { uploadId }

        private enum CodingKeys: String, CodingKey { case name, mediaType, uploadId, size, url }

        init(name: String, mediaType: String, uploadId: String, size: Int = 0, url: String? = nil) {
            self.name = name; self.mediaType = mediaType; self.uploadId = uploadId
            self.size = size; self.url = url
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? "file"
            mediaType = (try? c.decodeIfPresent(String.self, forKey: .mediaType)) ?? "application/octet-stream"
            uploadId = try c.decode(String.self, forKey: .uploadId)
            size = (try? c.decodeIfPresent(Int.self, forKey: .size)) ?? 0
            url = (try? c.decodeIfPresent(String.self, forKey: .url)) ?? nil
        }

        var isImage: Bool {
            ["image/png", "image/jpeg", "image/gif", "image/webp"].contains(mediaType.lowercased())
        }
        var isCSV: Bool {
            mediaType.lowercased().hasPrefix("text/csv") || name.lowercased().hasSuffix(".csv")
        }
    }

    struct Skipped: Decodable, Equatable, Sendable {
        let name: String
        let reason: String

        private enum CodingKeys: String, CodingKey { case name, reason }
        init(name: String, reason: String) { self.name = name; self.reason = reason }
        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
            reason = (try? c.decodeIfPresent(String.self, forKey: .reason)) ?? ""
        }
    }

    struct Run: Decodable, Equatable, Sendable {
        let stdout: String
        let stderr: String
        /// Nil when the run timed out.
        let exitCode: Int?
        let timedOut: Bool
        let skipped: [Skipped]

        private enum CodingKeys: String, CodingKey {
            case stdout, stderr, skipped
            case exitCode = "exit_code"
            case timedOut = "timed_out"
        }

        init(stdout: String = "", stderr: String = "", exitCode: Int? = 0, timedOut: Bool = false, skipped: [Skipped] = []) {
            self.stdout = stdout; self.stderr = stderr; self.exitCode = exitCode
            self.timedOut = timedOut; self.skipped = skipped
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            stdout = (try? c.decodeIfPresent(String.self, forKey: .stdout)) ?? ""
            stderr = (try? c.decodeIfPresent(String.self, forKey: .stderr)) ?? ""
            exitCode = (try? c.decodeIfPresent(Int.self, forKey: .exitCode)) ?? nil
            timedOut = (try? c.decodeIfPresent(Bool.self, forKey: .timedOut)) ?? false
            skipped = (try? c.decodeIfPresent([Skipped].self, forKey: .skipped)) ?? []
        }

        /// One line for the card: "Exit 0", "Timed out", "Exit 1 · 2 files skipped".
        var summary: String {
            var parts: [String] = []
            if timedOut { parts.append("Timed out") }
            else if let exitCode { parts.append("Exit \(exitCode)") }
            if !skipped.isEmpty { parts.append("\(skipped.count) file\(skipped.count == 1 ? "" : "s") skipped") }
            return parts.joined(separator: " · ")
        }
        var succeeded: Bool { !timedOut && exitCode == 0 }
    }

    var files: [File]
    var run: Run?

    /// Nil when the frame/part carries neither (an old runtime): callers then
    /// render nothing new.
    init?(files: [File]?, run: Run?) {
        guard files != nil || run != nil else { return nil }
        self.files = files ?? []
        self.run = run
    }
}

/// Decodes the `files` / `run` siblings of a frame or part. Never throws.
struct RunCodeKeys: Decodable {
    let result: RunCodeResult?
    private enum CodingKeys: String, CodingKey { case files, run }
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let files = (try? c.decodeIfPresent(LossyArray<RunCodeResult.File>.self, forKey: .files)) ?? nil
        let run = (try? c.decodeIfPresent(RunCodeResult.Run.self, forKey: .run)) ?? nil
        result = RunCodeResult(files: files?.elements, run: run)
    }
}

private struct LossySkip: Decodable {}

/// An array that drops elements that fail to decode instead of failing whole.
struct LossyArray<T: Decodable>: Decodable {
    let elements: [T]
    init(from decoder: Decoder) throws {
        var c = try decoder.unkeyedContainer()
        var out: [T] = []
        while !c.isAtEnd {
            if let v = try? c.decode(T.self) { out.append(v) } else { _ = try? c.decode(LossySkip.self) }
        }
        elements = out
    }
}
