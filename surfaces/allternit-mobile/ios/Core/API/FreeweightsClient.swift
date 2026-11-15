import CryptoKit
import Foundation

// MARK: - Catalog types (shapes from the live worker, freeweights/worker/src/index.ts)

/// One row of `GET https://freeweights.allternit.com/api/models` (the
/// worker's `summary()`): a Hugging Face–style model summary plus the
/// Freeweights section, license id and source badges. Rows carry no size;
/// `FreeweightsCatalogRow` (from `/api/catalog`) fills that in.
struct FreeweightsModelSummary: Codable, Sendable, Identifiable, Equatable {
    let id: String
    let sha: String?
    let pipelineTag: String?
    let libraryName: String?
    let tags: [String]?
    let downloads: Int?
    let lastModified: String?
    let section: String?
    let license: String?
    let badges: [String]?

    enum CodingKeys: String, CodingKey {
        case id, sha, tags, downloads, lastModified, section, license, badges
        case pipelineTag = "pipeline_tag"
        case libraryName = "library_name"
    }

    var org: String { String(id.split(separator: "/").first ?? "") }
    var name: String { String(id.split(separator: "/").last ?? Substring(id)) }
}

/// A row of `GET /api/catalog` — the curated catalog record behind each
/// model (catalog/rows/*.json). Used for size, license class and whether the
/// entry is link-only.
struct FreeweightsCatalogRow: Codable, Sendable, Equatable {
    let id: String
    let displayName: String?
    let kind: String?
    let section: String?
    let licenseId: String?
    let licenseClass: String?
    let licenseUrl: String?
    let redistribution: String?
    let requiresTerms: Bool?
    let manifestUrl: String?
    let upstreamUrl: String?
    let totalSize: Int64?
    let params: Int64?
    let formats: [String]?
    let tasks: [String]?
    let note: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, section, redistribution, params, formats, tasks, note
        case displayName = "display_name"
        case licenseId = "license_id"
        case licenseClass = "license_class"
        case licenseUrl = "license_url"
        case requiresTerms = "requires_terms"
        case manifestUrl = "manifest_url"
        case upstreamUrl = "upstream_url"
        case totalSize = "total_size"
    }

    /// Same rule as the site (freeweights/site/build.mjs `linkOnly`):
    /// non-commercial or gated licenses, or no manifest. Freeweights never
    /// hosts or pins these, so the app links to the site instead of pulling.
    var isLinkOnly: Bool {
        licenseClass == "non-commercial" || licenseClass == "gated" || (manifestUrl ?? "").isEmpty
    }
}

private struct FreeweightsCatalogResponse: Codable {
    let models: [FreeweightsCatalogRow]
}

/// `GET /api/models/{org}/{repo}` — the summary plus files, sources and the
/// catalog record.
struct FreeweightsModelDetail: Codable, Sendable, Equatable {
    struct Sibling: Codable, Sendable, Equatable {
        let rfilename: String
    }

    struct Source: Codable, Sendable, Equatable {
        let type: String?
        let state: String?
        let priority: Int?
        let lastChecked: String?

        enum CodingKeys: String, CodingKey {
            case type, state, priority
            case lastChecked = "last_checked"
        }
    }

    let id: String
    let sha: String?
    let pipelineTag: String?
    let libraryName: String?
    let section: String?
    let license: String?
    let badges: [String]?
    let siblings: [Sibling]?
    let sources: [Source]?
    let manifestUrl: String?
    let upstreamUrl: String?
    let catalog: FreeweightsCatalogRow?

    enum CodingKeys: String, CodingKey {
        case id, sha, section, license, badges, siblings, sources, catalog
        case pipelineTag = "pipeline_tag"
        case libraryName = "library_name"
        case manifestUrl = "manifest_url"
        case upstreamUrl = "upstream_url"
    }

    var isLinkOnly: Bool { catalog?.isLinkOnly ?? (manifestUrl ?? "").isEmpty }
    var filesCount: Int { siblings?.count ?? 0 }
}

/// One page of search results plus the next cursor from the `Link` header.
struct FreeweightsPage: Codable, Sendable, Equatable {
    let models: [FreeweightsModelSummary]
    let nextCursor: String?
}

