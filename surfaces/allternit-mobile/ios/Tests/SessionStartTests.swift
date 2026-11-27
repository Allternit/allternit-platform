import XCTest
@testable import Allternit

/// Start anywhere: the iOS models decode the contract fixtures
/// (docs/contracts/session-start, copied to Tests/Fixtures/SessionStart) and
/// the request encodes to the same JSON the device expects.
final class SessionStartTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "session-start.\(name)", withExtension: "json"),
                                "missing fixture \(name)")
        return try Data(contentsOf: url)
    }

    func testOptionsDecode() throws {
        let options = try JSONDecoder().decode(StartOptions.self, from: fixture("options.response"))
        XCTAssertEqual(options.device.name, "Studio Mac")
        XCTAssertTrue(options.device.remoteStart)
        XCTAssertEqual(options.folders.count, 4)
        XCTAssertEqual(options.folders[1].git?.dirty, true)
        XCTAssertEqual(options.folders[1].git?.ahead, 2)
        XCTAssertNil(options.folders[2].git)
        XCTAssertEqual(options.defaultRuntime, "allternit")
        XCTAssertEqual(options.modes["codex"]?.supported, ["auto", "default", "bypass"])
        XCTAssertEqual(options.defaultMode(for: "codex"), "auto")
        // A runtime the device has no entry for falls back to its default runtime's mode.
        XCTAssertEqual(options.defaultMode(for: "kimi"), "default")
    }

    func testStartResponsesDecode() throws {
        let engine = try JSONDecoder().decode(StartResult.self, from: fixture("start.response.engine"))
        XCTAssertEqual(engine, .engine(sessionId: "ses_01JD4Z8K2Q7W3M9X5T6V1N0B8C", directory: "/Users/dev/code/allternit"))
        let terminal = try JSONDecoder().decode(StartResult.self, from: fixture("start.response.terminal"))
        XCTAssertEqual(terminal, .terminal(terminalId: "term_7c1e9a40", cwd: "/Users/dev/code/allternit", agent: "claude"))
    }

    func testRequestsRoundTrip() throws {
        for name in ["start.request.path", "start.request.git"] {
            let data = try fixture(name)
            let request = try JSONDecoder().decode(StartRequest.self, from: data)
            XCTAssertEqual(request.runtime, "allternit")
            XCTAssertEqual(request.origin.surface, "ios")
            let reencoded = try JSONSerialization.jsonObject(with: JSONEncoder().encode(request)) as? NSDictionary
            let original = try JSONSerialization.jsonObject(with: data) as? NSDictionary
            XCTAssertEqual(reencoded, original, name)
        }
        let git = try JSONDecoder().decode(StartRequest.self, from: fixture("start.request.git"))
        XCTAssertEqual(git.folder, .git(remote: "https://github.com/example-org/allternit.git", branch: "main", head: nil))
        XCTAssertEqual(git.folder.displayName, "allternit")
    }

    func testErrorsCarryMessageAndAction() throws {
        let raw = try JSONSerialization.jsonObject(with: fixture("errors")) as? [[String: Any]] ?? []
        XCTAssertEqual(raw.count, 8)
        for entry in raw {
            let status = entry["status"] as? Int ?? 0
            let error = StartSessionError.from(status: status, data: try JSONSerialization.data(withJSONObject: entry))
            XCTAssertEqual(error.code, entry["error"] as? String)
            XCTAssertEqual(error.inline, "\(entry["message"] as? String ?? "") \(entry["action"] as? String ?? "")")
        }
    }

    func testSnapshotsDecode() throws {
        let delta = try JSONSerialization.jsonObject(with: fixture("presence.delta")) as? [String: Any]
        let get = try XCTUnwrap((delta?["getStartOptions"] as? [String: Any])?["response"])
        let snapshots = try JSONDecoder().decode(StartSnapshots.self, from: JSONSerialization.data(withJSONObject: get))
        XCTAssertEqual(snapshots.devices.first?.kind, "desktop")
        XCTAssertEqual(snapshots.devices.first?.folders.first?.git?.branch, "main")
        let example = try XCTUnwrap((delta?["presence"] as? [String: Any])?["example"] as? [String: Any])
        var row = example
        row["online"] = false
        let presence = try JSONDecoder().decode(StartPresence.self, from: JSONSerialization.data(withJSONObject: row))
        XCTAssertEqual(presence.kind, "desktop")
        XCTAssertEqual(presence.remoteStart, true)
    }

    func testChipRuntimeIdsMapToContractIds() {
        XCTAssertEqual(SessionStartClient.contractRuntime("allternit"), "allternit")
        XCTAssertEqual(SessionStartClient.contractRuntime("claude-code"), "claude")
        XCTAssertEqual(SessionStartClient.contractRuntime("codex"), "codex")
        XCTAssertEqual(SessionStartClient.contractRuntime("qwen-cli"), "qwen")
    }

    /// The asleep offer: a git folder moves to a cloud computer; a folder
    /// without a remote, or a computer with no snapshot, gives a reason.
    func testAsleepOffer() throws {
        let options = try JSONDecoder().decode(StartOptions.self, from: fixture("options.response"))
        let snapshot = try JSONDecoder().decode(StartSnapshotDevice.self, from: JSONSerialization.data(withJSONObject: [
            "runtimeId": "rt_8f3a21c9", "name": "Studio Mac",
            "folders": try JSONSerialization.jsonObject(with: JSONEncoder().encode(options.folders)),
        ]))
        let row = StartDeviceRow(id: "rt_8f3a21c9", name: "Studio Mac", state: .asleep, sub: "Asleep")
        guard case .cloud(let cloudId, let folderName, let git) = StartDeviceRows.asleepOffer(
            for: row, snapshot: snapshot, folderPath: nil, cloudRuntimeId: "rt_cloud") else {
            return XCTFail("expected a cloud offer")
        }
        XCTAssertEqual(cloudId, "rt_cloud")
        XCTAssertEqual(folderName, "allternit")
        XCTAssertEqual(git.branch, "main")
        guard case .none(_, let reason) = StartDeviceRows.asleepOffer(
            for: row, snapshot: snapshot, folderPath: "/Users/dev/Documents/taxes-2026", cloudRuntimeId: "rt_cloud") else {
            return XCTFail("expected no offer for a folder without git")
        }
        XCTAssertTrue(reason.contains("no git remote"))
        guard case .none = StartDeviceRows.asleepOffer(for: row, snapshot: nil, folderPath: nil, cloudRuntimeId: "rt_cloud") else {
            return XCTFail("expected no offer without a snapshot")
        }
    }
}
