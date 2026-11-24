import Foundation

// Checks a voice key with its vendor and lists ElevenLabs voices. Calls go
// from this phone straight to the vendor (api.openai.com, api.elevenlabs.io),
// never through Allternit. Same rules as the web's `vendors.ts` (plan
// snoopy-napping-rabbit, decision 6):
//
// - OpenAI: `GET /v1/models` with `Authorization: Bearer`.
// - ElevenLabs: `GET /v1/user` with `xi-api-key`; the label is the plan and
//   the characters left. A key without `user_read` (403, or a
//   `missing_permissions` body) is tried on `GET /v1/voices`; if that works
//   the key is "restricted": it speaks, it just can't read the account.
// - 401 means the vendor rejected the key.

/// What a key check found.
enum VoiceKeyCheck: Equatable, Sendable {
    /// The key works; `label` describes it ("Creator plan · 9,500 characters left").
    case ok(label: String)
    /// The key works but can't read everything ("restricted key").
    case restricted(label: String)
    /// The vendor refused the key.
    case rejected(message: String)
    /// The vendor couldn't be reached, or answered with a server error.
    case network(message: String)

    /// The label to save with a working key; nil when the key didn't work.
    var savedLabel: String? {
        switch self {
        case .ok(let label), .restricted(let label): return label
        case .rejected, .network: return nil
        }
    }
}

/// One ElevenLabs voice from `GET /v1/voices`.
struct ElevenLabsVoice: Decodable, Identifiable, Equatable, Sendable {
    let voiceId: String
    let name: String
    let category: String?
    let previewURL: URL?

    var id: String { voiceId }

    private enum CodingKeys: String, CodingKey {
        case voiceId = "voice_id", name, category, previewURL = "preview_url"
    }

    init(voiceId: String, name: String, category: String? = nil, previewURL: URL? = nil) {
        self.voiceId = voiceId
        self.name = name
        self.category = category
        self.previewURL = previewURL
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        voiceId = try c.decode(String.self, forKey: .voiceId)
        name = (try? c.decode(String.self, forKey: .name)) ?? voiceId
        category = try? c.decodeIfPresent(String.self, forKey: .category)
        // An empty or malformed preview_url is just "no preview".
        previewURL = (try? c.decodeIfPresent(String.self, forKey: .previewURL)).flatMap { $0.flatMap(URL.init(string:)) }
    }
}

struct VoiceVendorClient: Sendable {
    private let session: URLSession
    private let openAIBase: URL
    private let elevenLabsBase: URL

    static let openAIDefaultBase = URL(string: "https://api.openai.com")!
    static let elevenLabsDefaultBase = URL(string: "https://api.elevenlabs.io")!

    init(session: URLSession = .shared) {
        self.session = session
        #if DEBUG
        // `-voice-vendor-url <url>` (DEBUG only): a fake vendor serving both
        // vendors' /v1 routes (allternit-ai scripts/fake-voice-vendor.mjs).
        if let override = UserDefaults.standard.string(forKey: "voice-vendor-url"), let url = URL(string: override) {
            self.openAIBase = url
            self.elevenLabsBase = url
            return
        }
        #endif
        self.openAIBase = Self.openAIDefaultBase
        self.elevenLabsBase = Self.elevenLabsDefaultBase
    }

    #if DEBUG
    /// Tests and the fake vendor only: Release builds always call the real vendors.
    init(session: URLSession, openAIBase: URL, elevenLabsBase: URL) {
        self.session = session
        self.openAIBase = openAIBase
        self.elevenLabsBase = elevenLabsBase
    }
    #endif

    // MARK: Key check

    func checkKey(_ vendor: VoiceVendor, key rawKey: String) async -> VoiceKeyCheck {
        let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else { return .rejected(message: "Paste your \(vendor.name) key first.") }
        switch vendor {
        case .openai: return await checkOpenAI(key)
        case .elevenlabs: return await checkElevenLabs(key)
        }
    }

    private func checkOpenAI(_ key: String) async -> VoiceKeyCheck {
        let result = await get(openAIBase, "v1/models", vendor: .openai, key: key)
        guard case .success(let (status, _)) = result else { return Self.unreachable(.openai) }
        switch status {
        case 200..<300:
            return .ok(label: VoiceProviderKeys.masked(key))
        case 401:
            return Self.rejected(.openai)
        case 403:
            // A restricted key without "Models: read" still speaks if it can
            // reach audio; OpenAI answers 403 (insufficient permissions).
            return .restricted(label: "restricted key")
        case 429:
            // A real key that's out of credit or rate limited.
            return .ok(label: "\(VoiceProviderKeys.masked(key)) · check your OpenAI credit")
        default:
            return .network(message: "OpenAI could not check the key right now (error \(status)). Try again.")
        }
    }

