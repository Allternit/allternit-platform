import XCTest
@testable import AllternitFilesKit

/// 403 `whole_disk_off` (platform #2829): Files shows the server's own message and can offer the Settings switch.
final class WholeDiskErrorTests: XCTestCase {
    func testWholeDiskOffKeepsTheServerMessage() {
        let message = "This folder is outside the Files drive. Turn on whole-disk browsing for this computer in its Files settings first."
        let body = Data(#"{"error":"whole_disk_off","message":"\#(message)"}"#.utf8)
        let error = ErrorDecoding.error(status: 403, data: body)
        XCTAssertTrue(error.isWholeDiskOff)
        XCTAssertEqual(error.localizedDescription, message)
    }

    func testAnOrdinary403IsNotWholeDiskOff() {
        let error = ErrorDecoding.error(status: 403, data: Data(#"{"error":"forbidden","message":"No."}"#.utf8))
        XCTAssertFalse(error.isWholeDiskOff)
    }
}
