import Foundation

/// The latest phone task started from Messages, written by the app into the
/// App Group so the iMessage app can show its status card. The extension's
/// read token only reads the status summary, so the app (signed in) starts
/// the task with `source: "imessage_ext"` and keeps this record current.
struct SharedPhoneRun: Codable, Equatable, Sendable {
    var runId: String?
    var goal: String
    /// A `phone_task` card status, or `starting`.
    var status: String
    var step: Int
    var phoneName: String
    var updatedAt: Date
    /// `refused` · `quota` · `noPhone` · `offline` · `signedOut` · `failed`
    var problem: String?
    var message: String?

    private static let key = "surfaces.phone.lastRun"

    static func load() -> SharedPhoneRun? {
        guard let data = AppGroup.defaults?.data(forKey: key) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try? decoder.decode(SharedPhoneRun.self, from: data)
    }

    func save() {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        AppGroup.defaults?.set(try? encoder.encode(self), forKey: Self.key)
    }
}
