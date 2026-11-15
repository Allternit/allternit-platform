import XCTest
@testable import Allternit

final class StudioJobsClientTests: XCTestCase {
    private func page(_ json: String) throws -> StudioEventsPage {
        try JSONDecoder().decode(StudioEventsPage.self, from: Data(json.utf8))
    }

    func testPagingAdvancesAndSkipsReplayedEvents() throws {
        var tail = StudioLogTail()
        tail.apply(try page(#"{"state":"running","events":[{"seq":1,"at":"t","kind":"log","message":"a"},{"seq":2,"at":"t","kind":"progress","message":"step","progress":0.25}]}"#))
        XCTAssertEqual(tail.lastSeq, 2)
        XCTAssertEqual(tail.progress, 0.25)
        tail.apply(try page(#"{"state":"running","events":[{"seq":2,"at":"t","kind":"progress","message":"step","progress":0.25},{"seq":3,"at":"t","kind":"log","message":"b"}]}"#))
        XCTAssertEqual(tail.lines.map(\.seq), [1, 2, 3])
        XCTAssertEqual(tail.lastSeq, 3)
    }

    func testTailKeepsLastFiftyLines() throws {
        var tail = StudioLogTail()
        let events = (1...80).map { #"{"seq":\#($0),"at":"t","kind":"log","message":"l\#($0)"}"# }.joined(separator: ",")
        tail.apply(try page(#"{"state":"running","events":[\#(events)]}"#))
        XCTAssertEqual(tail.lines.count, 50)
        XCTAssertEqual(tail.lines.first?.seq, 31)
        XCTAssertEqual(tail.lastSeq, 80)
    }

    func testJobDecodes() throws {
        let json = #"{"id":"j1","state":"running","spec":{"spec_version":1,"engine":"mlx","base":{"repo":"org/m"},"method":"lora","resources":{"place":"local"}},"created_at":"2026-11-15T10:00:00Z","last_seq":4}"#
        let job = try JSONDecoder().decode(StudioJob.self, from: Data(json.utf8))
        XCTAssertEqual(job.spec.engine, "mlx")
        XCTAssertFalse(job.isFinished)
    }
}

final class StudioJobsFormatTests: XCTestCase {
    func testMethodLabels() {
        XCTAssertEqual(StudioJobsFormat.methodLabel("lora"), "LoRA")
        XCTAssertEqual(StudioJobsFormat.methodLabel("qlora"), "QLoRA")
        XCTAssertEqual(StudioJobsFormat.methodLabel("dpo"), "DPO")
        XCTAssertEqual(StudioJobsFormat.methodLabel("full"), "Full fine-tune")
    }

    func testRawErrorsBecomePlainSentences() {
        XCTAssertEqual(FriendlyErrorMessage.from("nf", doing: "load engines"), "Couldn't load engines. Check your connection and try again.")
        XCTAssertEqual(FriendlyErrorMessage.from("Freeweights answered 500", doing: "load engines"), "Couldn't load engines. Check your connection and try again.")
        XCTAssertEqual(FriendlyErrorMessage.from("Connect a computer or start Allternit Cloud to use this.", doing: "x"), "Connect a computer or start Allternit Cloud to use this.")
    }
}
