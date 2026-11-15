import Foundation

/// Serves `SampleWorkspaceAPI` in place of the network while
/// `-sample-workspace` is on. Registered globally, so it covers
/// `URLSession.shared` — the session every API client uses (APIClient,
/// ArtifactsV2Client, RuntimeRelayClient, InstancesClient, …). Only the
/// app's own API hosts and `allternit-relay://` placeholders are answered;
/// pack sprites, Clerk and every other host load as normal.
final class SampleURLProtocol: URLProtocol, @unchecked Sendable {
    private static let registered: Bool = {
        URLProtocol.registerClass(SampleURLProtocol.self)
        return true
    }()

    static func register() {
        _ = registered
    }

    /// The API hosts the app talks to (gateway, cloud-api, gizzi-code).
    private static let hosts: Set<String> = Set(
        [AppConfig.apiBaseURL, AppConfig.cloudAPIBaseURL, AppConfig.gizziCodeBaseURL, AppConfig.aciBaseURL]
            .compactMap { $0.host?.lowercased() }
    )

    static func handles(_ url: URL?) -> Bool {
        guard SampleWorkspace.isEnabled, let url, let scheme = url.scheme?.lowercased() else { return false }
        if scheme == "allternit-relay" { return true }
        guard scheme == "http" || scheme == "https", let host = url.host?.lowercased() else { return false }
        return hosts.contains(host)
    }

    override class func canInit(with request: URLRequest) -> Bool {
        handles(request.url)
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let answer = SampleWorkspaceAPI.answer(method: request.httpMethod ?? "GET", url: url, body: Self.body(of: request))
        let response = HTTPURLResponse(
            url: url,
            statusCode: answer.status,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: answer.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    /// URLSession hands a protocol the body as a stream, not `httpBody`.
    private static func body(of request: URLRequest) -> Data? {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 16_384)
        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: buffer.count)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }
}
