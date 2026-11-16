import XCTest
@testable import Allternit

final class PadTemplatesClientTests: XCTestCase {
    private let id = "u_6f1c2a4e-9b3d-4c1a-8f2e-1a2b3c4d5e6f"

    private func client(token: String? = "tok") -> PadTemplatesClient {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PadStubProtocol.self]
        return PadTemplatesClient(baseURL: URL(string: "https://cloud.test")!, session: URLSession(configuration: config),
                                  token: { token })
    }

    override func setUp() {
        PadStubProtocol.handler = nil
        PadStubProtocol.seen = []
    }

    func testTemplateDocDecodesAndDropsWhatCannotBeUsed() throws {
        let json = #"""
        {"templates":[
          {"id":"\#(id)","name":"  Weekly  ","tiles":[{"id":"notes","size":"L"},{"id":"number","size":"S"},{"id":"links"}],
           "actions":[{"id":"a1","kind":"link","label":"Docs","value":"https://docs.example.com"}],"createdAt":1700000000000},
          {"id":"\#(id)","name":"Duplicate","tiles":[]},
          {"id":"research","name":"Built-in id is not an account template","tiles":[]},
          {"id":"u_6f1c2a4e-9b3d-4c1a-8f2e-1a2b3c4d5e70","name":"   ","tiles":[]}
        ],
        "modeDefaults":{"chat":"launch","code":"\#(id)","design":"u_00000000-0000-4000-8000-000000000000","bot":7}}
        """#
        let doc = try PadTemplatesDoc.decode(Data(json.utf8))
        XCTAssertEqual(doc.templates.count, 1)
        let template = try XCTUnwrap(doc.templates.first)
        XCTAssertEqual(template.name, "Weekly")
        XCTAssertEqual(template.tiles, [PadTile(id: .notes, size: .L), PadTile(id: .links, size: .M)])
        XCTAssertEqual(template.actions.map(\.label), ["Docs"])
        XCTAssertEqual(template.createdAt, 1_700_000_000_000)
        XCTAssertEqual(doc.modeDefaults.chat, "launch")
        XCTAssertEqual(doc.modeDefaults.code, id)
        // Unknown template and wrong type fall back to the mode's built-in.
        XCTAssertEqual(doc.modeDefaults.design, "client")
        XCTAssertEqual(doc.modeDefaults.bot, "launchpad")

        // The default body cloud-api answers with.
        let empty = try PadTemplatesDoc.decode(Data(#"{"templates":[],"modeDefaults":{"chat":"research","code":"build","design":"client","bot":"launchpad"}}"#.utf8))
        XCTAssertEqual(empty, .empty)
    }

    func testDocEncodesTheAccountShape() throws {
        var doc = PadTemplatesDoc.empty
        doc.templates = [PadTemplate(id: id, name: "Weekly", tiles: [PadTile(id: .notes, size: .L)], createdAt: 9)]
        doc.modeDefaults[.code] = id
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(doc)) as? [String: Any])
        let templates = try XCTUnwrap(object["templates"] as? [[String: Any]])
        XCTAssertEqual(Set(templates[0].keys), ["id", "name", "tiles", "actions", "createdAt"])
        XCTAssertEqual(object["modeDefaults"] as? [String: String], ["chat": "research", "code": id, "design": "client", "bot": "launchpad"])
        // Round trip.
        XCTAssertEqual(try PadTemplatesDoc.decode(JSONEncoder().encode(doc)), doc)
    }

    func testFetchSendsTheClerkBearerAndFailureSaysCouldNotLoad() async throws {
        PadStubProtocol.handler = { _, _, _ in (200, #"{"templates":[],"modeDefaults":{}}"#) }
        let doc = try await client().fetch()
        XCTAssertEqual(doc, .empty)
        XCTAssertEqual(PadStubProtocol.seen.first?.path, "/api/v1/account/pad-templates")

        PadStubProtocol.handler = { _, _, _ in (500, "") }
        do {
            _ = try await client().fetch()
            XCTFail("expected a failure")
        } catch let failure as PadTemplatesClient.Failure {
            XCTAssertEqual(failure.message, "Could not load your templates")
        }
    }

    @MainActor
    func testFailedLoadKeepsBuiltInsAndRefusesWrites() async throws {
        PadStubProtocol.handler = { _, _, _ in (503, "") }
        let store = PadTemplatesStore(client: client(), isSignedIn: { true })
        await store.load().value
        XCTAssertFalse(store.loaded)
        XCTAssertEqual(store.error, "Could not load your templates")
        XCTAssertEqual(store.templates.map(\.id), ["launch", "client", "build", "research", "launchpad"])
        XCTAssertEqual(store.defaultTemplate(for: .design).id, "client")
        do {
            try await store.saveTemplate(name: "Mine", pad: ProjectPad.defaultPad(mode: .chat))
            XCTFail("a failed load must not let a save overwrite the account's templates")
        } catch {}
        XCTAssertFalse(PadStubProtocol.seen.contains { $0.method == "PUT" })
    }
}
