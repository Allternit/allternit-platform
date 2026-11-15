import Foundation

// "Save to my Allternit account" for the user's own voice keys (Settings ›
// Voice), off by default. When on, keys are also kept, encrypted, in the
// user's account (cloud-api `/api/v1/voice/keys`, migration 128) so their
// other signed-in devices can use them. Rules (plan snoopy-napping-rabbit,
// decisions 4 and 5):
//
// - The toggle is account-wide: a pull that finds saved rows shows it ON on
//   this phone, a pull that finds none shows it OFF (turning it on with no
//   keys anywhere therefore flips back off on the next pull).
// - The copy on this phone is the working copy. On Voice page open and app
//   foreground the newer `updated_at` wins, per vendor.
// - A change on this phone while sync is on is sent up (PUT).
// - A pull never changes Where voice runs.
// - Disconnect removes the key from this phone only and opts that vendor
//   out, so a pull won't bring it back. Connecting again clears it.
// - Turning sync off deletes every saved row from the account; keys on this
//   phone stay.
//
// Session auth only: the routes refuse API tokens, so requests go straight
// to cloud-api with the Clerk Bearer (`APIClient.authorizedRequest`), never
// through a runtime relay.

/// One saved row from `GET /api/v1/voice/keys`.
struct VoiceKeyRemote: Decodable, Equatable, Sendable {
    let provider: String
    let apiKey: String
    let voiceId: String?
    let label: String?
    let updatedAt: Date

    var vendor: VoiceVendor? { VoiceVendor(rawValue: provider) }

    private enum CodingKeys: String, CodingKey {
        case provider, apiKey = "api_key", voiceId = "voice_id", label, updatedAt = "updated_at"
    }

    init(provider: String, apiKey: String, voiceId: String? = nil, label: String? = nil, updatedAt: Date) {
        self.provider = provider
        self.apiKey = apiKey
        self.voiceId = voiceId
        self.label = label
        self.updatedAt = updatedAt
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        provider = try c.decode(String.self, forKey: .provider)
        apiKey = try c.decode(String.self, forKey: .apiKey)
        voiceId = try c.decodeIfPresent(String.self, forKey: .voiceId)
        label = try c.decodeIfPresent(String.self, forKey: .label)
        let stamp = try c.decode(String.self, forKey: .updatedAt)
        guard let date = VoiceKeyRemote.parseDate(stamp) else {
            throw DecodingError.dataCorruptedError(forKey: .updatedAt, in: c, debugDescription: "updated_at is not RFC 3339")
        }
        updatedAt = date
    }

    /// RFC 3339 as chrono writes it ("2026-11-13T09:12:33.123456Z" or
    /// "+00:00"), with any number of fractional digits.
    static func parseDate(_ text: String) -> Date? {
        var base = text
        var fraction = 0.0
        if let dot = text.firstIndex(of: ".") {
            let digitsEnd = text[text.index(after: dot)...].firstIndex { !$0.isNumber } ?? text.endIndex
            fraction = Double("0" + text[dot..<digitsEnd]) ?? 0
            base = String(text[..<dot]) + String(text[digitsEnd...])
        }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: base).map { $0.addingTimeInterval(fraction) }
    }
}

/// What one sync does: rows to copy onto this phone and vendors to send up.
struct VoiceKeyMergePlan: Equatable, Sendable {
    var pull: [VoiceKeyRemote] = []
    var push: [VoiceVendor] = []
}

