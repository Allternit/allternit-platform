import CryptoKit
import Foundation

// MARK: - Models (cloud-api /api/v1/sign-requests, snake_case JSON)

/// A signing area on a PDF page, in PDF points with the origin bottom-left.
struct SignSlot: Codable, Equatable, Hashable, Sendable {
    var id: String
    var label: String
    /// 0-based page index.
    var page: Int
    var x: Double
    var y: Double
    var w: Double
    var h: Double
}

struct SignDetail: Codable, Equatable, Hashable, Sendable {
    var label: String
    var value: String
}

struct SignRecipient: Codable, Equatable, Hashable, Sendable {
    var name: String?
    var email: String
}

enum SignStatus: String, Codable, Sendable {
    case pending, signed, declined, cancelled, expired
}

enum SignMethod: String, Codable, Sendable {
    case draw, type, upload
}

struct SignRequest: Codable, Identifiable, Equatable, Sendable {
    var id: String
    var status: SignStatus
    var requesterKind: String
    var requesterName: String?
    var sourceSurface: String
    var threadId: String?
    var title: String
    var summary: String?
    var details: [SignDetail]
    var documentFileId: String
    var documentName: String
    var documentSha256: String
    var slots: [SignSlot]
    var sendTo: SignRecipient?
    var createdAt: String
    var expiresAt: String
    var resolvedAt: String?
    var signatureMethod: SignMethod?
    var signatureSlot: SignSlot?
    var signedFileId: String?
    var signedSha256: String?
    var consentVersion: String?
    var signerName: String?
    var signerEmail: String?
    var signedSurface: String?
    var sentAt: String?
    var sentTo: String?
    var documentUrl: String?
    var signedUrl: String?

    enum CodingKeys: String, CodingKey {
        case id, status, title, summary, details, slots
        case requesterKind = "requester_kind"
        case requesterName = "requester_name"
        case sourceSurface = "source_surface"
        case threadId = "thread_id"
        case documentFileId = "document_file_id"
        case documentName = "document_name"
        case documentSha256 = "document_sha256"
        case sendTo = "send_to"
        case createdAt = "created_at"
        case expiresAt = "expires_at"
        case resolvedAt = "resolved_at"
        case signatureMethod = "signature_method"
        case signatureSlot = "signature_slot"
        case signedFileId = "signed_file_id"
        case signedSha256 = "signed_sha256"
        case consentVersion = "consent_version"
        case signerName = "signer_name"
        case signerEmail = "signer_email"
        case signedSurface = "signed_surface"
        case sentAt = "sent_at"
        case sentTo = "sent_to"
        case documentUrl = "document_url"
        case signedUrl = "signed_url"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        status = try c.decode(SignStatus.self, forKey: .status)
        requesterKind = try c.decode(String.self, forKey: .requesterKind)
        requesterName = try c.decodeIfPresent(String.self, forKey: .requesterName)
        sourceSurface = try c.decode(String.self, forKey: .sourceSurface)
        threadId = try c.decodeIfPresent(String.self, forKey: .threadId)
        title = try c.decode(String.self, forKey: .title)
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        details = try c.decodeIfPresent([SignDetail].self, forKey: .details) ?? []
        documentFileId = try c.decode(String.self, forKey: .documentFileId)
        documentName = try c.decode(String.self, forKey: .documentName)
        documentSha256 = try c.decode(String.self, forKey: .documentSha256)
        slots = try c.decodeIfPresent([SignSlot].self, forKey: .slots) ?? []
        sendTo = try c.decodeIfPresent(SignRecipient.self, forKey: .sendTo)
        createdAt = try c.decode(String.self, forKey: .createdAt)
        expiresAt = try c.decode(String.self, forKey: .expiresAt)
        resolvedAt = try c.decodeIfPresent(String.self, forKey: .resolvedAt)
        signatureMethod = try c.decodeIfPresent(SignMethod.self, forKey: .signatureMethod)
        signatureSlot = try c.decodeIfPresent(SignSlot.self, forKey: .signatureSlot)
        signedFileId = try c.decodeIfPresent(String.self, forKey: .signedFileId)
        signedSha256 = try c.decodeIfPresent(String.self, forKey: .signedSha256)
        consentVersion = try c.decodeIfPresent(String.self, forKey: .consentVersion)
        signerName = try c.decodeIfPresent(String.self, forKey: .signerName)
        signerEmail = try c.decodeIfPresent(String.self, forKey: .signerEmail)
        signedSurface = try c.decodeIfPresent(String.self, forKey: .signedSurface)
        sentAt = try c.decodeIfPresent(String.self, forKey: .sentAt)
        sentTo = try c.decodeIfPresent(String.self, forKey: .sentTo)
        documentUrl = try c.decodeIfPresent(String.self, forKey: .documentUrl)
        signedUrl = try c.decodeIfPresent(String.self, forKey: .signedUrl)
    }
}