// MARK: - Results

/// What a catalog call gives back. A failed request never throws: it falls
/// back to the last copy on disk (`.cached` with its age) or says the
/// catalog is down.
enum FreeweightsLoad<Value: Sendable>: Sendable {
    case live(Value)
    case cached(Value, age: TimeInterval)
    case catalogDown(reason: String)

    var value: Value? {
        switch self {
        case .live(let v), .cached(let v, _): return v
        case .catalogDown: return nil
        }
    }

    /// Age of the cached copy, nil when live or down.
    var staleAge: TimeInterval? {
        if case .cached(_, let age) = self { return age }
        return nil
    }
}

// MARK: - Client

/// Reads the public Freeweights catalog (https://freeweights.allternit.com)
/// and keeps the last answer for each request on disk so the Models screen
/// still shows something offline. No auth: the catalog is public.
final class FreeweightsClient: @unchecked Sendable {
    static let shared = FreeweightsClient()

    static let siteURL = URL(string: "https://freeweights.allternit.com")!

    private let baseURL: URL
    private let session: URLSession
    private let cacheDirectory: URL?

    init(baseURL: URL = FreeweightsClient.siteURL,
         session: URLSession? = nil,
         cacheDirectory: URL? = FreeweightsClient.defaultCacheDirectory()) {
        self.baseURL = baseURL
        if let session {
            self.session = session
        } else {
            let config = URLSessionConfiguration.default
            config.timeoutIntervalForRequest = 10
            config.requestCachePolicy = .reloadIgnoringLocalCacheData
            self.session = URLSession(configuration: config)
        }
        self.cacheDirectory = cacheDirectory
    }

    static func defaultCacheDirectory() -> URL? {
        FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
            .appendingPathComponent("freeweights", isDirectory: true)
    }

    /// The model's page on the site (link-only entries open this instead
    /// of pulling).
    static func pageURL(for id: String) -> URL {
        siteURL.appendingPathComponent(id)
    }

    // MARK: Calls

