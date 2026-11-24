import XCTest
@testable import Allternit

/// The size, ETA and blur rules of download option B. Mirrors allternit-ai
/// `byte-progress.test.ts` (#1226); time is passed in, nothing waits.
final class ByteProgressTests: XCTestCase {
    func testUnknownSizeHasNoTotalNoFractionAndNoEta() {
        var t = ByteProgressTracker(total: nil, startedAt: 0)
        let p = t.add(500, at: 2)
        XCTAssertNil(p.totalBytes)
        XCTAssertNil(p.fraction)
        XCTAssertNil(p.etaSeconds)
        XCTAssertEqual(p.bytes, 500)
    }

    func testNoEtaUntilTheDownloadHasRunOneSecond() {
        var t = ByteProgressTracker(total: 1000, startedAt: 0)
        XCTAssertNil(t.add(100, at: 0.5).etaSeconds)
        // Rate smooths 200 B/s toward 100 B/s: about 160 B/s, 800 left, so 5 s.
        XCTAssertEqual(t.add(100, at: 1.5).etaSeconds, 5)
    }

    func testNoEtaOnceAllBytesAreIn() {
        var t = ByteProgressTracker(total: 100, startedAt: 0)
        XCTAssertNil(t.add(100, at: 3).etaSeconds)
        XCTAssertEqual(t.finished().fraction, 1)
    }

    func testTotalNeverFallsBelowBytesReceived() {
        var t = ByteProgressTracker(total: 100, startedAt: 0)
        let p = t.add(150, at: 1)
        XCTAssertEqual(p.totalBytes, 150)
        XCTAssertEqual(p.fraction, 1)
    }

    func testTotalFromTheResponse() {
        XCTAssertEqual(ByteProgressTracker.totalBytes(expectedContentLength: 4096, contentEncoding: nil), 4096)
        XCTAssertEqual(ByteProgressTracker.totalBytes(expectedContentLength: 4096, contentEncoding: "identity"), 4096)
        XCTAssertNil(ByteProgressTracker.totalBytes(expectedContentLength: -1, contentEncoding: nil))
        XCTAssertNil(ByteProgressTracker.totalBytes(expectedContentLength: 0, contentEncoding: nil))
        // Compressed bodies: the length counts encoded bytes, we count decoded ones.
        XCTAssertNil(ByteProgressTracker.totalBytes(expectedContentLength: 4096, contentEncoding: "gzip"))
    }

    func testStatusLineWithAndWithoutASize() {
        let known = ByteProgress(bytes: 3_145_728, totalBytes: 7_759_462, etaSeconds: 3)
        XCTAssertEqual(ByteProgressText.status(verb: "Loading", known), "Loading 40% · 3.0 of 7.4 MB · ETA 3 s")
        let noEta = ByteProgress(bytes: 3_145_728, totalBytes: 7_759_462)
        XCTAssertEqual(ByteProgressText.status(verb: "Saving", noEta), "Saving 40% · 3.0 of 7.4 MB · — ETA")
        let unknown = ByteProgress(bytes: 3_145_728)
        XCTAssertEqual(ByteProgressText.status(verb: "Loading", unknown), "Loading · 3.0 MB · — ETA")
        XCTAssertEqual(ByteProgressText.formatBytes(412 * 1024), "412 KB")
        XCTAssertEqual(ByteProgressText.formatBytes(900), "900 B")
    }

    func testPosterBlurFollowsRealBytes() {
        XCTAssertEqual(ByteProgressText.blur(nil), 28)
        XCTAssertEqual(ByteProgressText.blur(ByteProgress(bytes: 0, totalBytes: 100)), 28)
        XCTAssertEqual(ByteProgressText.blur(ByteProgress(bytes: 50, totalBytes: 100)), 14)
        XCTAssertEqual(ByteProgressText.blur(ByteProgress(bytes: 100, totalBytes: 100)), 0)
        // Unknown size: held, never faked.
        XCTAssertEqual(ByteProgressText.blur(ByteProgress(bytes: 5_000)), 16)
    }

    func testPosterOnlyFromInlineImageOrHttps() {
        func meta(_ poster: String) -> [String: JSONValue] { ["media": .object(["poster": .string(poster)])] }
        XCTAssertEqual(ByteLoading.poster(meta: meta("data:image/jpeg;base64,AAAA")), "data:image/jpeg;base64,AAAA")
        XCTAssertEqual(ByteLoading.poster(meta: meta("https://x.test/p.jpg")), "https://x.test/p.jpg")
        XCTAssertNil(ByteLoading.poster(meta: meta("http://x.test/p.jpg")))
        XCTAssertNil(ByteLoading.poster(meta: meta("javascript:alert(1)")))
        XCTAssertNil(ByteLoading.poster(meta: ["media": .string("video")]))
        XCTAssertNil(ByteLoading.poster(meta: nil))
    }

    func testReportsAreThrottledButTheLastOneAlwaysGoes() {
        XCTAssertFalse(ProgressThrottle.shouldSend(now: 1.05, last: 1.0, minGap: 0.12, complete: false))
        XCTAssertTrue(ProgressThrottle.shouldSend(now: 1.2, last: 1.0, minGap: 0.12, complete: false))
        XCTAssertTrue(ProgressThrottle.shouldSend(now: 1.01, last: 1.0, minGap: 0.12, complete: true))
    }
}
