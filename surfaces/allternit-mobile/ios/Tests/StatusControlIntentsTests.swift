import XCTest
@testable import Allternit

final class StatusControlIntentsTests: XCTestCase {
    func testScheduleMapping() throws {
        XCTAssertEqual(try RoutineSchedule.cron(kind: .daily, hour: 9, minute: 5, everyHours: nil), "5 9 * * *")
        XCTAssertEqual(try RoutineSchedule.cron(kind: .weekdays, hour: 18, minute: 0, everyHours: nil), "0 18 * * 1-5")
        XCTAssertEqual(try RoutineSchedule.cron(kind: .everyFewHours, hour: nil, minute: nil, everyHours: 2), "0 0-23/2 * * *")
        XCTAssertThrowsError(try RoutineSchedule.cron(kind: .everyFewHours, hour: nil, minute: nil, everyHours: 5))
        XCTAssertThrowsError(try RoutineSchedule.cron(kind: .daily, hour: nil, minute: nil, everyHours: nil))
        XCTAssertThrowsError(try RoutineSchedule.cron(kind: .daily, hour: 25, minute: 0, everyHours: nil))
    }

    func testScheduleRoundTripsThroughTheEditorRule() throws {
        for (kind, hour, every) in [(RoutineRepeatChoice.daily, 7, 0), (.weekdays, 22, 0), (.everyFewHours, 0, 3)] {
            let cron = try RoutineSchedule.cron(kind: kind, hour: hour, minute: 30, everyHours: every)
            XCTAssertNotNil(RoutineRule(cron: cron), cron)
        }
    }

    func testFocusRequestBody() throws {
        var open = FocusFilterRequest.make(allowedBotIds: [], allowApprovals: true)
        open.deviceId = "d1"
        XCTAssertFalse(open.active)
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: FocusFilterClient.body(open)) as? [String: Any])
        XCTAssertTrue(json["allowedBotIds"] is NSNull)
        XCTAssertEqual(json["allowApprovals"] as? Bool, true)

        var limited = FocusFilterRequest.make(allowedBotIds: ["b1", "b2"], allowApprovals: false)
        limited.deviceId = "d1"
        XCTAssertTrue(limited.active)
        let j2 = try XCTUnwrap(JSONSerialization.jsonObject(with: FocusFilterClient.body(limited)) as? [String: Any])
        XCTAssertEqual(j2["allowedBotIds"] as? [String], ["b1", "b2"])
        XCTAssertEqual(j2["deviceId"] as? String, "d1")
        XCTAssertEqual(j2["allowApprovals"] as? Bool, false)
    }

    func testStatusFilterMapping() {
        XCTAssertTrue(StatusFormat.matches(status: "running", filter: .working))
        XCTAssertTrue(StatusFormat.matches(status: "working", filter: .working))
        XCTAssertFalse(StatusFormat.matches(status: "idle", filter: .working))
        XCTAssertTrue(StatusFormat.matches(status: "waiting_approval", filter: .needsMe))
        XCTAssertFalse(StatusFormat.matches(status: "running", filter: .needsMe))
        XCTAssertTrue(StatusFormat.matches(status: "idle", filter: .all))
        XCTAssertEqual(BotStatusChoice.needsMe.filter, .needsMe)
    }

    func testStatusAgeAndRow() {
        let now = Date()
        XCTAssertEqual(StatusFormat.age(of: now.addingTimeInterval(-600), now: now), "10 minutes ago")
        XCTAssertEqual(StatusFormat.age(of: nil), "time unknown")
        let row = StatusFormat.row(BotInfo(id: "b", name: "Raven", status: "waiting_approval", step: "Drafting"),
                                   asOf: now.addingTimeInterval(-7200), now: now)
        XCTAssertEqual(row, "Raven: waiting approval, Drafting (as of 2 hours ago)")
    }
}
