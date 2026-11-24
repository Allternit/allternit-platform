import Foundation

/// One `files/2` runtime request, before a transport wraps it. `path` is
/// relative to `/api/v1/files/v2` (for example `/roots`).
public struct FilesRequest: Sendable {
    public enum Body: Sendable {
        case none
        /// Small in-memory body (JSON). Sent as `utf8` over the relay.
        case json(Data)
        /// Bytes `range` of a file (chunk PUT, small write). Sent as
        /// `base64` over the relay, streamed through a temp file.
        case file(URL, range: Range<UInt64>)
    }

    public var method: String
    public var path: String
    public var query: [URLQueryItem]
    public var headers: [String: String]
    public var body: Body

    public init(method: String = "GET", path: String, query: [URLQueryItem] = [], headers: [String: String] = [:], body: Body = .none) {
        self.method = method
        self.path = path
        self.query = query
        self.headers = headers
        self.body = body
    }

    /// `path?query`, with every query value percent-encoded (`+` and `&` too).
    var pathAndQuery: String {
        guard !query.isEmpty else { return path }
        var allowed = CharacterSet.urlQueryAllowed
        allowed.remove(charactersIn: "+&=?/")
        let pairs = query.map { item -> String in
            let name = item.name.addingPercentEncoding(withAllowedCharacters: allowed) ?? item.name
            guard let value = item.value else { return name }
            return name + "=" + (value.addingPercentEncoding(withAllowedCharacters: allowed) ?? value)
        }
        return path + "?" + pairs.joined(separator: "&")
    }
}

/// How requests reach the computer (SPEC section 10). The File Provider
/// extension only ever uses `.relay` (spike 5: no tsnet in the extension).
public enum FilesTransport: Sendable, Equatable {
    /// cloud-api `POST /api/v1/runtime-devices/:id/proxy` (`base_url` of the
    /// relay candidate).
    case relay(proxyURL: URL)
    /// loopback or mesh: `base_url` ends in `/api/v1/files/v2`.
    case direct(baseURL: URL)

    public static let runtimePrefix = "/api/v1/files/v2"

    /// Request headers cloud-api's relay passes to the runtime (its allow-list
    /// plus the `files/2` additions, SPEC section 2 "Relayed request headers").
    /// Anything else is dropped here so nothing leaks into the envelope.
    static let relayedHeaders: Set<String> = [
        "accept", "content-type", "if-none-match", "if-modified-since", "x-request-id",
        "x-allternit-device-id", "x-allternit-device-name", "x-allternit-device-platform",
        "x-allternit-wake-ticket", "range", "if-match", "x-allternit-unlock",
    ]

    /// What the transport produced: send `request`, with `bodyFile` as the
    /// streamed body when it is set (delete the file afterwards).
    struct Prepared {
        var request: URLRequest
        var bodyFile: URL?
    }

    /// Builds the outgoing request. `bearer` authorizes cloud-api (relay) or
    /// the runtime (direct). Large bodies go to a file in `scratch`.
    func prepare(_ files: FilesRequest, bearer: String, scratch: URL, timeout: TimeInterval) throws -> Prepared {
        switch self {
        case .relay(let proxyURL):
            var request = URLRequest(url: proxyURL)
            request.httpMethod = "POST"
            request.timeoutInterval = timeout
            request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            var headers: [String: String] = [:]
            for (name, value) in files.headers where Self.relayedHeaders.contains(name.lowercased()) {
                headers[name.lowercased()] = value
            }
            let path = Self.runtimePrefix + files.pathAndQuery
            switch files.body {
            case .none:
                request.httpBody = try RelayEnvelope.inMemory(method: files.method, path: path, headers: headers, body: "", encoding: "utf8")
                return Prepared(request: request, bodyFile: nil)
            case .json(let data):
                let text = String(decoding: data, as: UTF8.self)
                request.httpBody = try RelayEnvelope.inMemory(method: files.method, path: path, headers: headers, body: text, encoding: "utf8")
                return Prepared(request: request, bodyFile: nil)
            case .file(let url, let range):
                let envelope = try RelayEnvelope.writeBase64(method: files.method, path: path, headers: headers,
                                                             source: url, range: range, scratch: scratch)
                return Prepared(request: request, bodyFile: envelope)
            }
        case .direct(let baseURL):
            let base = baseURL.absoluteString.hasSuffix("/") ? String(baseURL.absoluteString.dropLast()) : baseURL.absoluteString
            guard let url = URL(string: base + files.pathAndQuery) else {
                throw FilesError.invalidResponse("bad URL for \(files.path)")
            }
            var request = URLRequest(url: url)
            request.httpMethod = files.method
            request.timeoutInterval = timeout
            request.setValue("Bearer \(bearer)", forHTTPHeaderField: "Authorization")
            for (name, value) in files.headers { request.setValue(value, forHTTPHeaderField: name) }
            switch files.body {
            case .none:
                return Prepared(request: request, bodyFile: nil)
            case .json(let data):
                request.httpBody = data
                return Prepared(request: request, bodyFile: nil)
            case .file(let url, let range):
                let slice = try RelayEnvelope.copyRange(source: url, range: range, scratch: scratch)
                return Prepared(request: request, bodyFile: slice)
            }
        }
    }
}