/// Body of `POST /api/v1/sign-requests`.
struct SignRequestDraft: Encodable, Sendable {
    var title: String
    var summary: String?
    var details: [SignDetail]?
    var documentFileId: String
    var documentSha256: String
    var slots: [SignSlot]?
    var sendTo: SignRecipient?
    var requesterKind: String = "self"
    var requesterName: String?
    var sourceSurface: String = "ios"
    var threadId: String?
    var expiresInDays: Int?

    enum CodingKeys: String, CodingKey {
        case title, summary, details, slots
        case documentFileId = "document_file_id"
        case documentSha256 = "document_sha256"
        case sendTo = "send_to"
        case requesterKind = "requester_kind"
        case requesterName = "requester_name"
        case sourceSurface = "source_surface"
        case threadId = "thread_id"
        case expiresInDays = "expires_in_days"
    }
}

/// Body of `POST /api/v1/sign-requests/:id/sign`.
struct SignSubmission: Encodable, Sendable {
    var consent: Bool = true
    var consentVersion: String = SignRequestsClient.consentVersion
    var method: SignMethod
    /// PNG, at most 256 KB, base64.
    var signaturePngBase64: String
    var slot: SignSlot
    var documentSha256: String
    var signedFileId: String
    var signedSha256: String
    var surface: String = "ios"

    enum CodingKeys: String, CodingKey {
        case consent, method, slot, surface
        case consentVersion = "consent_version"
        case signaturePngBase64 = "signature_png_base64"
        case documentSha256 = "document_sha256"
        case signedFileId = "signed_file_id"
        case signedSha256 = "signed_sha256"
    }
}

// MARK: - Client

/// Review & sign: the sign-request record in Allternit Cloud. Auth, base URL
/// and error body follow `CloudFilesClient`. An agent can never sign; `/sign`
/// only accepts an interactive user session (403 `user-session-required`).
final class SignRequestsClient: @unchecked Sendable {
    static let shared = SignRequestsClient()

    /// Consent text version sent with every signature (Design decision 7).
    static let consentVersion = "v1"
    /// The line shown above Sign & send; the line itself is the consent.
    static let consentText = "By tapping Sign & send, you agree to sign this document electronically. This adds an image of your signature and a signing record. It is not a certificate-based digital signature."
    /// Server cap on the signature PNG.
    static let maxSignatureBytes = 256 * 1024

