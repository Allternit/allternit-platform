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
    /// Absolute URL of the publisher's logo, or null. Newer worker deploys
    /// only; absent on older ones.
    let publisherLogo: String?
    /// "community" for a repo a signed-in user uploaded; "catalog" or absent
    /// for the curated catalog. Absent on workers that predate uploads.
    var source: String?

    enum CodingKeys: String, CodingKey {
        case id, sha, tags, downloads, lastModified, section, license, badges, source
        case pipelineTag = "pipeline_tag"
        case libraryName = "library_name"
        case publisherLogo = "publisher_logo"
    }

    var publisherLogoURL: URL? {
        guard let publisherLogo, let url = URL(string: publisherLogo),
              url.scheme == "https" || url.scheme == "http" else { return nil }
        return url
    }

    var org: String { String(id.split(separator: "/").first ?? "") }
    var name: String { String(id.split(separator: "/").last ?? Substring(id)) }
    var isCommunity: Bool { source == "community" }
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
    var source: String?
    /// Community repos only: draft | scanning | live | held (spec 11.6).
    /// It describes what is waiting; `sha` + `siblings` are the last clean
    /// head being served.
    var status: String?

    enum CodingKeys: String, CodingKey {
        case id, sha, section, license, badges, siblings, sources, catalog, source, status
        case pipelineTag = "pipeline_tag"
        case libraryName = "library_name"
        case manifestUrl = "manifest_url"
        case upstreamUrl = "upstream_url"
    }

    var isCommunity: Bool { source == "community" }
    /// Community repos have no catalog record, but they are served by
    /// Freeweights itself, so they are never link-only.
    var isLinkOnly: Bool { isCommunity ? false : (catalog?.isLinkOnly ?? (manifestUrl ?? "").isEmpty) }
    var filesCount: Int { siblings?.count ?? 0 }
    /// A community repo can be downloaded once a clean commit is served.
    var isDownloadable: Bool { isCommunity ? sha != nil : true }
    /// One line for a community repo whose newest commit is not served yet.
    var communityStatusNote: String? {
        guard isCommunity else { return nil }
        if sha == nil { return "Files are being scanned. Downloads open when every file passes." }
        switch status {
        case "scanning": return "An update is being scanned. The last clean version is served."
        case "held": return "An update did not pass its scan. The last clean version is served."
        default: return nil
        }
    }
}

/// One page of search results plus the next cursor from the `Link` header.
struct FreeweightsPage: Codable, Sendable, Equatable {
    let models: [FreeweightsModelSummary]
    let nextCursor: String?
}

// MARK: - My uploads (signed in)

/// One row of `GET /api/fw/v1/repos` (spec/parts/write-api.yaml `MyRepo`):
/// the caller's own community repos in every state, with scan counts for
/// the head commit.
struct FreeweightsMyRepo: Codable, Sendable, Identifiable, Equatable {
    struct Scans: Codable, Sendable, Equatable {
        let queued: Int
        let scanning: Int
        let clean: Int
        let held: Int
        let error: Int
        let heldCodes: [String]?

        enum CodingKeys: String, CodingKey {
            case queued, scanning, clean, held, error
            case heldCodes = "held_codes"
        }
    }

    let id: String
    let type: String
    let name: String
    let status: String
    let removedReason: String?
    let bytes: Int64
    let scans: Scans

    enum CodingKeys: String, CodingKey {
        case id, type, name, status, bytes, scans
        case removedReason = "removed_reason"
    }

    /// What the row says. A held file wins over everything else, then
    /// scanning; a `live` repo with a stray queued scan still reads live.
    enum ScanState: Equatable { case scanning, live, held, removed, draft }

    var scanState: ScanState {
        switch status {
        case "live": return .live
        case "held": return .held
        case "removed": return .removed
        case "scanning": return .scanning
        default: return scans.queued + scans.scanning > 0 ? .scanning : .draft
        }
    }

    var scanLabel: String {
        switch scanState {
        case .scanning: return "Scanning"
        case .live: return "Live"
        case .held: return "Held"
        case .removed: return "Removed"
        case .draft: return "Draft"
        }
    }

    var detailText: String {
        switch scanState {
        case .scanning:
            return "Files are scanned before anyone can download them."
        case .live:
            return "Every file passed its scan."
        case .held:
            return "A file did not pass its scan, so nothing from this commit is served."
        case .removed:
            return "This repo was removed."
        case .draft:
            return "No files committed yet."
        }
    }
}