enum VoiceKeyMerge {
    /// Newer wins, per vendor. A vendor this phone opted out of (Disconnect)
    /// is left alone both ways; a key saved before sync existed (no change
    /// time) loses to the account's copy.
    static func plan(local: [VoiceVendor: VoiceKeyLocal], remote: [VoiceKeyRemote], optedOut: Set<VoiceVendor>) -> VoiceKeyMergePlan {
        var plan = VoiceKeyMergePlan()
        let remoteByVendor = Dictionary(remote.compactMap { row in row.vendor.map { ($0, row) } },
                                        uniquingKeysWith: { a, b in a.updatedAt >= b.updatedAt ? a : b })
        for vendor in VoiceVendor.allCases where !optedOut.contains(vendor) {
            switch (local[vendor], remoteByVendor[vendor]) {
            case (nil, nil):
                continue
            case (nil, let row?):
                plan.pull.append(row)
            case (_?, nil):
                plan.push.append(vendor)
            case (let mine?, let row?):
                let sameVoice = vendor != .elevenlabs || (mine.voiceId ?? "") == (row.voiceId ?? "")
                if mine.apiKey == row.apiKey && sameVoice { continue }
                let mineAt = mine.updatedAt ?? .distantPast
                if mineAt > row.updatedAt {
                    plan.push.append(vendor)
                } else if row.updatedAt > mineAt {
                    plan.pull.append(row)
                }
            }
        }
        return plan
    }
}

/// The voice keys on this phone, for Settings › Voice, plus account sync.
@MainActor
final class VoiceKeySync: ObservableObject {
    static let shared = VoiceKeySync()

    private static let enabledKey = "voice.byok.accountSync"

    /// "Save to my Allternit account".
    @Published private(set) var isEnabled: Bool
    @Published private(set) var isBusy = false
    /// The last sync problem, shown under the toggle; nil when fine.
    @Published private(set) var lastError: String?
    /// Bumped whenever keys on this phone change, so views re-read them.
    @Published private(set) var revision = 0

    private var syncTask: Task<Void, Never>?

    private init() {
        isEnabled = UserDefaults.standard.bool(forKey: Self.enabledKey)
    }

    // MARK: Local changes

    /// Keys on this phone changed (Connect, handoff, voice pick): refresh
    /// views and, with sync on, send that vendor up.
    func localChanged(_ vendor: VoiceVendor) {
        revision += 1
        guard isEnabled else { return }
        Task { await self.push(vendor) }
    }

    /// Disconnect: this phone only; the account copy stays.
    func disconnect(_ vendor: VoiceVendor) {
        VoiceProviderKeys.disconnect(vendor)
        revision += 1
    }

    // MARK: Toggle

    /// The toggle is account-wide (same rule as the web): it reads ON while
    /// the account holds saved keys and OFF when it holds none.
    func setEnabled(_ on: Bool) async {
        guard on != isEnabled else { return }
        lastError = nil
        isBusy = true
        defer { isBusy = false }
        if on {
            setStored(true)
            // Keys from older builds count as this phone's current copy.
            for vendor in VoiceVendor.allCases { VoiceProviderKeys.stampIfNeeded(vendor) }
            do {
                for vendor in VoiceVendor.allCases where !VoiceProviderKeys.isOptedOut(vendor) {
                    try await pushOrThrow(vendor)
                }
                // Picks up keys other devices saved. With nothing saved
                // anywhere, the account holds no rows and this turns it off.
                try await pullAndMerge()
                if !isEnabled { lastError = "Connect a key first. There's nothing to save to your account yet." }
            } catch {
                setStored(false)
                lastError = "Couldn't save your keys to your Allternit account: \(Self.describe(error))"
            }
        } else {
            do {
                try await send("DELETE", path: "api/v1/voice/keys", body: nil)
                setStored(false)
            } catch {
                lastError = "Couldn't delete your keys from your Allternit account: \(Self.describe(error)) Try again."
            }
        }
    }

    private func setStored(_ on: Bool) {
        isEnabled = on
        UserDefaults.standard.set(on, forKey: Self.enabledKey)
    }

    // MARK: Sync

    /// Pull once and merge (Voice page open, app foreground), when signed in.
    func sync() async {
        guard AuthManager.shared.isSignedIn else { return }
        if let syncTask { return await syncTask.value }
        let task = Task { await self.runSync() }
        syncTask = task
        await task.value
        syncTask = nil
    }

