import XCTest
@testable import Allternit

final class ArtifactTileFilterTests: XCTestCase {
    func testTapFiltersThenClears() {
        XCTAssertEqual(ArtifactTileFilter.tap(.docs, current: nil), .filter(.doc))
        XCTAssertEqual(ArtifactTileFilter.tap(.docs, current: .doc), .clear)
        XCTAssertEqual(ArtifactTileFilter.tap(.sheets, current: .doc), .filter(.sheet))
    }

    func testTileWithoutKindStillCreates() {
        XCTAssertEqual(ArtifactTileFilter.tap(.build, current: nil), .noFilter)
        XCTAssertEqual(ArtifactTileFilter.tap(.research, current: .doc), .noFilter)
    }

    func testEveryOtherTileMapsToAKind() {
        let mapped = ArtifactStudioKind.allCases.filter { $0.filterKind != nil }
        XCTAssertEqual(mapped.count, 10)
    }

    func testStringsMirrorWeb() {
        XCTAssertEqual(ArtifactTileFilter.newRowTitle(.docs), "New Docs")
        XCTAssertEqual(ArtifactTileFilter.emptyText(kind: .doc, tile: .docs), "No docs yet")
        XCTAssertEqual(ArtifactTileFilter.emptyText(kind: .pdf, tile: nil), "No pdfs yet")
        XCTAssertEqual(ArtifactTileFilter.subtitle(kindLabel: "Doc", edited: "Edited today"), "Doc · Edited today")
        XCTAssertEqual(ArtifactTileFilter.subtitle(kindLabel: "Doc", edited: ""), "Doc")
    }

    func testLaunchArgument() {
        XCTAssertEqual(ArtifactTileFilter.kind(fromArgument: "docs"), .doc)
        XCTAssertEqual(ArtifactTileFilter.kind(fromArgument: "slides"), .slides)
        XCTAssertEqual(ArtifactTileFilter.kind(fromArgument: "pdf"), .pdf)
        XCTAssertNil(ArtifactTileFilter.kind(fromArgument: "nope"))
    }
}