/// cloud-api's relay body, `{method, path, headers, body, body_encoding}`
/// (runtime_relay.rs `BrowserProxyRequest`; it reads `bodyEncoding` and, per
/// SPEC section 2, also `body_encoding`, which Files clients send).
enum RelayEnvelope {
    private struct Small: Encodable {
        let method: String
        let path: String
        let headers: [String: String]
        let body: String
        let body_encoding: String
    }

    static func inMemory(method: String, path: String, headers: [String: String], body: String, encoding: String) throws -> Data {
        try JSONEncoder().encode(Small(method: method, path: path, headers: headers, body: body, body_encoding: encoding))
    }

    /// Writes the envelope with bytes `range` of `source` as base64, streamed
    /// in 768 KiB reads (a multiple of 3, so only the last piece pads). Peak
    /// memory ~2 MiB whatever the chunk size. Caller deletes the file.
    static func writeBase64(method: String, path: String, headers: [String: String],
                            source: URL, range: Range<UInt64>, scratch: URL) throws -> URL {
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        let out = scratch.appendingPathComponent("env-\(UUID().uuidString).json")
        guard FileManager.default.createFile(atPath: out.path, contents: nil, attributes: FilesProtection.inProgressAttributes) else {
            throw FilesError.invalidResponse("could not create \(out.lastPathComponent)")
        }
        let writer = try FileHandle(forWritingTo: out)
        defer { try? writer.close() }
        let encoder = JSONEncoder()
        func json<T: Encodable>(_ value: T) throws -> String { String(decoding: try encoder.encode(value), as: UTF8.self) }
        let head = "{\"method\":\(try json(method)),\"path\":\(try json(path)),\"headers\":\(try json(headers)),\"body_encoding\":\"base64\",\"body\":\""
        try writer.write(contentsOf: Data(head.utf8))
        let reader = try FileHandle(forReadingFrom: source)
        defer { try? reader.close() }
        try reader.seek(toOffset: range.lowerBound)
        var remaining = range.upperBound - range.lowerBound
        let piece: UInt64 = 786_432
        while remaining > 0 {
            try Task.checkCancellation()
            let want = Int(min(piece, remaining))
            guard let data = try reader.read(upToCount: want), !data.isEmpty else {
                throw FilesError.invalidResponse("file shorter than expected")
            }
            try writer.write(contentsOf: data.base64EncodedData())
            remaining -= UInt64(data.count)
        }
        try writer.write(contentsOf: Data("\"}".utf8))
        return out
    }

    /// Copies bytes `range` of `source` to a scratch file (direct transport).
    static func copyRange(source: URL, range: Range<UInt64>, scratch: URL) throws -> URL {
        try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
        let out = scratch.appendingPathComponent("body-\(UUID().uuidString)")
        guard FileManager.default.createFile(atPath: out.path, contents: nil, attributes: FilesProtection.inProgressAttributes) else {
            throw FilesError.invalidResponse("could not create \(out.lastPathComponent)")
        }
        let writer = try FileHandle(forWritingTo: out)
        defer { try? writer.close() }
        let reader = try FileHandle(forReadingFrom: source)
        defer { try? reader.close() }
        try reader.seek(toOffset: range.lowerBound)
        var remaining = range.upperBound - range.lowerBound
        while remaining > 0 {
            let want = Int(min(1_048_576, remaining))
            guard let data = try reader.read(upToCount: want), !data.isEmpty else {
                throw FilesError.invalidResponse("file shorter than expected")
            }
            try writer.write(contentsOf: data)
            remaining -= UInt64(data.count)
        }
        return out
    }
}

/// Data protection for what the kit writes (SPEC "cache protection"):
/// in-progress files are `completeUnlessOpen`, so a transfer already running
/// survives the phone locking; finished files are `complete`, unreadable
/// while the phone is locked. No-op on macOS.
public enum FilesProtection {
    static var inProgressAttributes: [FileAttributeKey: Any]? {
        #if os(iOS)
        return [.protectionKey: FileProtectionType.completeUnlessOpen]
        #else
        return nil
        #endif
    }

    /// Marks a finished file `complete`.
    public static func protectFinished(_ url: URL) {
        #if os(iOS)
        try? FileManager.default.setAttributes([.protectionKey: FileProtectionType.complete], ofItemAtPath: url.path)
        #endif
    }

    /// Marks a cache directory `completeUnlessOpen` (new files inherit it).
    public static func protectDirectory(_ url: URL) {
        #if os(iOS)
        try? FileManager.default.setAttributes([.protectionKey: FileProtectionType.completeUnlessOpen], ofItemAtPath: url.path)
        #endif
    }
}