    private func runSync() async {
        do {
            try await pullAndMerge()
            lastError = nil
        } catch {
            // A background pull only reports problems while the toggle is on.
            guard isEnabled else { return }
            if error is DecodingError {
                lastError = "Your Allternit account sent keys this app couldn't read."
            } else {
                lastError = "Couldn't sync with your Allternit account: \(Self.describe(error))"
            }
        }
    }

    /// GET the account's rows: none → the toggle reads OFF; some → ON, and
    /// the newer copy of each vendor wins.
    private func pullAndMerge() async throws {
        let (data, _) = try await send("GET", path: "api/v1/voice/keys", body: nil)
        struct List: Decodable { let keys: [VoiceKeyRemote] }
        let remote = try JSONDecoder().decode(List.self, from: data).keys
        guard !remote.isEmpty else {
            setStored(false)
            return
        }
        setStored(true)
        var local: [VoiceVendor: VoiceKeyLocal] = [:]
        var optedOut: Set<VoiceVendor> = []
        for vendor in VoiceVendor.allCases {
            local[vendor] = VoiceProviderKeys.local(vendor)
            if VoiceProviderKeys.isOptedOut(vendor) { optedOut.insert(vendor) }
        }
        let plan = VoiceKeyMerge.plan(local: local, remote: remote, optedOut: optedOut)
        for row in plan.pull {
            guard let vendor = row.vendor else { continue }
            // Never touches Where voice runs.
            VoiceProviderKeys.store(vendor, apiKey: row.apiKey, voiceId: row.voiceId,
                                    label: row.label, updatedAt: row.updatedAt)
        }
        if !plan.pull.isEmpty { revision += 1 }
        for vendor in plan.push { try await pushOrThrow(vendor) }
    }

    private func push(_ vendor: VoiceVendor) async {
        do {
            try await pushOrThrow(vendor)
        } catch {
            lastError = "Couldn't save your \(vendor.name) key to your Allternit account: \(Self.describe(error))"
        }
    }

    private func pushOrThrow(_ vendor: VoiceVendor) async throws {
        guard let mine = VoiceProviderKeys.local(vendor) else { return }
        var body: [String: String] = ["api_key": mine.apiKey]
        if let voiceId = mine.voiceId { body["voice_id"] = voiceId }
        if let label = mine.label { body["label"] = String(label.prefix(128)) }
        let (data, _) = try await send("PUT", path: "api/v1/voice/keys/\(vendor.rawValue)",
                                       body: try JSONSerialization.data(withJSONObject: body))
        // Adopt the server's change time so the next sync sees them equal.
        if let row = try? JSONDecoder().decode(VoiceKeyRemote.self, from: data),
           VoiceProviderKeys.value(vendor.keyField) == mine.apiKey {
            VoiceProviderKeys.store(vendor, apiKey: mine.apiKey, voiceId: mine.voiceId,
                                    label: mine.label, updatedAt: row.updatedAt)
        }
        lastError = nil
    }

    // MARK: Network

    @discardableResult
    private func send(_ method: String, path: String, body: Data?) async throws -> (Data, URLResponse) {
        let client = APIClient.shared
        var request = try await client.authorizedRequest(url: AppConfig.cloudAPIBaseURL.appendingPathComponent(path), method: method)
        request.httpBody = body
        request.cachePolicy = .reloadIgnoringLocalCacheData
        let (data, response) = try await client.session.data(for: request)
        try client.validate(response, data: data)
        return (data, response)
    }

    static func describe(_ error: Error) -> String {
        if case APIError.httpError(let status, _) = error {
            switch status {
            case 401: return "sign in to Allternit on this phone."
            case 403: return "your Allternit session can't use this. Sign in again."
            default: return "the server answered \(status)."
            }
        }
        if error is URLError { return "no connection." }
        return error.localizedDescription
    }
}
