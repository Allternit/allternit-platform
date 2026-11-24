import XCTest
@testable import Allternit

/// The Swift agent-frame parser against the real captured screens shared with
/// the web and Android (surfaces/allternit-mobile/agent-frame-fixtures, copied
/// from allternit-ai src/lib/terminal/__fixtures__/agent-frame). Same steps as
/// agent-frame.test.ts: split on "\n", drop the one trailing empty piece, pad
/// to the 40-row viewport, compare with expected.json.
final class AgentFrameTests: XCTestCase {
    private static let viewportRows = 40

    private func fixturesDir() throws -> URL {
        try XCTUnwrap(Bundle(for: Self.self).url(forResource: "agent-frame-fixtures", withExtension: nil),
                      "agent-frame-fixtures is missing from the test bundle's resources")
    }

    /// Rows the way the terminal caller builds them: one string per viewport row.
    private func rows(of text: String) -> [String] {
        var body = text
        if body.hasSuffix("\n") { body.removeLast() }
        var rows = body.components(separatedBy: "\n")
        while rows.count < Self.viewportRows { rows.append("") }
        return rows
    }

    private func expected() throws -> [String: AgentFrame?] {
        let data = try Data(contentsOf: fixturesDir().appendingPathComponent("expected.json"))
        return try JSONDecoder().decode([String: AgentFrame?].self, from: data)
    }

    func testEveryFixtureHasAnExpectationAndBackAgain() throws {
        let files = try FileManager.default.contentsOfDirectory(atPath: fixturesDir().path)
            .filter { $0.hasSuffix(".txt") }
            .sorted()
        XCTAssertEqual(try expected().keys.sorted(), files)
        XCTAssertEqual(files.count, 34)
    }

    func testCapturedScreens() throws {
        let dir = try fixturesDir()
        for (file, want) in try expected().sorted(by: { $0.key < $1.key }) {
            let text = try String(contentsOf: dir.appendingPathComponent(file), encoding: .utf8)
            XCTAssertEqual(AgentFrameParser.parse(rows(of: text)), want, file)
        }
    }

    func testPlainShellScreenIsNotAnAgent() {
        let screen = ["joe@mac repo % ls", "README.md  package.json  src", "joe@mac repo % "].joined(separator: "\n")
        XCTAssertNil(AgentFrameParser.parse(rows(of: screen)))
    }

    /// The bytes the composer sends (agent-frame.ts agentPromptInput / agentDialogKeys).
    func testKeysForPromptAndAnswers() {
        let prompt = AgentKeys.promptInput("fix it\nplease")
        XCTAssertEqual(prompt.paste, "\u{1b}[200~fix it\nplease\u{1b}[201~")
        XCTAssertEqual(prompt.enter, "\r")

        let numbered = [AgentDialogOption(key: "1", label: "Yes", selected: true, index: 0),
                        AgentDialogOption(key: "2", label: "No", selected: false, index: 1)]
        XCTAssertEqual(AgentKeys.dialogKeys(numbered, pick: numbered[1]), "2")

        let arrows = [AgentDialogOption(key: "", label: "A", selected: false, index: 0),
                      AgentDialogOption(key: "", label: "B", selected: true, index: 1),
                      AgentDialogOption(key: "", label: "C", selected: false, index: 2)]
        XCTAssertEqual(AgentKeys.dialogKeys(arrows, pick: arrows[0]), "\u{1b}[A\r")
        XCTAssertEqual(AgentKeys.dialogKeys(arrows, pick: arrows[2]), "\u{1b}[B\r")
        XCTAssertEqual(AgentKeys.dialogKeys(arrows, pick: arrows[1]), "\r")
    }
}