    /// `GET /api/models?search=&section=&pipeline_tag=&limit=&cursor=`.
    func search(query: String = "",
                section: String? = nil,
                pipelineTag: String? = nil,
                limit: Int = 25,
                cursor: String? = nil) async -> FreeweightsLoad<FreeweightsPage> {
        var components = URLComponents(url: baseURL.appendingPathComponent("api/models"), resolvingAgainstBaseURL: false)!
        var items = [URLQueryItem(name: "limit", value: String(limit))]
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty { items.append(URLQueryItem(name: "search", value: trimmed)) }
        if let section, !section.isEmpty { items.append(URLQueryItem(name: "section", value: section)) }
        if let pipelineTag, !pipelineTag.isEmpty { items.append(URLQueryItem(name: "pipeline_tag", value: pipelineTag)) }
        if let cursor, !cursor.isEmpty { items.append(URLQueryItem(name: "cursor", value: cursor)) }
        components.queryItems = items
        guard let url = components.url else { return .catalogDown(reason: "Bad search URL") }

        return await load(url) { data, response in
            let models = try JSONDecoder().decode([FreeweightsModelSummary].self, from: data)
            let link = (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Link")
            return FreeweightsPage(models: models, nextCursor: Self.nextCursor(fromLinkHeader: link))
        }
    }

    /// `GET /api/models/{org}/{repo}`.
    func detail(id: String) async -> FreeweightsLoad<FreeweightsModelDetail> {
        var url = baseURL.appendingPathComponent("api/models")
        for part in id.split(separator: "/") { url.appendPathComponent(String(part)) }
        return await load(url) { data, _ in
            try JSONDecoder().decode(FreeweightsModelDetail.self, from: data)
        }
    }

    /// `GET /api/catalog` keyed by id — size, license class and link-only
    /// for list rows (`/api/models` rows carry none of these).
    func catalogRows() async -> FreeweightsLoad<[String: FreeweightsCatalogRow]> {
        await load(baseURL.appendingPathComponent("api/catalog")) { data, _ in
            let rows = try JSONDecoder().decode(FreeweightsCatalogResponse.self, from: data).models
            return Dictionary(rows.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        }
    }

    // MARK: Loading + disk cache

    private struct CacheEntry: Codable {
        let fetchedAt: Date
        let body: Data
        let link: String?
    }

    /// Fetches `url`; on success caches the body, on any failure (network,
    /// 5xx, undecodable) answers the cached copy or `.catalogDown`.
    private func load<Value: Sendable>(_ url: URL,
                                       decode: @escaping (Data, URLResponse?) throws -> Value) async -> FreeweightsLoad<Value> {
        var request = URLRequest(url: url)
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        let failure: String
        do {
            let (data, response) = try await session.data(for: request)
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            if (200...299).contains(status) {
                let value = try decode(data, response)
                writeCache(url, data: data, link: (response as? HTTPURLResponse)?.value(forHTTPHeaderField: "Link"))
                return .live(value)
            }
            failure = status == 404 ? "Not in the catalog" : "Freeweights answered \(status)"
            if status == 404 { return .catalogDown(reason: failure) }
        } catch is CancellationError {
            return .catalogDown(reason: "Cancelled")
        } catch {
            failure = error.localizedDescription
        }

        if let entry = readCache(url) {
            let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil,
                                           headerFields: entry.link.map { ["Link": $0] })
            if let value = try? decode(entry.body, response) {
                return .cached(value, age: Date().timeIntervalSince(entry.fetchedAt))
            }
        }
        return .catalogDown(reason: failure)
    }

    private func cacheFile(for url: URL) -> URL? {
        guard let cacheDirectory else { return nil }
        let digest = SHA256.hash(data: Data(url.absoluteString.utf8))
        let name = digest.map { String(format: "%02x", $0) }.joined()
        return cacheDirectory.appendingPathComponent("\(name).json")
    }

    private func writeCache(_ url: URL, data: Data, link: String?) {
        guard let file = cacheFile(for: url), let cacheDirectory else { return }
        try? FileManager.default.createDirectory(at: cacheDirectory, withIntermediateDirectories: true)
        let entry = CacheEntry(fetchedAt: Date(), body: data, link: link)
        if let encoded = try? JSONEncoder().encode(entry) {
            try? encoded.write(to: file, options: .atomic)
        }
    }

    private func readCache(_ url: URL) -> CacheEntry? {
        guard let file = cacheFile(for: url), let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(CacheEntry.self, from: data)
    }

    /// `</api/models?limit=2&cursor=2>; rel="next"` → `"2"`.
    static func nextCursor(fromLinkHeader header: String?) -> String? {
        guard let header else { return nil }
        for part in header.split(separator: ",") {
            guard part.contains("rel=\"next\""),
                  let open = part.firstIndex(of: "<"),
                  let close = part.firstIndex(of: ">"), open < close else { continue }
            let target = String(part[part.index(after: open)..<close])
            let components = URLComponents(string: target)
            return components?.queryItems?.first(where: { $0.name == "cursor" })?.value
        }
        return nil
    }
}

// MARK: - Pulls on the user's computer (local-engine via allternit-api)

/// The `verification` object a download status carries
/// (contracts/model-source/model-source.schema.json).
struct ModelVerification: Codable, Sendable, Equatable {
    let state: String
    let sourceUsed: String?
    let sourceUrl: String?
    let manifestCommit: String?
    let checkedAt: String?

    enum CodingKeys: String, CodingKey {
        case state
        case sourceUsed = "source_used"
        case sourceUrl = "source_url"
        case manifestCommit = "manifest_commit"
        case checkedAt = "checked_at"
    }

    /// The label the app shows for each state.
    static func label(for verification: ModelVerification?) -> String {
        guard let verification else { return "Not checked by this version" }
        switch verification.state {
        case "freeweights_signed": return "Verified by Freeweights"
        case "publisher_hash": return "Matches the publisher's hash"
        case "pinned": return "Pinned by Allternit"
        case "unverified": return "Not verified"
        default: return "Not checked by this version"
        }
    }
}

/// `POST /models/download` answer (services/local-engine routes/models.rs
/// `DownloadResponse`).
struct ComputerPullStarted: Codable, Sendable, Equatable {
    let id: String
    let repoId: String
    let revision: String?
    let status: String
    let message: String?

