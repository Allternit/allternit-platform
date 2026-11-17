import Combine
import Foundation

/// Which template each mode's new projects start with: a built-in id or an
/// account template id. Encodes as `{ chat, code, design, bot }`.
struct PadModeDefaults: Codable, Equatable, Sendable {
    var chat: String = PadMode.chat.defaultTemplateID
    var code: String = PadMode.code.defaultTemplateID
    var design: String = PadMode.design.defaultTemplateID
    var bot: String = PadMode.bot.defaultTemplateID

    subscript(mode: PadMode) -> String {
        get {
            switch mode {
            case .chat: return chat
            case .code: return code
            case .design: return design
            case .bot: return bot
            }
        }
        set {
            switch mode {
            case .chat: chat = newValue
            case .code: code = newValue
            case .design: design = newValue
            case .bot: bot = newValue
            }
        }
    }
}

/// The account's pad templates document on cloud-api,
/// `{ templates: [...], modeDefaults: { chat, code, design, bot } }`
/// (web `PadTemplatesDoc`).
struct PadTemplatesDoc: Encodable, Equatable, Sendable {
    /// Account templates only; built-ins live in code (`PadTemplate.builtIns`).
    var templates: [PadTemplate] = []
    var modeDefaults = PadModeDefaults()

    static let empty = PadTemplatesDoc()

    /// Whatever arrived, with unusable templates dropped and unknown defaults
    /// replaced by the mode's built-in (web `normalizeTemplatesDoc`).
    static func normalize(_ raw: Any?) -> PadTemplatesDoc {
        let r = raw as? [String: Any] ?? [:]
        var templates: [PadTemplate] = []
        for item in r["templates"] as? [Any] ?? [] {
            if let template = PadTemplate.normalize(item), !templates.contains(where: { $0.id == template.id }) {
                templates.append(template)
            }
            if templates.count >= PadLimits.templates { break }
        }
        let given = r["modeDefaults"] as? [String: Any] ?? [:]
        var modeDefaults = PadModeDefaults()
        for mode in PadMode.allCases {
            if let id = given[mode.rawValue] as? String,
               PadTemplate.isBuiltInID(id) || templates.contains(where: { $0.id == id }) {
                modeDefaults[mode] = id
            }
        }
        return PadTemplatesDoc(templates: templates, modeDefaults: modeDefaults)
    }

    /// A response body as a document. Throws when it is not JSON.
    static func decode(_ data: Data) throws -> PadTemplatesDoc {
        normalize(try JSONSerialization.jsonObject(with: data))
    }
}

/// `GET/PUT /api/v1/account/pad-templates` on cloud-api with the Clerk
/// session as Bearer (templates belong to the person, on every device, so they
/// do not go through the runtime routing in `APIClient`).
final class PadTemplatesClient: @unchecked Sendable {
    static let shared = PadTemplatesClient()
    static let path = "api/v1/account/pad-templates"
    static let loadError = "Could not load your templates"
    static let signInError = "Sign in to save templates to your account."

    struct Failure: LocalizedError, Equatable {
        let message: String
        var errorDescription: String? { message }
    }

    private let baseURL: URL
    private let session: URLSession
    private let token: @Sendable () async -> String?

    init(baseURL: URL = AppConfig.cloudAPIBaseURL,
         session: URLSession = .shared,
         token: @escaping @Sendable () async -> String? = {
             guard let token = try? await AuthManager.shared.getToken(), !token.isEmpty else { return nil }
             return token
         }) {
        self.baseURL = baseURL
        self.session = session
        self.token = token
    }

    func fetch() async throws -> PadTemplatesDoc {
        try await request(doc: nil)
    }

    /// Stores the whole document and returns what cloud-api kept.
    func store(_ doc: PadTemplatesDoc) async throws -> PadTemplatesDoc {
        try await request(doc: doc)
    }

    private func request(doc: PadTemplatesDoc?) async throws -> PadTemplatesDoc {
        var request = URLRequest(url: baseURL.appendingPathComponent(Self.path))
        request.httpMethod = doc == nil ? "GET" : "PUT"
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let doc {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(doc)
        }
        if let token = await token() {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let reply: (Data, URLResponse)
        do {
            reply = try await session.data(for: request)
        } catch {
            throw Failure(message: doc == nil ? Self.loadError : "You appear to be offline. Connect and try again.")
        }
        let (data, response) = reply
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 401 || status == 403 { throw Failure(message: Self.signInError) }
        guard (200...299).contains(status) else {
            if doc == nil { throw Failure(message: Self.loadError) }
            struct ErrorBody: Decodable { let error: String? }
            let said = (try? JSONDecoder().decode(ErrorBody.self, from: data))?.error ?? ""
            throw Failure(message: status == 400 && !said.isEmpty ? said : "Could not save your templates. Try again.")
        }
        do {
            return try PadTemplatesDoc.decode(data)
        } catch {
            throw Failure(message: doc == nil ? Self.loadError : "Could not save your templates. Try again.")
        }
    }
}

/// The person's pad templates for the screens (web `usePadTemplates`).
///
/// When the route fails, the built-in templates still work and `error` holds
/// "Could not load your templates" for the Retry line. Writes refuse until the
/// document has loaded once, so a failed load never overwrites saved templates.
@MainActor
final class PadTemplatesStore: ObservableObject {
    static let shared = PadTemplatesStore()

