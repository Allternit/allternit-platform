import XCTest
@testable import Allternit

final class AppearanceClientTests: XCTestCase {
    private func decode(_ json: String) throws -> Appearance? {
        try AppearanceClient.decode(Data(json.utf8))
    }

    private let legacy = ##"{"version":1,"color":"#1F4FA8","paperColor":"#ffffff","documents":true,"overlay":"text","symbol":"sparkle","text":"AB","overlayColor":"#ffffff","x":0,"y":0,"scale":1,"rotation":0"##

    func testRecordWithoutCoverDecodesAndResolvesToClassicWithLegacyArt() throws {
        let a = try XCTUnwrap(try decode(#"{"appearance":"# + legacy + "}}"))
        XCTAssertNil(a.cover)
        XCTAssertEqual(a.color, "#1F4FA8")
        var expected = Cover.classic
        expected.art = .text
        XCTAssertEqual(Appearance.resolvedCover(a), expected)
    }

    func testRecordWithCoverDecodesEveryField() throws {
        let cover = ##","cover":{"v":2,"preset":"badge","art":"symbol","font":"sans","place":"top","pattern":"dots","frame":"none","edge":"shadow","sheets":3,"tabColor":"#9B6BD6","tabLabel":"LAUNCH","showMode":false}"##
        let a = try XCTUnwrap(try decode(#"{"appearance":"# + legacy + cover + "}}"))
        let c = try XCTUnwrap(a.cover)
        XCTAssertEqual(c.preset, "badge")
        XCTAssertEqual(c.art, .symbol)
        XCTAssertEqual(c.font, .sans)
        XCTAssertEqual(c.place, .top)
        XCTAssertEqual(c.pattern, .dots)
        XCTAssertEqual(c.frame, .none)
        XCTAssertEqual(c.edge, .shadow)
        XCTAssertEqual(c.sheets, 3)
        XCTAssertEqual(c.tabColor, "#9B6BD6")
        XCTAssertEqual(c.tabLabel, "LAUNCH")
        XCTAssertFalse(c.showMode)
        XCTAssertEqual(Appearance.resolvedCover(a), c)
    }

    func testUnknownCoverFallsBackToClassic() throws {
        let bad = #","cover":{"v":3,"art":"hologram"}"#
        let a = try XCTUnwrap(try decode(#"{"appearance":"# + legacy + bad + "}}"))
        XCTAssertNil(a.cover)
        XCTAssertEqual(Appearance.resolvedCover(a).pattern, .grain)
        let badEnum = #","cover":{"v":2,"art":"hologram"}"#
        XCTAssertNil(try XCTUnwrap(try decode(#"{"appearance":"# + legacy + badEnum + "}}")).cover)
    }

    func testNullRecordAndMissingRecordUseTheDefaultCover() throws {
        XCTAssertNil(try decode(#"{"appearance":null}"#))
        XCTAssertEqual(Appearance.resolvedCover(nil), Cover.classic)
        XCTAssertEqual(Cover.classic.art, .title)
        XCTAssertEqual(Cover.classic.sheets, 2)
    }

    func testOverlayNoneShowsTheTitle() throws {
        var a = Appearance()
        a.overlay = "none"
        XCTAssertEqual(Appearance.resolvedCover(a).art, .title)
    }

    func testWithCoverSyncKeepsOldFieldsInStep() {
        var a = Appearance()
        var cover = Cover.classic
        cover.art = .title
        cover.sheets = 0
        a.cover = cover
        a.overlay = "symbol"
        a.documents = true
        let synced = a.withCoverSync()
        XCTAssertEqual(synced.overlay, "none")
        XCTAssertFalse(synced.documents)
    }

    func testKeyEncoding() {
        let key = AppearanceClient.projectKey(mode: "chat", id: "abc-123")
        XCTAssertEqual(key, ["project", "chat", "abc-123"])
        XCTAssertEqual(AppearanceClient.keyString(key), #"["project","chat","abc-123"]"#)
        XCTAssertEqual(AppearanceClient.encodedKey(key), "%5B%22project%22%2C%22chat%22%2C%22abc-123%22%5D")
        XCTAssertEqual(AppearanceClient.path(for: key), "appearance/%5B%22project%22%2C%22chat%22%2C%22abc-123%22%5D")
        XCTAssertEqual(AppearanceClient.keyString(["project", "code", "a/b"]), #"["project","code","a/b"]"#)
    }

    func testCoverRoundTripsThroughJSON() throws {
        var a = Appearance()
        a.cover = Cover.classic
        let data = try JSONEncoder().encode(["appearance": a])
        XCTAssertEqual(try AppearanceClient.decode(data), a)
    }

    func testSavedLookRoundTripsAndStaysInSync() throws {
        var a = Appearance()
        a.image = "data:image/png;base64,iVBORw0KGgo="
        a = FolderLookEdit.applyPreset(a, FolderCoverPreset.all.first { $0.id == "badge" }!)
        a = FolderLookEdit.patchCover(a) { $0.art = .image; $0.sheets = 0; $0.tabLabel = "Quarterly plan" }
        XCTAssertEqual(a.cover?.tabLabel, "Quarterl") // Cover.tabLabelMax = 8, same as web COVER_TAB_LABEL_MAX
        XCTAssertNil(a.cover?.preset)
        XCTAssertEqual(a.overlay, "image")
        XCTAssertFalse(a.documents)
        XCTAssertEqual(a.color, "#9B6BD6")
        let data = try JSONEncoder().encode(["appearance": a.withCoverSync()])
        let back = try XCTUnwrap(AppearanceClient.decode(data))
        XCTAssertEqual(back, a)
        XCTAssertEqual(back.cover?.pattern, .dots)
        XCTAssertEqual(back.cover?.edge, .shadow)
    }
}
