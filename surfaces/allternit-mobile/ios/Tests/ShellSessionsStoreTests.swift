import XCTest
@testable import Allternit

@MainActor
final class ShellSessionsStoreTests: XCTestCase {
    private func session(_ id: String, updated: String) throws -> AgentSession {
        let json = #"{"id":"\#(id)","name":"\#(id)","created_at":"\#(updated)","updated_at":"\#(updated)"}"#
        return try JSONDecoder().decode(AgentSession.self, from: Data(json.utf8))
    }

    private func store(
        computer: @escaping @Sendable () async throws -> [AgentSession],
        plans: @escaping @Sendable () async throws -> [AgentSession]
    ) -> ShellSessionsStore {
        ShellSessionsStore(fetchComputerSessions: computer, fetchPlanThreads: plans, fetchPlaces: { [:] })
    }

    func testNoComputerStillShowsPlanChats() async throws {
        let plan = try session("pth_1", updated: "2026-11-15T10:00:00Z")
        let s = store(computer: { throw APIError.httpError(statusCode: 428, message: "no computer") },
                      plans: { [plan] })
        await s.load()
        XCTAssertNil(s.loadError)
        XCTAssertTrue(s.hasLoaded)
        XCTAssertEqual(s.sessions.map(\.id), ["pth_1"])
    }

    func testNoComputerAndNoPlanChatsIsTheEmptyStateNotAnError() async {
        let s = store(computer: { throw APIError.httpError(statusCode: 428, message: nil) }, plans: { [] })
        await s.load()
        XCTAssertNil(s.loadError)
        XCTAssertTrue(s.hasLoaded)
        XCTAssertTrue(s.sessions.isEmpty)
    }

    func testMergesBothSortedNewestFirst() async throws {
        let a = try session("ses_a", updated: "2026-11-15T09:00:00Z")
        let p = try session("pth_p", updated: "2026-11-15T11:00:00Z")
        let s = store(computer: { [a] }, plans: { [p] })
        await s.load()
        XCTAssertNil(s.loadError)
        XCTAssertEqual(s.sessions.map(\.id), ["pth_p", "ses_a"])
    }

    func testPlanStoreFailureLeavesComputerSessions() async throws {
        let a = try session("ses_a", updated: "2026-11-15T09:00:00Z")
        let s = store(computer: { [a] }, plans: { throw APIError.httpError(statusCode: 500, message: nil) })
        await s.load()
        XCTAssertNil(s.loadError)
        XCTAssertEqual(s.sessions.map(\.id), ["ses_a"])
    }

    func testOtherComputerFailureWithPlanChatsKeepsPlanChats() async throws {
        let plan = try session("pth_1", updated: "2026-11-15T10:00:00Z")
        let s = store(computer: { throw APIError.httpError(statusCode: 500, message: "boom") }, plans: { [plan] })
        await s.load()
        XCTAssertNil(s.loadError)
        XCTAssertEqual(s.sessions.map(\.id), ["pth_1"])
    }

    func testOtherComputerFailureWithNoPlanChatsShowsError() async {
        let s = store(computer: { throw APIError.httpError(statusCode: 500, message: "boom") }, plans: { [] })
        await s.load()
        XCTAssertEqual(s.loadError, "boom")
        XCTAssertFalse(s.hasLoaded)
    }

    func testBothFailingShowsError() async {
        let s = store(computer: { throw APIError.httpError(statusCode: 428, message: "no computer") },
                      plans: { throw APIError.httpError(statusCode: 500, message: "plan down") })
        await s.load()
        XCTAssertNotNil(s.loadError)
        XCTAssertFalse(s.hasLoaded)
    }

    func testLaunchGreetingBucketChangesWithTimeOfDay() {
        var cal = Calendar(identifier: .gregorian)
        cal.timeZone = TimeZone(identifier: "UTC")!
        func at(_ iso: String) -> Date { ISO8601DateFormatter().date(from: iso)! }
        let night = ChatLaunchGreeting.titleBucket(at("2026-11-15T03:00:00Z"), calendar: cal)
        let morning = ChatLaunchGreeting.titleBucket(at("2026-11-15T08:44:00Z"), calendar: cal)
        let later = ChatLaunchGreeting.titleBucket(at("2026-11-15T09:30:00Z"), calendar: cal)
        let nextDay = ChatLaunchGreeting.titleBucket(at("2026-11-16T08:44:00Z"), calendar: cal)
        XCTAssertNotEqual(night, morning)
        XCTAssertEqual(morning, later)
        XCTAssertNotEqual(morning, nextDay)
    }
}
