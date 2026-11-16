import XCTest
@testable import Allternit

final class PhoneLiveTests: XCTestCase {
    private let sps = Data([0x67, 0x42, 0x00, 0x1f])
    private let pps = Data([0x68, 0xce, 0x3c, 0x80])
    private let idr = Data([0x65, 0x88, 0x84, 0x01])
    private let slice = Data([0x41, 0x9a, 0x02])

    private func annexB(_ nals: [Data], longStart: Bool = true) -> Data {
        var out = Data()
        for nal in nals {
            out.append(contentsOf: longStart ? [0, 0, 0, 1] : [0, 0, 1])
            out.append(nal)
        }
        return out
    }

    func testConfigPacketResetsTheLayerAndFramesBeforeAFormatAreDropped() throws {
        let config = LiveScreenPacket.json(.config, ["width": 720, "height": 1600, "scale": 1, "rotation": 0])
        let packet = try XCTUnwrap(LivePacket.read(config))
        XCTAssertEqual(packet, .config(width: 720, height: 1600, rotation: 0))
        XCTAssertEqual(LiveLayerAction.decide(for: packet, hasFormat: true), .reset(width: 720, height: 1600))

        // A P-frame with no format yet cannot start the picture; an IDR with SPS+PPS can.
        let p = LivePacket.frame(annexB([slice]))
        XCTAssertEqual(LiveLayerAction.decide(for: p, hasFormat: false), .ignore)
        let key = LivePacket.frame(annexB([sps, pps, idr]))
        guard case .enqueue(let unit) = LiveLayerAction.decide(for: key, hasFormat: false) else { return XCTFail("keyframe should enqueue") }
        XCTAssertTrue(unit.isKeyframe)
        XCTAssertEqual(unit.sps, sps)
        XCTAssertEqual(unit.pps, pps)
        XCTAssertEqual(unit.slices, [idr])
        guard case .enqueue = LiveLayerAction.decide(for: p, hasFormat: true) else { return XCTFail("P-frame enqueues once a format exists") }
    }

    func testNalSplitHandlesThreeAndFourByteStartCodesAndAvccFraming() {
        let unit = AnnexBAccessUnit.parse(annexB([sps, pps], longStart: false) + annexB([idr, slice]))
        XCTAssertEqual(unit.sps, sps)
        XCTAssertEqual(unit.slices, [idr, slice])
        XCTAssertEqual(unit.avcc.prefix(4), Data([0, 0, 0, 4]))
        XCTAssertEqual(unit.avcc.count, 4 + idr.count + 4 + slice.count)
    }

    func testPacketReaderRejectsPartialFrames() {
        var packet = LiveScreenPacket.encode(.frame, Data([1, 2, 3]))
        XCTAssertNotNil(LivePacket.read(packet))
        packet.removeLast()
        XCTAssertNil(LivePacket.read(packet))
        XCTAssertEqual(LivePacket.read(LiveScreenPacket.json(.ended, ["reason": "x"])), .ended(reason: "x"))
    }

    func testTapMapsThroughAspectFitLetterboxing() throws {
        // A 1000x2000 picture in a 400x400 view is 200 wide, centred (100pt bars).
        let view = CGSize(width: 400, height: 400), video = CGSize(width: 1000, height: 2000)
        XCTAssertEqual(PhoneLiveGeometry.fittedRect(video: video, in: view), CGRect(x: 100, y: 0, width: 200, height: 400))
        XCTAssertEqual(PhoneLiveGeometry.videoPoint(CGPoint(x: 200, y: 200), view: view, video: video, clamp: false), CGPoint(x: 500, y: 1000))
        XCTAssertEqual(PhoneLiveGeometry.videoPoint(CGPoint(x: 100, y: 0), view: view, video: video, clamp: false), CGPoint(x: 0, y: 0))
        XCTAssertNil(PhoneLiveGeometry.videoPoint(CGPoint(x: 50, y: 200), view: view, video: video, clamp: false))
        XCTAssertEqual(PhoneLiveGeometry.videoPoint(CGPoint(x: 50, y: 500), view: view, video: video, clamp: true), CGPoint(x: 0, y: 2000))
    }

    func testPendingPairsKeepOnlySixDigitCodes() {
        let json = #"{"pairs":[{"phoneId":"abc","name":"iPhone","model":"Moto","code":"123456","platform":"android"},{"phoneId":"d","code":"12"}]}"#
        let list = AndroidPairing.readPending(Data(json.utf8))
        XCTAssertEqual(list.count, 1)
        XCTAssertEqual(list[0].name, "Android phone")
        XCTAssertEqual(AndroidPairing.deepLink("ABCD-1234")?.absoluteString, "allternit://phone-share?code=ABCD-1234")
    }
}
