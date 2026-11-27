import XCTest
@testable import Allternit

final class SkillResultTests: XCTestCase {
    private func text(_ body: String, args: String? = nil) -> String {
        "Skill nimbrook:triage from Nimbrook. This text comes from that server.\n" + body
            + (args.map { "\n\nARGUMENTS: \($0)" } ?? "") + "\nTrack these steps with your todo list as you work."
    }

    func testReadsIdAndFromFromLineOne() {
        let r = SkillResult.parse(text("1. Read the ticket"))
        XCTAssertEqual(r?.id, "nimbrook:triage")
        XCTAssertEqual(r?.from, "Nimbrook")
    }

    func testNestedListAndMarkdownAreHandled() {
        let r = SkillResult.parse(text("1. **Read** the [ticket](http://x)\n   1. nested\n2) Reply"))
        XCTAssertEqual(r?.steps, ["Read the ticket", "Reply"])
    }

    func testArgumentsCutsTheList() {
        let r = SkillResult.parse(text("1. One", args: "2. not a step"))
        XCTAssertEqual(r?.steps, ["One"])
    }

    func testMoreThanTwentyStepsAreCut() {
        let many = (1...25).map { "\($0). step \($0)" }.joined(separator: "\n")
        XCTAssertEqual(SkillResult.parse(text(many))?.steps.count, 20)
    }

    func testLoadSkillToolNames() {
        XCTAssertTrue(SkillResult.isLoadSkill("load_skill"))
        XCTAssertTrue(SkillResult.isLoadSkill("nimbrook_load_skill"))
        XCTAssertFalse(SkillResult.isLoadSkill("load_skills"))
    }
}
