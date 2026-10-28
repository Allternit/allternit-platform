import Foundation

/// One pinned drawer entry — same shape as the web's `PinnedRailEntry`
/// (allternit-ai src/lib/rail-pins.ts), `pinnedAt` in epoch milliseconds.
struct PinnedRailEntry: Codable, Equatable, Sendable {
    let id: String
    let kind: String
    let mode: String
    let pinnedAt: Double
}

/// Pinned recents for the drawer. Local to this device, like the web's
/// localStorage pins (same key, so the contract stays recognizable).
@MainActor
final class RailPinsStore: ObservableObject {
    static let shared = RailPinsStore()
    static let storageKey = "allternit:rail:pinned"

    @Published private(set) var pins: [PinnedRailEntry]

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        if let data = defaults.data(forKey: Self.storageKey),
           let decoded = try? JSONDecoder().decode([PinnedRailEntry].self, from: data) {
            pins = decoded
        } else {
            pins = []
        }
    }

    func isPinned(_ id: String) -> Bool {
        pins.contains { $0.id == id }
    }

    /// Pin or unpin; returns whether the entry is pinned afterwards.
    @discardableResult
    func toggle(id: String, kind: String, mode: String) -> Bool {
        if isPinned(id) {
            pins.removeAll { $0.id == id }
        } else {
            pins.append(PinnedRailEntry(id: id, kind: kind, mode: mode, pinnedAt: Date().timeIntervalSince1970 * 1000))
        }
        save()
        return isPinned(id)
    }

    /// Drops a pin (e.g. after its session is deleted).
    func remove(id: String) {
        guard isPinned(id) else { return }
        pins.removeAll { $0.id == id }
        save()
    }

    private func save() {
        if let data = try? JSONEncoder().encode(pins) {
            defaults.set(data, forKey: Self.storageKey)
        }
    }
}