    struct SignError: LocalizedError {
        let status: Int
        let code: String
        let message: String
        var errorDescription: String? { message }
        var isNotPending: Bool { status == 409 && code == "not-pending" }
        var isDocumentChanged: Bool { status == 409 && code == "document-changed" }
        var isExpired: Bool { status == 410 }
    }

    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL = AppConfig.cloudAPIBaseURL, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.session = session
    }

    /// Lowercase hex SHA-256 (the contract's `document_sha256` / `signed_sha256`).
    static func sha256Hex(_ data: Data) -> String {
        SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
    }

    func create(_ draft: SignRequestDraft) async throws -> SignRequest {
        try await json("api/v1/sign-requests", method: "POST", body: draft, fallback: "The signature request could not be created.")
    }

    func list(status: SignStatus? = nil, threadId: String? = nil, limit: Int? = nil) async throws -> [SignRequest] {
        struct Page: Decodable { let items: [SignRequest] }
        var query: [URLQueryItem] = []
        if let status { query.append(URLQueryItem(name: "status", value: status.rawValue)) }
        if let threadId { query.append(URLQueryItem(name: "thread_id", value: threadId)) }
        if let limit { query.append(URLQueryItem(name: "limit", value: String(limit))) }
        let data = try await send("api/v1/sign-requests", method: "GET", query: query, body: nil,
                                  fallback: "Your signature requests could not be loaded.")
        do { return try JSONDecoder().decode(Page.self, from: data).items } catch { throw APIError.decoding(error) }
    }

    func get(_ id: String) async throws -> SignRequest {
        try await json("api/v1/sign-requests/\(Self.seg(id))", method: "GET", body: Optional<SignNoBody>.none,
                       fallback: "The signature request could not be loaded.")
    }

    /// The stored signature image (owner only).
    func signature(_ id: String) async throws -> Data {
        try await send("api/v1/sign-requests/\(Self.seg(id))/signature", method: "GET", query: [], body: nil,
                       fallback: "The signature could not be loaded.")
    }

    func sign(_ id: String, _ submission: SignSubmission) async throws -> SignRequest {
        try await json("api/v1/sign-requests/\(Self.seg(id))/sign", method: "POST", body: submission,
                       fallback: "The signature could not be saved.")
    }

    func decline(_ id: String, reason: String? = nil) async throws -> SignRequest {
        struct Body: Encodable { let reason: String? }
        return try await json("api/v1/sign-requests/\(Self.seg(id))/decline", method: "POST", body: Body(reason: reason),
                              fallback: "The request could not be declined.")
    }

    func cancel(_ id: String) async throws -> SignRequest {
        try await json("api/v1/sign-requests/\(Self.seg(id))/cancel", method: "POST", body: SignNoBody(),
                       fallback: "The request could not be cancelled.")
    }

    /// Records that the signed copy was emailed (only when signed).
    func markSent(_ id: String, to: String) async throws -> SignRequest {
        struct Body: Encodable { let to: String }
        return try await json("api/v1/sign-requests/\(Self.seg(id))/sent", method: "POST", body: Body(to: to),
                              fallback: "The signed copy could not be recorded as sent.")
    }

    // MARK: Transport

    private struct SignNoBody: Encodable {}

    private static func seg(_ id: String) -> String {
        id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
    }

    private func clerkToken() async -> String? {
        guard let token = try? await AuthManager.shared.getToken(), !token.isEmpty else { return nil }
        return token
    }

    private func json<T: Decodable, B: Encodable>(_ path: String, method: String, body: B?, fallback: String) async throws -> T {
        let encoded = try body.map { try JSONEncoder().encode($0) }
        let data = try await send(path, method: method, query: [], body: encoded, fallback: fallback)
        do { return try JSONDecoder().decode(T.self, from: data) } catch { throw APIError.decoding(error) }
    }

    private func send(_ path: String, method: String, query: [URLQueryItem], body: Data?, fallback: String) async throws -> Data {
        var components = URLComponents(url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)
        if !query.isEmpty { components?.queryItems = query }
        guard let url = components?.url else { throw APIError.invalidResponse }
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
            let failure = try? JSONDecoder().decode(SignFailureBody.self, from: data)
            throw SignError(status: status, code: failure?.code ?? "http_\(status)", message: failure?.message ?? fallback)
        }
        return data
    }
}

/// A cloud-api error body (`{ code, message }`).
private struct SignFailureBody: Decodable {
    let code: String?
    let message: String?
}
