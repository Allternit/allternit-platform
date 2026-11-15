import Foundation

/// One port currently held open by a pty's own process tree
/// (`GET v1/pty/:id/ports`, `cmd/gizzi-code/src/runtime/server/routes/pty.ts`).
/// `command` is the owning process's name (e.g. "node", "Python") when
/// available, for a friendlier row label than a bare port number.
struct DevServerPort: Decodable, Identifiable, Sendable {
    let port: Int
    let command: String?

    var id: Int { port }
}

/// A short-lived (5 minute) capability token scoped to one pty+port pair,
/// minted via `POST v1/pty/:id/preview/token`.
struct DevPreviewToken: Decodable, Sendable {
    let token: String
    let expiresAt: String
}

/// Client for gizzi-code's dev-server preview routes (`routes/pty.ts`'s
/// `/ports`, `/preview/token`, `/preview/:port/*`) — port detection and
/// preview-token minting for the Code tab's dev-server preview. Like
/// `FileClient`/`SessionDiffClient` this connects to whatever `baseURL` the
/// caller resolved (see `InstanceConnection`); it does no instance/mesh
/// resolution itself.
final class DevPreviewClient: @unchecked Sendable {
    let baseURL: URL
    private let endpointBaseURL: URL

    init(baseURL: URL = AppConfig.gizziCodeBaseURL) {
        self.baseURL = baseURL
        self.endpointBaseURL = baseURL
    }

    /// Per-call target: the given base URL, or — for the static default —
    /// whatever `InstanceConnection.resolve()` picks now (explicit URL →
    /// mesh → the relay to the user's computer).
    private func client() async throws -> APIClient {
        try await InstanceConnection.gizziClient(for: endpointBaseURL)
    }

    /// `GET v1/pty/:id/ports`.
    func ports(ptyID: String) async throws -> [DevServerPort] {
        try await client().get(path: "v1/pty/\(Self.escape(ptyID))/ports")
    }

    /// `POST v1/pty/:id/preview/token`.
    func mintPreviewToken(ptyID: String, port: Int) async throws -> DevPreviewToken {
        try await client().post(
            path: "v1/pty/\(Self.escape(ptyID))/preview/token",
            body: MintPreviewTokenRequest(port: port)
        )
    }

    /// The `WKWebView`'s initial load URL: `<baseURL>/v1/pty/:id/preview/:port/?token=`.
    /// Built via `URLComponents` (not `appendingPathComponent`, whose
    /// trailing-slash handling is ambiguous) — the trailing slash matters:
    /// the server's injected `<base href>` mirrors this exact shape so
    /// relative asset/link paths keep resolving through the same relay
    /// route rather than replacing its last path segment.
    func previewURL(ptyID: String, port: Int, token: String) -> URL {
        var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)!
        let existingPath = components.path.hasSuffix("/") ? String(components.path.dropLast()) : components.path
        // `.path` takes the DECODED path — URLComponents percent-encodes it
        // when building `.url`, so ptyID goes in raw here (pre-escaping it
        // ourselves, as `Self.escape` does for plain path strings elsewhere
        // in this file, would double-encode).
        components.path = "\(existingPath)/v1/pty/\(ptyID)/preview/\(port)/"
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url!
    }

    private struct MintPreviewTokenRequest: Encodable {
        let port: Int
    }

    private static func escape(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }
}