    enum CodingKeys: String, CodingKey {
        case id, revision, status, message
        case repoId = "repo_id"
    }
}

/// One `GET /models` row (`CachedModelResponse`) with the optional
/// `verification` the model-source contract adds.
struct ComputerModelStatus: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let name: String
    let status: String
    let downloadedBytes: Int64
    let totalBytes: Int64
    let verification: ModelVerification?

    enum CodingKeys: String, CodingKey {
        case id, name, status, verification
        case downloadedBytes = "downloaded_bytes"
        case totalBytes = "total_bytes"
    }

    init(id: String, name: String, status: String, downloadedBytes: Int64 = 0,
         totalBytes: Int64 = 0, verification: ModelVerification? = nil) {
        self.id = id
        self.name = name
        self.status = status
        self.downloadedBytes = downloadedBytes
        self.totalBytes = totalBytes
        self.verification = verification
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = (try? c.decodeIfPresent(String.self, forKey: .name)) ?? ""
        status = (try? c.decodeIfPresent(String.self, forKey: .status)) ?? "pending"
        downloadedBytes = (try? c.decodeIfPresent(Int64.self, forKey: .downloadedBytes)) ?? 0
        totalBytes = (try? c.decodeIfPresent(Int64.self, forKey: .totalBytes)) ?? 0
        // A malformed or future verification shape reads as "not checked".
        verification = try? c.decodeIfPresent(ModelVerification.self, forKey: .verification)
    }

    var progress: Double? {
        guard totalBytes > 0 else { return nil }
        return min(1, Double(downloadedBytes) / Double(totalBytes))
    }

    var isFinished: Bool { status == "ready" || status == "failed" }
}

private struct ComputerModelsResponse: Codable {
    let models: [ComputerModelStatus]
}

/// Sends model pulls to the user's computer: allternit-api's
/// `/api/local-engine/*` proxy (cmd/allternit-api/src/local_engine_routes.rs)
/// on that computer, reached through `InstanceConnection.runtimeAPIData` —
/// in Release the cloud runtime relay (`POST {cloud}/api/v1/runtime-devices/
/// :id/proxy`), which Allternit Desktop forwards to its own allternit-api
/// (auth-manager.ts `relayLocalUrl`), whose proxy reaches the local engine
/// on the same machine.
final class ComputerModelPullClient: @unchecked Sendable {
    static let shared = ComputerModelPullClient()

    private let client: APIClient
    private let baseURL: URL

    init(client: APIClient = .shared, baseURL: URL = ComputerModelPullClient.defaultBaseURL) {
        self.client = client
        self.baseURL = baseURL
    }

    /// `{apiBase}/local-engine`, derived like `AppConfig.factoryBaseURL`.
    static let defaultBaseURL: URL = {
        let api = AppConfig.apiBaseURL
        let apiBase = api.lastPathComponent == "v1" ? api.deletingLastPathComponent() : api
        return apiBase.appendingPathComponent("local-engine")
    }()

    private struct PullBody: Encodable {
        let repo_id: String
        let revision: String
    }

    /// `POST /api/local-engine/models/download` `{ repo_id, revision }`.
    func startPull(repoId: String, revision: String?) async throws -> ComputerPullStarted {
        var request = try await client.authorizedRequest(url: baseURL.appendingPathComponent("models/download"), method: "POST")
        request.httpBody = try JSONEncoder().encode(PullBody(repo_id: repoId, revision: revision ?? "main"))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data, method: "POST")
        do {
            return try JSONDecoder().decode(ComputerPullStarted.self, from: data)
        } catch {
            throw APIError.decoding(error)
        }
    }

    /// `GET /api/local-engine/models` — the computer's model cache with
    /// download progress.
    func listModels() async throws -> [ComputerModelStatus] {
        let request = try await client.authorizedRequest(url: baseURL.appendingPathComponent("models"))
        let (data, response) = try await InstanceConnection.runtimeAPIData(for: request, session: client.session)
        try client.validate(response, data: data)
        do {
            return try JSONDecoder().decode(ComputerModelsResponse.self, from: data).models
        } catch {
            throw APIError.decoding(error)
        }
    }

    func status(id: String) async throws -> ComputerModelStatus? {
        try await listModels().first { $0.id == id }
    }
}