    /// The account's document once loaded; nil before, signed out, or after a failed load.
    @Published private(set) var doc: PadTemplatesDoc?
    @Published private(set) var loading = false
    @Published private(set) var error: String?

    private let client: PadTemplatesClient
    private let isSignedIn: @MainActor () -> Bool
    private var generation = 0
    private var writeChain: Task<Void, Never>?

    init(client: PadTemplatesClient = .shared,
         isSignedIn: @escaping @MainActor () -> Bool = { AuthManager.shared.isSignedIn }) {
        self.client = client
        self.isSignedIn = isSignedIn
    }

    var loaded: Bool { doc != nil }
    var builtInTemplates: [PadTemplate] { PadTemplate.builtIns }
    var userTemplates: [PadTemplate] { doc?.templates ?? [] }
    /// Built-ins first, then the account's own.
    var templates: [PadTemplate] { builtInTemplates + userTemplates }
    var modeDefaults: PadModeDefaults { doc?.modeDefaults ?? PadModeDefaults() }

    /// The template new projects in this mode start with.
    func defaultTemplate(for mode: PadMode) -> PadTemplate {
        let id = modeDefaults[mode]
        return templates.first { $0.id == id } ?? .research
    }

    /// Loads the document (again). Call on appear, on sign-in, and from Retry.
    @discardableResult
    func load() -> Task<Void, Never> {
        generation += 1
        let forGeneration = generation
        doc = nil
        error = nil
        guard isSignedIn() else {
            loading = false
            return Task {}
        }
        loading = true
        let client = client
        return Task { [weak self] in
            let result: Result<PadTemplatesDoc, Error>
            do { result = .success(try await client.fetch()) } catch { result = .failure(error) }
            guard let self, self.generation == forGeneration else { return }
            self.loading = false
            switch result {
            case .success(let doc): self.doc = doc
            case .failure(let error): self.error = (error as? PadTemplatesClient.Failure)?.message ?? PadTemplatesClient.loadError
            }
        }
    }

    func retry() { load() }

    /// Saves the pad's tiles and quick actions as a new account template.
    @discardableResult
    func saveTemplate(name: String, pad: ProjectPad) async throws -> PadTemplate {
        let clean = PadJSON.clip(name.trimmingCharacters(in: .whitespacesAndNewlines), PadLimits.templateName)
        guard !clean.isEmpty else { throw PadTemplatesClient.Failure(message: "Give the template a name.") }
        let template = PadTemplate(id: PadTemplate.newUserID(), name: clean,
                                   tiles: PadTile.normalize(PadJSON.object(pad.tiles)),
                                   actions: PadAction.normalize(PadJSON.object(pad.data.actions)),
                                   createdAt: ProjectPad.nowMillis())
        let stored = try await write { doc in
            guard doc.templates.count < PadLimits.templates else {
                throw PadTemplatesClient.Failure(message: "You can keep up to \(PadLimits.templates) templates. Delete one first.")
            }
            doc.templates.append(template)
        }
        return stored.templates.first { $0.id == template.id } ?? template
    }

    /// Removes an account template; modes that started with it go back to their built-in default.
    func deleteTemplate(id: String) async throws {
        try await write { doc in
            doc.templates.removeAll { $0.id == id }
            for mode in PadMode.allCases where doc.modeDefaults[mode] == id {
                doc.modeDefaults[mode] = mode.defaultTemplateID
            }
        }
    }

    /// Which template new projects in a mode start with (built-in or account template).
    func setModeDefault(_ mode: PadMode, templateID: String) async throws {
        try await write { doc in
            guard PadTemplate.isBuiltInID(templateID) || doc.templates.contains(where: { $0.id == templateID }) else {
                throw PadTemplatesClient.Failure(message: "Choose one of the templates.")
            }
            doc.modeDefaults[mode] = templateID
        }
    }

    /// Applies a change to the latest loaded document and stores it; one write at a time.
    @discardableResult
    private func write(_ change: @escaping @Sendable (inout PadTemplatesDoc) throws -> Void) async throws -> PadTemplatesDoc {
        let previous = writeChain
        let forGeneration = generation
        let client = client
        let task = Task { [weak self] () -> Result<PadTemplatesDoc, Error> in
            await previous?.value
            guard let self else { return .failure(CancellationError()) }
            guard self.isSignedIn() else { return .failure(PadTemplatesClient.Failure(message: PadTemplatesClient.signInError)) }
            guard var next = self.doc else {
                return .failure(PadTemplatesClient.Failure(message: "\(PadTemplatesClient.loadError). Retry, then save again."))
            }
            do {
                try change(&next)
                let stored = try await client.store(next)
                guard self.generation == forGeneration else {
                    return .failure(PadTemplatesClient.Failure(message: "The account changed. Try again."))
                }
                self.doc = stored
                self.error = nil
                return .success(stored)
            } catch {
                return .failure(error)
            }
        }
        writeChain = Task { _ = await task.value }
        return try await task.value.get()
    }
}