private struct FreeweightsMyReposResponse: Codable {
    let repos: [FreeweightsMyRepo]
}

/// The reasons the report sheet offers (the `reason_class` enum of
/// `POST /api/fw/v1/reports`; legal notices go to the takedown mailbox).
enum FreeweightsReportReason: String, CaseIterable, Identifiable, Sendable {
    case copyright
    case licenseViolation = "license_violation"
    case malwareOrSecrets = "malware_or_secrets"
    case realPersonLikeness = "real_person_likeness"
    case spam
    case abuse
    case other

    var id: String { rawValue }

    var label: String {
        switch self {
        case .copyright: return "Copyright"
        case .licenseViolation: return "License violation"
        case .malwareOrSecrets: return "Malware or leaked secrets"
        case .realPersonLikeness: return "Real person's likeness"
        case .spam: return "Spam"
        case .abuse: return "Abuse"
        case .other: return "Something else"
        }
    }
}

/// Why `myRepos()` or `report()` could not finish; each case carries
/// plain-words copy for the screen.
enum FreeweightsAccountError: Error, Equatable, LocalizedError {
    case signedOut
    case rateLimited
    case failed(Int)
    case network

    var errorDescription: String? {
        switch self {
        case .signedOut: return "Sign in to do this."
        case .rateLimited: return "Too many reports just now. Try again later."
        case .failed(let status): return "Freeweights answered \(status)."
        case .network: return "Couldn't reach Freeweights. Check your connection."
        }
    }
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

    /// `GET /api/models?search=&section=&pipeline_tag=&sort=rank&limit=&cursor=`.
    /// Always `sort=rank`: official lab repos and base models first (the
    /// worker's default with a search term, and its no-search order too).
    func search(query: String = "",
                section: String? = nil,
                pipelineTag: String? = nil,
                limit: Int = 25,
                cursor: String? = nil) async -> FreeweightsLoad<FreeweightsPage> {
        var components = URLComponents(url: baseURL.appendingPathComponent("api/models"), resolvingAgainstBaseURL: false)!
        var items = [URLQueryItem(name: "sort", value: "rank"), URLQueryItem(name: "limit", value: String(limit))]
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

    // MARK: Signed-in calls (My uploads, reports)

    /// `GET /api/fw/v1/repos` with the account's token. A 404 means no
    /// Freeweights profile yet, which is an empty list, not an error.
    func myRepos(kind: String? = nil, client auth: APIClient = .shared) async throws -> [FreeweightsMyRepo] {
        var components = URLComponents(url: baseURL.appendingPathComponent("api/fw/v1/repos"), resolvingAgainstBaseURL: false)!
        if let kind { components.queryItems = [URLQueryItem(name: "kind", value: kind)] }
        let request = try await auth.authorizedRequest(url: components.url!)
        let (data, status) = try await send(request, using: auth)
        switch status {
        case 200...299:
            do {
                return try JSONDecoder().decode(FreeweightsMyReposResponse.self, from: data).repos
            } catch {
                throw APIError.decoding(error)
            }
        case 401: throw FreeweightsAccountError.signedOut
        case 404: return []
        default: throw FreeweightsAccountError.failed(status)
        }
    }

    private struct ReportBody: Encodable {
        struct Target: Encodable { let type: String; let id: String }
        let target: Target
        let reason_class: String
        let body: String?
    }

    /// `POST /api/fw/v1/reports` for a repo (`namespace/name`).
    func report(repo id: String, reason: FreeweightsReportReason, note: String,
                client auth: APIClient = .shared) async throws {
        var request = try await auth.authorizedRequest(url: baseURL.appendingPathComponent("api/fw/v1/reports"), method: "POST")
        let trimmed = note.trimmingCharacters(in: .whitespacesAndNewlines)
        request.httpBody = try JSONEncoder().encode(ReportBody(
            target: .init(type: "repo", id: id),
            reason_class: reason.rawValue,
            body: trimmed.isEmpty ? nil : String(trimmed.prefix(5000))))
        let (_, status) = try await send(request, using: auth)
        switch status {
        case 200...299: return
        case 401: throw FreeweightsAccountError.signedOut
        case 429: throw FreeweightsAccountError.rateLimited
        default: throw FreeweightsAccountError.failed(status)
        }
    }

    private func send(_ request: URLRequest, using auth: APIClient) async throws -> (Data, Int) {
        do {
            let (data, response) = try await session.data(for: request)
            return (data, (response as? HTTPURLResponse)?.statusCode ?? 0)
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw FreeweightsAccountError.network
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
