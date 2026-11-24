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

    /// A job on a local folder has no repo; it must still decode, show the folder's name and the backend the host picked.
    func testLocalBaseJobWithBackendDecodes() throws {
        let json = #"{"id":"j2","state":"queued","spec":{"engine":"allternit-tune","base":{"path":"/Users/x/models/qwen-small"},"method":"lora"},"created_at":"t","backend":"mlx"}"#
        let job = try JSONDecoder().decode(StudioJob.self, from: Data(json.utf8))
        XCTAssertNil(job.spec.base.repo)
        XCTAssertEqual(job.spec.base.name, "qwen-small")
        XCTAssertEqual(job.backend, "mlx")
        XCTAssertEqual(StudioJobsFormat.backendLine(nil, jobBackend: job.backend), "Runs with MLX")
    }
}

final class StudioEngineCardTests: XCTestCase {
    func testCardWithUnknownHardwareAndFitDecodes() throws {
        let json = #"{"engines":[{"id":"allternit-tune","name":"Allternit Tune","fit":{"ok":true,"hardware":"quantum-x9","backend":"torch-warp","reason":"r","warning":null,"methods_unavailable":{"dpo":"no"},"outputs_unavailable":{"holo":"no"}},"extra":1},{"id":"old","name":"Old"},{"id":"bad","name":"Bad","fit":{"ok":"maybe","backend":7}}]}"#
        struct Env: Decodable { let engines: [StudioEngineCard] }
        let env = try JSONDecoder().decode(Env.self, from: Data(json.utf8))
        XCTAssertEqual(env.engines[0].fit?.hardware, "quantum-x9")
        XCTAssertEqual(env.engines[0].fit?.outputsUnavailable?["holo"], "no")
        XCTAssertNil(env.engines[1].fit)
        XCTAssertNil(env.engines[2].fit?.backend)
        XCTAssertEqual(StudioJobsFormat.backendLine(env.engines[0].fit), "Runs with torch-warp")
        let mac = StudioFit(ok: true, hardware: "apple-silicon", backend: "mlx")
        XCTAssertEqual(StudioJobsFormat.backendLine(mac), "Runs with MLX on Mac with Apple Silicon")
        XCTAssertTrue(StudioJobsFormat.isUntested(mac))
    }

    func testMlxOutputAndUnknownKindDecode() throws {
        let job = #"{"id":"j","state":"succeeded","spec":{"engine":"e","base":{"repo":"o/m"},"method":"lora","outputs":{"mlx":true}},"created_at":"t"}"#
        XCTAssertEqual(try JSONDecoder().decode(StudioJob.self, from: Data(job.utf8)).spec.outputs?.mlx, true)
        let out = try JSONDecoder().decode(StudioJobOutput.self, from: Data(#"{"kind":"mlx","path":"/p"}"#.utf8))
        XCTAssertEqual(StudioJobsFormat.outputLabel(out.kind), "MLX model")
        XCTAssertEqual(StudioJobsFormat.outputLabel("zzz"), "zzz")
    }

    func testRefusalSentencesUseHostReason() {
        XCTAssertEqual(StudioJobsFormat.sentence(code: "method_not_supported", reason: "QLoRA needs a GPU."), "QLoRA needs a GPU.")
        XCTAssertEqual(StudioJobsFormat.sentence(code: "backend_not_supported", reason: "backend_not_supported"), "That backend can't be used on your computer. Leave the backend on automatic.")
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
