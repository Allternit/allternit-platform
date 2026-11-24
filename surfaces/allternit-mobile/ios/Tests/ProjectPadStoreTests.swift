import XCTest
@testable import Allternit

/// Stands in for the runtime (the network edge) so the real transport and store run.
final class PadStubProtocol: URLProtocol {
    /// (method, path, body) -> (status, response body)
    nonisolated(unsafe) static var handler: ((String, String, Data?) -> (Int, String))?
    nonisolated(unsafe) static var seen: [(method: String, path: String, body: Data?)] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        var body = request.httpBody
        if body == nil, let stream = request.httpBodyStream {
            stream.open()
            var data = Data()
            var buffer = [UInt8](repeating: 0, count: 4096)
            while stream.hasBytesAvailable {
                let n = stream.read(&buffer, maxLength: buffer.count)
                if n <= 0 { break }
                data.append(buffer, count: n)
            }
            stream.close()
            body = data
        }
        let method = request.httpMethod ?? "GET"
        // percentEncodedPath keeps "%3A" so the test sees what went on the wire.
        let path = URLComponents(url: request.url!, resolvingAgainstBaseURL: false)?.percentEncodedPath ?? ""
        Self.seen.append((method, path, body))
        let (status, text) = Self.handler?(method, path, body) ?? (500, "")
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(text.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

@MainActor
final class ProjectPadStoreTests: XCTestCase {
    private let scope = "project:cowork:abc"
    private let legacyScope = "project:chat:abc"
    private var store: UserDefaults?

    /// A fresh defaults suite per test, removed afterwards.
    private var defaults: UserDefaults {
        if let store { return store }
        let name = "ProjectPadStoreTests.\(UUID().uuidString)"
        let fresh = UserDefaults(suiteName: name)!
        store = fresh
        addTeardownBlock { UserDefaults().removePersistentDomain(forName: name) }
        return fresh
    }

    /// Each test that uses it sets `PadStubProtocol.handler` first.
    private func stubbedClient() -> APIClient {
        PadStubProtocol.seen = []
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PadStubProtocol.self]
        return APIClient(baseURL: URL(string: "https://runtime.test/api/v1")!, session: URLSession(configuration: config),
                         tokenProvider: { nil }, routeProvider: { .direct })
    }

    private func unavailableClient() -> APIClient {
        APIClient(baseURL: URL(string: "https://runtime.test/api/v1")!, tokenProvider: { nil },
                  routeProvider: { .unavailable(reason: "runtime_unavailable") })
    }

    private func makeStore(_ client: APIClient) -> ProjectPadStore {
        ProjectPadStore(scope: scope, mode: .chat, legacyLookScopes: [legacyScope],
                        transport: RuntimeProjectPadTransport(client: client), defaults: defaults,
                        saveDelayMilliseconds: 0, isSignedIn: { true })
    }

    private func saveOldLook(_ json: String, scope: String) {
        defaults.set(Data(json.utf8), forKey: ProjectLook.storageKey(scope: scope))
    }

    // MARK: Scope

    func testScopeUsesTheWebFormula() {
        XCTAssertEqual(ProjectPadStore.scope(mode: "cowork", id: "abc"), "project:cowork:abc")
        XCTAssertEqual(ProjectPadStore.scope(mode: "bot", id: "b-1"), "project:bot:b-1")
        XCTAssertTrue(ProjectPadStore.isValidScope("project:cowork:abc"))
        XCTAssertFalse(ProjectPadStore.isValidScope("project:x:abc"))
        XCTAssertFalse(ProjectPadStore.isValidScope("project:chat:a b"))
        XCTAssertEqual(PadMode.of("cowork"), .chat)
        XCTAssertEqual(PadMode.of("design"), .design)
    }

    // MARK: Old look conversion

    func testEveryStyleConvertsWithItsTilesAndHiddenOnes() {
        for kind in ProjectLayoutKind.builtIn {
            let look = ProjectLook(layout: kind, colors: "blue", hidden: [.decision])
            let pad = ProjectPad.fromLook(look, now: 42)
            XCTAssertEqual(pad.layout, PadLayout(rawValue: kind.rawValue), kind.rawValue)
            XCTAssertEqual(pad.layout.style, kind)
            XCTAssertEqual(pad.view, .pad)
            XCTAssertEqual(pad.colors, "blue")
            XCTAssertEqual(pad.hidden, [.decision])
            XCTAssertNil(pad.template)
            XCTAssertEqual(pad.data, .empty)
            XCTAssertEqual(pad.updatedAt, 42)
            let expected = kind.tiles.filter { $0 != .decision }.map { PadTile(id: PadTileID($0), size: PadTileID($0).defaultSize) }
            XCTAssertEqual(pad.tiles, expected, kind.rawValue)
        }
        // Spot-check the web's default sizes.
        let pulse = ProjectPad.fromLook(ProjectLook(layout: .pulse), now: 0)
        XCTAssertEqual(pulse.tiles, [PadTile(id: .decision, size: .M), PadTile(id: .chart, size: .S),
                                     PadTile(id: .schedule, size: .M), PadTile(id: .activity, size: .L)])
    }

    func testMakeOneConvertsToTheFreeGridAtDefaultSizes() {
        let look = ProjectLook(layout: .custom, colors: "sunset", hidden: [.team], customTiles: [.files, .countdown, .activity],
                               customPrompt: "files and a deadline")
        let pad = ProjectPad.fromLook(look, now: 7)
        XCTAssertEqual(pad.layout, .pad)
        XCTAssertNil(pad.layout.style)
        XCTAssertEqual(pad.colors, "sunset")
        XCTAssertEqual(pad.hidden, [])
        XCTAssertEqual(pad.tiles, [PadTile(id: .files, size: .M), PadTile(id: .countdown, size: .S), PadTile(id: .activity, size: .L)])
    }

    func testSavedLooksAreReadLeniently() throws {
        let ios = try XCTUnwrap(ProjectLook.decodeSaved(Data(#"{"layout":"score","colors":"green","hidden":["team","number"]}"#.utf8)))
        XCTAssertEqual(ios.layout, .score)
        XCTAssertEqual(ios.colors, "green")
        XCTAssertEqual(ios.hidden, [.team])

        let iosCustom = try XCTUnwrap(ProjectLook.decodeSaved(Data(#"{"layout":"custom","colors":"quiet","hidden":[],"customTiles":["files","nope"],"customPrompt":"x"}"#.utf8)))
        XCTAssertEqual(iosCustom.layout, .custom)
        XCTAssertEqual(iosCustom.customTiles, [.files])

        let web = try XCTUnwrap(ProjectLook.decodeSaved(Data(#"{"layout":"custom","colors":"mono","hidden":[],"custom":{"prompt":"p","tiles":["team"]}}"#.utf8)))
        XCTAssertEqual(web.layout, .custom)
        XCTAssertEqual(web.customTiles, [.team])
        XCTAssertEqual(web.customPrompt, "p")

        // "Make one" with no tiles and an unknown palette read as the web reads them.
        let broken = try XCTUnwrap(ProjectLook.decodeSaved(Data(#"{"layout":"custom","colors":"neon"}"#.utf8)))
        XCTAssertEqual(broken.layout, .pulse)
        XCTAssertEqual(broken.colors, "quiet")
        XCTAssertNil(ProjectLook.decodeSaved(Data("not json".utf8)))
    }

    // MARK: Fallback and sync

    func testUnavailableRuntimeKeepsThePadOnThisDevice() async throws {
        saveOldLook(#"{"layout":"mission","colors":"blue","hidden":["team"]}"#, scope: legacyScope)
        let store = makeStore(unavailableClient())
        await store.load().value

        XCTAssertEqual(store.status, .local)
        XCTAssertEqual(store.statusMessage, "Saved on this device only. Connect a computer to sync.")
        XCTAssertNil(store.error)
        // The old look shows, converted, until the person changes something.
        XCTAssertEqual(store.pad.layout, .mission)
        XCTAssertEqual(store.pad.hidden, [.team])
        XCTAssertEqual(store.pad.tiles.map(\.id), [.decision, .schedule, .activity])

        store.save { $0.data.notes = "Call the printer" }
        await store.flush()
        XCTAssertEqual(store.status, .local)
        XCTAssertEqual(store.pad.data.notes, "Call the printer")
        XCTAssertNotNil(defaults.data(forKey: ProjectPadStore.localPrefix + scope))
        // The old key stays for older builds.
        XCTAssertNotNil(defaults.data(forKey: ProjectLook.storageKey(scope: legacyScope)))

        // A new screen on this device reads the saved pad.
        let again = makeStore(unavailableClient())
        await again.load().value
        XCTAssertEqual(again.pad.data.notes, "Call the printer")
        XCTAssertEqual(again.pad.layout, .mission)
        XCTAssertTrue(again.hasOwnPad)
    }

    func testNoSavedLookOpensOnTheModeDefaultTemplate() async {
        let store = makeStore(unavailableClient())
        await store.load().value
        XCTAssertFalse(store.hasOwnPad)
        XCTAssertEqual(store.pad.template, "research")
        XCTAssertEqual(store.pad.tiles.map(\.id), [.notes, .threads, .links, .files])
        // A late account default is followed while the project has no pad.
        store.setDefaultTemplate(PadTemplate.builtIn(id: "launchpad"))
        XCTAssertEqual(store.pad.template, "launchpad")
    }

    func testFirstEmptyGetUploadsTheConvertedLookAndKeepsTheOldKey() async throws {
        saveOldLook(#"{"layout":"custom","colors":"bright","hidden":[],"customTiles":["decision","files"]}"#, scope: legacyScope)
        PadStubProtocol.handler = { method, _, body in
            method == "GET" ? (200, #"{"pad":null}"#) : (200, String(decoding: body ?? Data(), as: UTF8.self))
        }
        let store = makeStore(stubbedClient())
        await store.load().value

        XCTAssertEqual(store.status, .synced)
        XCTAssertNil(store.statusMessage)
        let put = try XCTUnwrap(PadStubProtocol.seen.first { $0.method == "PUT" })
        XCTAssertEqual(put.path, "/api/v1/project-pad/project%3Acowork%3Aabc")
        let body = try XCTUnwrap(JSONSerialization.jsonObject(with: try XCTUnwrap(put.body)) as? [String: Any])
        let sent = try XCTUnwrap(body["pad"] as? [String: Any])
        XCTAssertEqual(sent["layout"] as? String, "pad")
        XCTAssertEqual(sent["colors"] as? String, "bright")
        XCTAssertTrue(sent["template"] is NSNull, "template goes out as null like the web")
        XCTAssertEqual((sent["tiles"] as? [[String: String]])?.map { $0["id"] ?? "" }, ["decision", "files"])
        XCTAssertNotNil(defaults.data(forKey: ProjectLook.storageKey(scope: legacyScope)))
        XCTAssertEqual(store.pad.tiles.map(\.id), [.decision, .files])
    }

    func testRuntimePadWinsAndIsKeptOnThisDevice() async {
        PadStubProtocol.handler = { _, _, _ in
            (200, #"{"pad":{"v":1,"view":"list","layout":"pad","colors":"green","hidden":[],"template":"build","tiles":[{"id":"notes","size":"W"}],"data":{"notes":"hi","links":[],"checklist":[],"actions":[]},"updatedAt":5}}"#)
        }
        let store = makeStore(stubbedClient())
        await store.load().value
        XCTAssertEqual(store.status, .synced)
        XCTAssertEqual(store.pad.view, .list)
        XCTAssertEqual(store.pad.tiles, [PadTile(id: .notes, size: .W)])
        XCTAssertEqual(store.pad.data.notes, "hi")
        XCTAssertFalse(PadStubProtocol.seen.contains { $0.method == "PUT" })
        let local = defaults.data(forKey: ProjectPadStore.localPrefix + scope).flatMap { ProjectPad.decodeStored($0, mode: .chat) }
        XCTAssertEqual(local?.data.notes, "hi")
    }

    func testOldRuntimeAsksForAnUpdate() async {
        PadStubProtocol.handler = { _, _, _ in (404, "") }
        let store = makeStore(stubbedClient())
        await store.load().value
        XCTAssertEqual(store.status, .oldRuntime)
        XCTAssertEqual(store.statusMessage, "Update your computer to sync pads.")
    }

    func testSignedOutIsReadOnly() async {
        let store = ProjectPadStore(scope: scope, mode: .code, transport: RuntimeProjectPadTransport(client: unavailableClient()),
                                    defaults: defaults, saveDelayMilliseconds: 0, isSignedIn: { false })
        await store.load().value
        XCTAssertEqual(store.status, .signedOut)
        XCTAssertEqual(store.pad.template, "build")
        store.save { $0.data.notes = "nope" }
        XCTAssertEqual(store.pad.data.notes, "")
        XCTAssertEqual(store.error, "Sign in to customize")
        XCTAssertNil(defaults.data(forKey: ProjectPadStore.localPrefix + scope))
    }

    // MARK: Normalizing

    func testNormalizeAppliesTheServerRules() throws {
        let notes = String(repeating: "a", count: 4100)
        let json = #"""
        {"v":1,"view":"pad","layout":"custom","colors":"neon","hidden":["team","team","number"],
         "tiles":[{"id":"notes","size":"W"},{"id":"notes","size":"S"},{"id":"number","size":"S"},{"id":"links"}],
         "data":{"notes":"NOTES",
           "links":[{"title":"","url":"https://example.com/x"},{"title":"bad","url":"javascript:alert(1)"}],
           "checklist":[{"id":"c1","text":"one","done":1},{"id":"c1","text":"two","done":true}],
           "actions":[{"id":"a1","kind":"link","label":"Site","value":"ftp://x"},{"id":"a2","kind":"thread","label":"Ask","value":"t1"}]},
         "updatedAt":true}
        """#.replacingOccurrences(of: "NOTES", with: notes)
        let raw = try JSONSerialization.jsonObject(with: Data(json.utf8))
        let pad = ProjectPad.normalize(raw, mode: .chat)
        XCTAssertEqual(pad.layout, .pad)
        XCTAssertEqual(pad.colors, "quiet")
        XCTAssertEqual(pad.hidden, [.team])
        XCTAssertEqual(pad.tiles, [PadTile(id: .notes, size: .W), PadTile(id: .links, size: .M)])
        XCTAssertEqual(pad.data.notes.count, 4000)
        XCTAssertEqual(pad.data.links, [PadLink(title: "example.com", url: "https://example.com/x")])
        XCTAssertEqual(pad.data.checklist.map(\.done), [false, true])
        XCTAssertNotEqual(pad.data.checklist[0].id, pad.data.checklist[1].id)
        XCTAssertEqual(pad.data.actions.map(\.id), ["a2"])
        XCTAssertEqual(pad.updatedAt, 0)
        XCTAssertNil(ProjectPad.normalize(raw, mode: .chat).template)
    }
}

final class PadCustomColorsTests: XCTestCase {
    func testParsesAndSerializesLowercase() throws {
        let own = try XCTUnwrap(CustomPadColors.parse("#A1B2C3,#00ff7f"))
        XCTAssertEqual(own.main, 0xA1B2C3)
        XCTAssertEqual(own.accent, 0x00FF7F)
        XCTAssertEqual(own.serialized, "#a1b2c3,#00ff7f")
        XCTAssertEqual(own.serialized.count, 15)
        XCTAssertEqual(CustomPadColors.parse(own.serialized), own)
    }

    func testRejectsAnythingElse() {
        for bad in ["quiet", "", "#fff,#000", "#a1b2c3", "#a1b2c3,#00ff7", "a1b2c3,00ff7f", "#a1b2c3,#00ff7f,", "#a1b2cg,#00ff7f", " #a1b2c3,#00ff7f"] {
            XCTAssertNil(CustomPadColors.parse(bad), bad)
        }
    }

    func testInkFollowsLuminance() {
        XCTAssertTrue(CustomPadColors.prefersDarkInk(0xFFFFFF))
        XCTAssertTrue(CustomPadColors.prefersDarkInk(0xFFC31F))
        XCTAssertFalse(CustomPadColors.prefersDarkInk(0x000000))
        XCTAssertFalse(CustomPadColors.prefersDarkInk(0x2F54EB))
        XCTAssertEqual(CustomPadColors.luminance(0xFFFFFF), 1, accuracy: 0.0001)
    }

    func testMixMatchesTheWebRule() {
        XCTAssertEqual(CustomPadColors.mix(0xFF0000, over: 0xFFFFFF, amount: 0.28), 0xFFB8B8)
        XCTAssertEqual(CustomPadColors.mix(0xFFFFFF, over: 0x000000, amount: 0.55), 0x8C8C8C)
    }

    func testPadKeepsCustomColorsAndFallsBackOnJunk() {
        let kept = ProjectPad.normalize(["colors": "#a1b2c3,#00ff7f"], mode: .chat)
        XCTAssertEqual(kept.colors, "#a1b2c3,#00ff7f")
        XCTAssertEqual(kept.normalized(mode: .chat).colors, "#a1b2c3,#00ff7f")
        XCTAssertEqual(ProjectPad.normalize(["colors": "#a1b2c3,nope"], mode: .chat).colors, "quiet")
        let look = ProjectLook.decodeSaved(Data(##"{"layout":"pulse","colors":"#a1b2c3,#00ff7f"}"##.utf8))
        XCTAssertEqual(look?.colors, "#a1b2c3,#00ff7f")
        XCTAssertEqual(ProjectColorSet.named("#a1b2c3,#00ff7f").custom?.main, 0xA1B2C3)
        XCTAssertNil(ProjectColorSet.named("blue").custom)
    }
}
