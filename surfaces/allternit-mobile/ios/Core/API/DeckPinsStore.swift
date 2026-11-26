import Foundation

/// An artifact pinned to the composer deck's Artifacts menu (web
/// src/lib/artifacts/deck-pins.ts `DeckPin`).
struct DeckPin: Codable, Equatable, Identifiable, Sendable {
    let id: String
    let kind: String
    let title: String
}

/// Pins for the deck's Artifacts ▾ Pinned section. Same rules as the web:
/// newest first, at most 8, plans are never pinned. Kept per user on the
/// runtime (`GET/PUT /deck-pins`, through `APIClient.shared` so direct and
/// relay routing apply); a runtime without the route (404), a signed-out or
/// refused call (401/403) or no paired computer (503) falls back to this
/// device's UserDefaults.
@MainActor
final class DeckPinsStore: ObservableObject {
    static let shared = DeckPinsStore()

    static let maxPins = 8
    static let defaultsKey = "allternit:deck-pins"

    @Published private(set) var pins: [DeckPin]

    private enum Mode { case runtime, local }
    private var mode: Mode = .local
    private let client: APIClient
    private let defaults: UserDefaults
    private var ticket = 0
    private var loading: Task<Void, Never>?

    init(client: APIClient = .shared, defaults: UserDefaults = .standard) {
        self.client = client
        self.defaults = defaults
        self.pins = Self.readLocal(defaults)
    }

    // MARK: Pure rules (mirror deck-pins.ts)

    static func normalize(_ raw: [DeckPin]) -> [DeckPin] {
        var seen = Set<String>()
        var out: [DeckPin] = []
        for p in raw {
            if p.id.isEmpty || seen.contains(p.id) || p.kind == "plan" { continue }
            seen.insert(p.id)
            out.append(DeckPin(id: p.id, kind: p.kind.isEmpty ? "doc" : p.kind, title: p.title.isEmpty ? "Untitled" : p.title))
            if out.count >= maxPins { break }
        }
        return out
    }

    /// Pins (or re-pins) to the front; the oldest drops off past the cap.
    static func withPin(_ pins: [DeckPin], _ pin: DeckPin) -> [DeckPin] {
        if pin.kind == "plan" { return pins }
        return normalize([pin] + pins.filter { $0.id != pin.id })
    }

    static func withoutPin(_ pins: [DeckPin], id: String) -> [DeckPin] {
        pins.filter { $0.id != id }
    }

    // MARK: Public API (the card ⋯ menu in 5.5 calls pin / unpin / toggle)

    func isPinned(_ id: String) -> Bool { pins.contains { $0.id == id } }

    func pin(id: String, kind: String, title: String) async {
        await settled()
        await save(Self.withPin(pins, DeckPin(id: id, kind: kind, title: title)))
    }

    func unpin(id: String) async {
        await settled()
        await save(Self.withoutPin(pins, id: id))
    }

    func toggle(id: String, kind: String, title: String) async {
        if isPinned(id) { await unpin(id: id) } else { await pin(id: id, kind: kind, title: title) }
    }

    /// Drops pins whose artifact no longer exists.
    func prune(missing ids: [String]) async {
        await settled()
        guard pins.contains(where: { ids.contains($0.id) }) else { return }
        await save(pins.filter { !ids.contains($0.id) })
    }

    /// Reads the runtime copy (or the local one when the runtime can't keep pins).
    func load() async {
        if let loading { return await loading.value }
        ticket += 1
        let mine = ticket
        let work = Task { [weak self] in
            guard let self else { return }
            do {
                let fetched = try await self.fetchRemote()
                guard mine == self.ticket else { return }
                self.mode = .runtime
                self.pins = fetched
            } catch {
                guard mine == self.ticket else { return }
                self.mode = .local
                self.pins = Self.readLocal(self.defaults)
            }
        }
        loading = work
        await work.value
        loading = nil
    }

    // MARK: Storage

    private func settled() async {
        if let loading { await loading.value }
    }

    private func save(_ next: [DeckPin]) async {
        let previous = pins
        ticket += 1 // a load still in flight must not overwrite this change
        pins = next
        if mode == .local {
            Self.writeLocal(next, defaults)
            return
        }
        do {
            try await putRemote(next)
        } catch let APIError.httpError(status, _) where [404, 401, 403, 503].contains(status) {
            mode = .local
            Self.writeLocal(next, defaults)
        } catch {
            pins = previous // couldn't save: keep what the runtime has
        }
    }

    private struct Body: Codable { let pins: [DeckPin]? }

    private func fetchRemote() async throws -> [DeckPin] {
        let body: Body = try await client.get(path: "deck-pins")
        return Self.normalize(body.pins ?? [])
    }

    private func putRemote(_ next: [DeckPin]) async throws {
        try await client.put(path: "deck-pins", body: Body(pins: next))
    }

    private static func readLocal(_ defaults: UserDefaults) -> [DeckPin] {
        guard let data = defaults.data(forKey: defaultsKey),
              let decoded = try? JSONDecoder().decode([DeckPin].self, from: data) else { return [] }
        return normalize(decoded)
    }

    private static func writeLocal(_ pins: [DeckPin], _ defaults: UserDefaults) {
        if let data = try? JSONEncoder().encode(pins) { defaults.set(data, forKey: defaultsKey) }
    }
}
