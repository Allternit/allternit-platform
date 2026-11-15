import Combine
import Foundation

/// Errors specific to artifact content resolution.
enum ArtifactContentError: Error, LocalizedError, Sendable {
    case badURL
    case binaryContent
    case noContentSource

    var errorDescription: String? {
        switch self {
        case .badURL:
            return "The artifact URL is invalid."
        case .binaryContent:
            return "The artifact content is not text."
        case .noContentSource:
            return "The artifact event carried no content, URL, or fetchable id."
        }
    }
}

/// Loads the content of a chat-frame `ArtifactRecord` for the in-chat card
/// viewer. The `artifact.created` frame is the content carrier, so
/// resolution is a fallback chain over the fields it can carry:
///
/// 1. `url` — provider-hosted file URL (native `file` stream parts). External
///    http(s) URLs are fetched WITHOUT the Clerk Bearer (never leak the token
///    to a third-party host); gateway-relative paths go through APIClient.
/// 2. `inlinePreview` — full inline content (`<document>` extraction path).
/// 3. Artifacts v2 (`GET /api/v2/artifacts/:id`) for an `art_` id — the
///    body of the record the frame refers to.
/// 4. `.failed` with the last error, or `noContentSource`.
@MainActor
final class ArtifactContentLoader: ObservableObject {
    enum State: Equatable, Sendable {
        case idle
        case loading
        /// Loaded text content; an empty string is a valid "empty artifact".
        case loaded(String)
        case failed(String)
    }

    @Published private(set) var state: State = .idle

    /// The loaded content, if any (drives the details view's copy button).
    var content: String? {
        if case .loaded(let text) = state { return text }
        return nil
    }

    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// First load — no-op unless the loader is idle (so re-renders and
    /// re-appearing views don't refetch). Cancellation resets to `.idle`.
    func load(artifact: ArtifactRecord) async {
        guard case .idle = state else { return }
        await resolveAndPublish(artifact)
    }

    /// User-initiated retry from the error state.
    func retry(artifact: ArtifactRecord) async {
        await resolveAndPublish(artifact)
    }

    private func resolveAndPublish(_ artifact: ArtifactRecord) async {
        state = .loading
        do {
            let content = try await resolve(artifact)
            try Task.checkCancellation()
            state = .loaded(content)
        } catch is CancellationError {
            // Sheet dismissed mid-flight; next open starts fresh.
            state = .idle
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    // MARK: - Resolution chain

    private func resolve(_ artifact: ArtifactRecord) async throws -> String {
        var lastError: Error?

        // 1. url (native file artifacts)
        if let urlString = artifact.url?.trimmingCharacters(in: .whitespacesAndNewlines),
           !urlString.isEmpty {
            do {
                return try await fetchURLContent(urlString)
            } catch {
                lastError = error
            }
        }

        // 2. inlinePreview (<document> artifacts — content is already local)
        if let preview = artifact.inlinePreview, !preview.isEmpty {
            return preview
        }

        // 3. Artifacts v2 record body
        if let artifactId = artifact.artifactId, artifactId.hasPrefix("art_") {
            do {
                return try await ArtifactsV2Client.shared.get(id: artifactId).version.body
            } catch {
                lastError = error
            }
        }

        throw lastError ?? ArtifactContentError.noContentSource
    }

    /// Fetches `urlString` as UTF-8 text. Absolute http(s) URLs are fetched
    /// with a plain request (external host — no Clerk token); `data:` URLs are
    /// decoded locally; `file:` URLs are read from disk; anything else is
    /// treated as a gateway-relative path through the authenticated client.
    private func fetchURLContent(_ urlString: String) async throws -> String {
        guard let url = URL(string: urlString) else {
            throw ArtifactContentError.badURL
        }

        let data: Data
        switch url.scheme?.lowercased() {
        case "http", "https":
            let (externalData, response) = try await client.session.data(from: url)
            try client.validate(response, data: externalData)
            data = externalData
        case "data":
            guard let decoded = Self.decodeDataURL(url) else {
                throw ArtifactContentError.badURL
            }
            data = decoded
        case "file":
            data = try Data(contentsOf: url)
        default:
            var request = try await client.authorizedRequest(path: urlString)
            request.setValue("*/*", forHTTPHeaderField: "Accept")
            let (gatewayData, response) = try await client.session.data(for: request)
            try client.validate(response, data: gatewayData)
            data = gatewayData
        }

        guard let text = String(data: data, encoding: .utf8) else {
            throw ArtifactContentError.binaryContent
        }
        return text
    }

    /// Decodes a `data:[<mediatype>][;base64],<data>` URL into raw bytes.
    private static func decodeDataURL(_ url: URL) -> Data? {
        guard url.scheme?.lowercased() == "data" else { return nil }
        let string = url.absoluteString
        guard let commaIndex = string.firstIndex(of: ","),
              commaIndex > string.startIndex else { return nil }
        let metaStart = string.index(string.startIndex, offsetBy: 5) // after "data:"
        let meta = String(string[metaStart..<commaIndex]).lowercased()
        let dataString = String(string[string.index(after: commaIndex)...])
        if meta.contains(";base64") {
            return Data(base64Encoded: dataString)
        }
        return dataString.removingPercentEncoding?.data(using: .utf8)
    }
}
