import Foundation

/// Allternit Cloud file storage (R2) for files too big to travel as base64
/// (web lib/attachments/cloud-upload.ts).
///
/// Flow: `POST /api/v1/files/uploads` (plan caps checked) → PUT the bytes to
/// the presigned URL → `POST /api/v1/files/:id/complete` (size verified) →
/// `GET /api/v1/files/:id?expires=…` for a permanent link (`linkUrl`, else a
/// 7-day presigned URL).
final class CloudFilesClient: @unchecked Sendable {
    static let shared = CloudFilesClient()

    /// Below this, base64 inline is fine and avoids a round trip.
    static let minCloudBytes = 1024 * 1024
    /// How long the presigned fallback link stays valid.
    private static let linkSeconds = 7 * 24 * 3600

    struct UploadError: LocalizedError {
        let message: String
        let code: String
        var errorDescription: String? { message }
    }

    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = AppConfig.cloudAPIBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// web shouldUploadToCloud: big enough, and signed in to the cloud.
    func shouldUpload(size: Int) async -> Bool {
        guard size >= Self.minCloudBytes else { return false }
        return await clerkToken() != nil
    }

    func upload(_ data: Data, name: String, contentType: String) async throws -> (fileId: String, url: String) {
        try await upload(.data(data), bytes: data.count, name: name, contentType: contentType)
    }

    /// Streams a file from disk (video footage is too big to hold in memory).
    func upload(fileAt url: URL, name: String, contentType: String) async throws -> (fileId: String, url: String) {
        let size = (try? FileManager.default.attributesOfItem(atPath: url.path)[.size] as? Int) ?? 0
        return try await upload(.file(url), bytes: size, name: name, contentType: contentType)
    }

    private enum Source { case data(Data), file(URL) }

    private func upload(_ source: Source, bytes: Int, name: String, contentType: String) async throws -> (fileId: String, url: String) {
        let type = contentType.isEmpty ? "application/octet-stream" : contentType
        struct Begin: Encodable { let name: String; let contentType: String; let bytes: Int }
        struct Started: Decodable { let fileId: String; let putUrl: String }
        struct Link: Decodable { let url: String; let linkUrl: String? }
        let meta = try JSONEncoder().encode(Begin(name: name, contentType: type, bytes: bytes))

        let started: Started = try await call("api/v1/files/uploads", method: "POST", body: meta,
                                              fallback: "The file could not be uploaded.")

        guard let putURL = URL(string: started.putUrl) else {
            throw UploadError(message: "The file could not be uploaded.", code: "put_url")
        }
        var put = URLRequest(url: putURL)
        put.httpMethod = "PUT"
        put.setValue(type, forHTTPHeaderField: "Content-Type")
        let putResponse: URLResponse
        switch source {
        case .data(let data): (_, putResponse) = try await session.upload(for: put, from: data)
        case .file(let file): (_, putResponse) = try await session.upload(for: put, fromFile: file)
        }
        let putStatus = (putResponse as? HTTPURLResponse)?.statusCode ?? 0
        guard (200...299).contains(putStatus) else {
            throw UploadError(message: "The file could not be uploaded.", code: "put_\(putStatus)")
        }

        let id = started.fileId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? started.fileId
        let _: EmptyReply = try await call("api/v1/files/\(id)/complete", method: "POST", body: meta,
                                           fallback: "The upload could not be verified.")
        let link: Link = try await call("api/v1/files/\(id)?expires=\(Self.linkSeconds)", method: "GET", body: nil,
                                        fallback: "The file link could not be created.")
        return (started.fileId, link.linkUrl ?? link.url)
    }

    private struct EmptyReply: Decodable {}

    private func clerkToken() async -> String? {
        guard let token = try? await AuthManager.shared.getToken(), !token.isEmpty else { return nil }
        return token
    }

    private func call<T: Decodable>(_ path: String, method: String, body: Data?, fallback: String) async throws -> T {
        let url: URL
        if let split = path.firstIndex(of: "?"),
           var components = URLComponents(url: baseURL.appendingPathComponent(String(path[..<split])), resolvingAgainstBaseURL: false) {
            components.percentEncodedQuery = String(path[path.index(after: split)...])
            url = components.url ?? baseURL.appendingPathComponent(path)
        } else {
            url = baseURL.appendingPathComponent(path)
        }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
        }
        if let token = await clerkToken() {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200...299).contains(status) else {
            let failure = try? JSONDecoder().decode(UploadFailureBody.self, from: data)
            throw UploadError(message: failure?.message ?? fallback, code: failure?.code ?? "http_\(status)")
        }
        if T.self == EmptyReply.self, let empty = EmptyReply() as? T { return empty }
        do {
            return try JSONDecoder().decode(T.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }
}

/// A cloud-api error body (`{ code, message }`).
private struct UploadFailureBody: Decodable {
    let code: String?
    let message: String?
}
