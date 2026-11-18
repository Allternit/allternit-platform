import XCTest
@testable import Allternit

final class AccountProjectsStubProtocol: URLProtocol {
    nonisolated(unsafe) static var handler: ((URLRequest) -> (Int, String))?
    nonisolated(unsafe) static var lastBody: Data?
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        if let stream = request.httpBodyStream {
            stream.open()
            var data = Data()
            var buffer = [UInt8](repeating: 0, count: 4096)
            while stream.hasBytesAvailable {
                let n = stream.read(&buffer, maxLength: buffer.count)
                if n <= 0 { break }
                data.append(buffer, count: n)
            }
            stream.close()
            Self.lastBody = data
        } else {
            Self.lastBody = request.httpBody
        }
        let (status, body) = Self.handler?(request) ?? (500, "")
        let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class AccountProjectsTests: XCTestCase {
    private func client() -> AccountProjectsClient {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [AccountProjectsStubProtocol.self]
        return AccountProjectsClient(baseURL: URL(string: "https://cloud.test")!, session: URLSession(configuration: config),
                                     tokenProvider: { "tok" })
    }

    private func project(_ id: String, kind: String = "chat", deleted: String? = nil) -> AccountProject {
        AccountProject(id: id, kind: kind, title: id, deletedAt: deleted)
    }

    // MARK: Client

    func testListDecodesBriefAndDropsTombstones() async throws {
        AccountProjectsStubProtocol.handler = { request in
            XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
            XCTAssertEqual(request.url?.path, "/api/v1/account-projects")
            XCTAssertEqual(request.url?.query, "kind=bot")
            return (200, #"{"projects":[{"id":"a","kind":"bot","title":"Scout","description":null,"instructions":"Watch prices","metadata":null,"adopted_computer_id":null,"created_at":"2026-11-18T10:00:00Z","updated_at":"2026-11-18T10:00:00Z","deleted_at":null},{"id":"b","kind":"bot","title":"Gone","instructions":"x","deleted_at":"2026-11-18T11:00:00Z"}]}"#)
        }
        let list = try await client().list(kind: .bot)
        XCTAssertEqual(list.map(\.id), ["a"])
        XCTAssertEqual(list.first?.brief, "Watch prices")
        XCTAssertEqual(list.first?.mode, .bot)
    }

    /// Rows written before the brief moved to `instructions`.
    func testOldRowWithStringMetadataBriefStillReads() throws {
        let json = #"{"id":"a","kind":"bot","title":"Scout","metadata":"{\"brief\":\"Watch prices\"}"}"#
        let decoded = try JSONDecoder().decode(AccountProject.self, from: Data(json.utf8))
        XCTAssertEqual(decoded.brief, "Watch prices")
    }

    func testRouteNotDeployedIsAnEmptyListNotAnError() async throws {
        AccountProjectsStubProtocol.handler = { _ in (404, #"{"error":"not found"}"#) }
        let list = try await client().list()
        XCTAssertTrue(list.isEmpty)
    }

    func testRealFailureThrows() async {
        AccountProjectsStubProtocol.handler = { _ in (500, #"{"error":"boom"}"#) }
        do { _ = try await client().list(); XCTFail("expected throw") } catch {}
    }

    /// cloud-api answers 400 to a string `metadata.brief`: a bot's brief
    /// goes in `instructions` and no metadata is sent.
    @MainActor
    func testCreateBotSendsBriefAsInstructions() async throws {
        AccountProjectsStubProtocol.handler = { request in
            XCTAssertEqual(request.httpMethod, "POST")
            XCTAssertEqual(request.url?.path, "/api/v1/account-projects")
            return (201, #"{"project":{"id":"new","kind":"bot","title":"Scout","instructions":"Watch prices"}}"#)
        }
        let store = ProjectsLibraryStore(defaults: UserDefaults(suiteName: "AccountProjectsTests")!)
        store.accountClient = client()
        let created = try await store.createAccountProject(kind: .bot, title: "Scout", description: nil, brief: "Watch prices")
        XCTAssertEqual(created.id, "new")
        XCTAssertEqual(created.brief, "Watch prices")
        XCTAssertEqual(store.accountProjects.map(\.id), ["new"])
        let body = try XCTUnwrap(AccountProjectsStubProtocol.lastBody)
        let sent = try XCTUnwrap(JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertEqual(sent["kind"] as? String, "bot")
        XCTAssertEqual(sent["instructions"] as? String, "Watch prices")
        XCTAssertNil(sent["metadata"])
    }


    // MARK: Merge

    func testComputerWinsByIdAndRepeatsAndTombstonesDrop() {
        let account = [project("shared"), project("mine"), project("mine"), project("old", deleted: "2026-11-18T00:00:00Z")]
        let shown = ProjectsLibraryStore.accountOnly(account, computerIds: ["shared"])
        XCTAssertEqual(shown.map(\.id), ["mine"])
    }

    func testAccountBotCardSaysItRunsLater() {
        XCTAssertEqual(ProjectsLibraryStore.accountCountLine(project("b", kind: "bot")), "Runs when a computer connects")
        XCTAssertEqual(ProjectsLibraryStore.accountCountLine(project("c")), "Saved to your account")
    }


    // MARK: No computer

    func testNoComputerFromStatusOrRefusedRead() {
        XCTAssertTrue(ProjectsLibraryStore.isNoComputer(computeState: "none", refused: false))
        XCTAssertTrue(ProjectsLibraryStore.isNoComputer(computeState: "paired", refused: true))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputer(computeState: "paired", refused: false))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputer(computeState: nil, refused: false))
    }

    func testGateRefusalAndRelayNoComputerAreNotErrors() {
        let gate = APIError.httpError(statusCode: 409, message: CalmCopy.computeRequired)
        XCTAssertTrue(ProjectsLibraryStore.isNoComputerFailure(gate))
        XCTAssertTrue(ProjectsLibraryStore.isNoComputerFailure(RuntimeRelayError.noComputer))
        XCTAssertTrue(ProjectsLibraryStore.isNoComputerFailure(APIError.transport(RuntimeRelayError.noComputer)))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputerFailure(APIError.httpError(statusCode: 500, message: "boom")))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputerFailure(URLError(.notConnectedToInternet)))

        XCTAssertTrue(ProjectsLibraryStore.isNoComputerMessage(gate.localizedDescription))
        XCTAssertTrue(ProjectsLibraryStore.isNoComputerMessage(RuntimeRelayError.noComputer.localizedDescription))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputerMessage("The Internet connection appears to be offline."))
        XCTAssertFalse(ProjectsLibraryStore.isNoComputerMessage(nil))
    }
}