    private func checkElevenLabs(_ key: String) async -> VoiceKeyCheck {
        let result = await get(elevenLabsBase, "v1/user", vendor: .elevenlabs, key: key)
        guard case .success(let (status, body)) = result else { return Self.unreachable(.elevenlabs) }
        if (200..<300).contains(status) {
            return .ok(label: Self.elevenLabsLabel(body) ?? "ElevenLabs key")
        }
        if status == 403 || Self.isMissingPermissions(body) {
            let voices = await get(elevenLabsBase, "v1/voices", vendor: .elevenlabs, key: key)
            guard case .success(let (voicesStatus, voicesBody)) = voices else { return Self.unreachable(.elevenlabs) }
            if (200..<300).contains(voicesStatus) || voicesStatus == 403 || Self.isMissingPermissions(voicesBody) {
                return .restricted(label: "restricted key")
            }
            return voicesStatus == 401 ? Self.rejected(.elevenlabs)
                : .network(message: "ElevenLabs could not check the key right now (error \(voicesStatus)). Try again.")
        }
        if status == 401 { return Self.rejected(.elevenlabs) }
        return .network(message: "ElevenLabs could not check the key right now (error \(status)). Try again.")
    }

    // MARK: Voices

    struct VoicesError: Error, LocalizedError, Sendable {
        let message: String
        var errorDescription: String? { message }
    }

    /// The voices on the user's ElevenLabs account (premade and their own).
    func listVoices(key: String) async throws -> [ElevenLabsVoice] {
        let result = await get(elevenLabsBase, "v1/voices", vendor: .elevenlabs, key: key)
        guard case .success(let (status, body)) = result else {
            throw VoicesError(message: "ElevenLabs could not be reached. Check your connection.")
        }
        guard (200..<300).contains(status) else {
            if status == 403 || Self.isMissingPermissions(body) {
                throw VoicesError(message: "This key can't list voices. Enter a voice ID instead, or give the key the Voices permission.")
            }
            throw VoicesError(message: "ElevenLabs didn't list your voices (error \(status)).")
        }
        struct Page: Decodable { let voices: [ElevenLabsVoice] }
        do {
            return try JSONDecoder().decode(Page.self, from: body).voices
        } catch {
            throw VoicesError(message: "ElevenLabs sent a voice list this app couldn't read.")
        }
    }

    // MARK: Helpers

    private func get(_ base: URL, _ path: String, vendor: VoiceVendor, key: String) async -> Result<(Int, Data), Error> {
        var request = URLRequest(url: base.appendingPathComponent(path))
        request.httpMethod = "GET"
        request.timeoutInterval = 15
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        switch vendor {
        case .openai: request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        case .elevenlabs: request.setValue(key, forHTTPHeaderField: "xi-api-key")
        }
        do {
            let (data, response) = try await session.data(for: request)
            return .success(((response as? HTTPURLResponse)?.statusCode ?? 0, data))
        } catch {
            return .failure(error)
        }
    }

    private static func rejected(_ vendor: VoiceVendor) -> VoiceKeyCheck {
        .rejected(message: "\(vendor.name) didn't accept this key. Copy it again from your \(vendor.name) account.")
    }

    private static func unreachable(_ vendor: VoiceVendor) -> VoiceKeyCheck {
        .network(message: "\(vendor.name) couldn't be reached to check the key. Check your connection.")
    }

    /// ElevenLabs reports a key without a permission as
    /// `{"detail":{"status":"missing_permissions", …}}`.
    private static func isMissingPermissions(_ body: Data) -> Bool {
        guard let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else { return false }
        if let detail = object["detail"] as? [String: Any], let status = detail["status"] as? String {
            return status == "missing_permissions"
        }
        return (object["status"] as? String) == "missing_permissions"
    }

    /// "Creator plan · 9,500 characters left" from `GET /v1/user`.
    static func elevenLabsLabel(_ body: Data) -> String? {
        guard let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any],
              let subscription = object["subscription"] as? [String: Any] else { return nil }
        var parts: [String] = []
        if let tier = subscription["tier"] as? String, !tier.isEmpty {
            parts.append("\(tier.prefix(1).uppercased())\(tier.dropFirst()) plan")
        }
        if let used = (subscription["character_count"] as? NSNumber)?.intValue,
           let limit = (subscription["character_limit"] as? NSNumber)?.intValue {
            let left = max(0, limit - used)
            let formatter = NumberFormatter()
            formatter.numberStyle = .decimal
            formatter.locale = Locale(identifier: "en_US")
            parts.append("\(formatter.string(from: NSNumber(value: left)) ?? String(left)) characters left")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}
