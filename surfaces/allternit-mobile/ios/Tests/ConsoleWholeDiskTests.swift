import XCTest
@testable import Allternit

/// Whole-disk Files (platform #2829): the optional `wholeDisk` field on a console
/// row, and the 403 `whole_disk_off` message reaching the user as the server wrote it.
final class ConsoleWholeDiskTests: XCTestCase {
    private func row(_ extra: String) throws -> ACIComputersClient.ConsoleComputer {
        let json = """
        {"id":"c1","kind":"paired","group":"g","name":"Studio","provider":"Mac","os":"macOS","osFamily":"mac",
         "reach":"relay","status":"online"\(extra)}
        """
        return try JSONDecoder().decode(ACIComputersClient.ConsoleComputer.self, from: Data(json.utf8))
    }

    func testMissingWholeDiskIsUnavailableWithNoSwitcher() throws {
        let c = try row("")
        XCTAssertNil(c.wholeDisk)
        XCTAssertFalse(c.wholeDiskState.available)
        XCTAssertFalse(c.wholeDiskState.showsRootsSwitcher)
    }

    func testEnabledListsRootsInServerOrder() throws {
        let c = try row(#","wholeDisk":{"enabled":true,"available":true,"roots":[{"id":"disk:C:","path":"C:\\","label":"Drive C:"},{"id":"disk:D:","path":"D:\\","label":"Drive D:"}]}"#)
        XCTAssertTrue(c.wholeDiskState.showsRootsSwitcher)
        XCTAssertEqual(c.wholeDiskState.roots.map(\.label), ["Drive C:", "Drive D:"])
    }

    func testOffHidesTheSwitcherAndUnavailableSaysWhy() throws {
        let off = try row(#","wholeDisk":{"enabled":false,"available":true,"roots":[]}"#)
        XCTAssertFalse(off.wholeDiskState.showsRootsSwitcher)
        let shared = try row(#","wholeDisk":{"enabled":false,"available":false,"reason":"not_owner","roots":[]}"#)
        XCTAssertEqual(shared.wholeDiskState.reasonCopy, "Only the owner of this computer can turn this on.")
        XCTAssertEqual(try row(#","wholeDisk":{"enabled":false,"available":false,"roots":[]}"#).wholeDiskState.reasonCopy,
                       "Not available on this computer.")
    }

    func testWholeDiskOffKeepsTheServerMessage() throws {
        let message = "This folder is outside the Files drive. Turn on whole-disk browsing for this computer in its Files settings first."
        let body = Data(#"{"error":"whole_disk_off","message":"\#(message)"}"#.utf8)
        let url = URL(string: "https://cloud.example/api/v1/computers/c1/files/roots")!
        let response = HTTPURLResponse(url: url, statusCode: 403, httpVersion: nil, headerFields: nil)!
        XCTAssertThrowsError(try ACIComputersClient.check(response, data: body)) { error in
            guard case ACIComputersClient.Failure.wholeDiskOff(let shown) = error else { return XCTFail("got \(error)") }
            XCTAssertEqual(shown, message)
            XCTAssertEqual(consoleFailureText(error), message)
        }
    }

    func testAnOrdinary403IsNotWholeDiskOff() {
        let body = Data(#"{"error":"forbidden","message":"No."}"#.utf8)
        let url = URL(string: "https://cloud.example/api/v1/computers/c1/files/roots")!
        let response = HTTPURLResponse(url: url, statusCode: 403, httpVersion: nil, headerFields: nil)!
        XCTAssertThrowsError(try ACIComputersClient.check(response, data: body)) { error in
            if case ACIComputersClient.Failure.wholeDiskOff = error { XCTFail("not whole-disk") }
        }
    }
}
