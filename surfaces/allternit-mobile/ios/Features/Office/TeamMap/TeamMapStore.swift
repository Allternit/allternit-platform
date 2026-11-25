import Foundation

/// Loads `GET /team/map` for the sheet and applies the writes the detail
/// screens make (levels, reporting line, rules, asks), then reloads so the
/// server's rule text and states are what shows.
@MainActor
final class TeamMapStore: ObservableObject {
    enum Phase: Equatable {
        case loading
        case loaded
        case failed(TeamMapError)
    }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var map: TeamMap?

    private let client: TeamMapClient

    init(client: TeamMapClient = .shared) {
        self.client = client
    }

    func load() async {
        guard AuthManager.shared.isSignedIn else {
            phase = .failed(.signedOut)
            return
        }
        if map == nil { phase = .loading }
        do {
            map = try await client.map()
            phase = .loaded
        } catch is CancellationError {
            return
        } catch let error as TeamMapError {
            // Keep a map already on screen; the next pull retries.
            if map == nil { phase = .failed(error) }
        } catch {
            if map == nil { phase = .failed(.storeUnavailable) }
        }
    }

    // MARK: Writes (each returns nil on success, else the sentence to show)

    func setLevel(_ grant: TeamGrant, to level: String) async -> String? {
        await write { _ = try await self.client.setLevel(grant, level: level) }
    }

    func setReportsTo(userId: String, managerUserId: String?) async -> String? {
        await write { try await self.client.setReportsTo(userId: userId, managerUserId: managerUserId) }
    }

    func createRule(_ input: TeamRuleInput) async -> String? {
        await write { try await self.client.createRule(input) }
    }

    func deleteRule(id: String) async -> String? {
        await write { try await self.client.deleteRule(id: id) }
    }

    func decide(_ ask: TeamAsk, approve: Bool) async -> String? {
        await write { try await self.client.resolve(ask, approve: approve) }
    }

    private func write(_ body: () async throws -> Void) async -> String? {
        do {
            try await body()
            await load()
            return nil
        } catch {
            return TeamMapError.text(error)
        }
    }
}
