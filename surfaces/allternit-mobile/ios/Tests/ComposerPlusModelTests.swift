import XCTest
@testable import Allternit

/// The + sheet and chat ⋯ menu rules (phone calm pass step 22, contract C2):
/// which page the sheet opens on, the Computer row's value for each chip
/// state, and that the old + items stay reachable from the ⋯ menu.
final class ComposerPlusModelTests: XCTestCase {

    // MARK: - Opening page

    func testOpensOnTheGroupedListByDefault() {
        XCTAssertEqual(ComposerPlusPage.initial(openPermission: false, openComputer: false), .root)
        XCTAssertEqual(ComposerPlusPage.root.title, "Add to chat")
    }

    func testPermissionWinsOverComputer() {
        XCTAssertEqual(ComposerPlusPage.initial(openPermission: true, openComputer: true), .permission)
        XCTAssertEqual(ComposerPlusPage.initial(openPermission: false, openComputer: true), .computer)
    }

    func testPermissionPageNeedsAChatThatShowsIt() {
        // A bot thread has no permission choice: the sheet opens on the list.
        XCTAssertEqual(ComposerPlusPage.initial(openPermission: true, openComputer: false, permissionAvailable: false), .root)
    }

    // MARK: - Computer row value

    func testComputerRowNamesTheComputerWhenOneAnswers() {
        XCTAssertEqual(computerRowValue(.hidden, activeName: "Studio MacBook"), "Studio MacBook")
        XCTAssertEqual(computerRowValue(.hidden, activeName: nil), "Connected")
        XCTAssertEqual(computerRowValue(.hidden, activeName: ""), "Connected")
    }

    func testComputerRowReadsEachChipState() {
        let noComputer = computerChipState(ComputerChipInput(runtimeAvailable: false))
        let cantReach = computerChipState(ComputerChipInput(runtimeAvailable: false, paired: true))
        let waking = computerChipState(ComputerChipInput(waking: true, paired: true))
        let gate = computerChipState(ComputerChipInput(gate: true, runtimeAvailable: false))
        XCTAssertEqual(computerRowValue(noComputer, activeName: "Mac"), "No computer")
        XCTAssertEqual(computerRowValue(cantReach, activeName: "Mac"), "Can't reach")
        XCTAssertEqual(computerRowValue(waking, activeName: "Mac"), "Waking…")
        XCTAssertEqual(computerRowValue(gate, activeName: nil), "Connect")
    }

    func testComputerTextHasNoDigits() {
        let all = ["noComputer", "cantReach", "waking", "gate", "starting", "suspended", "runway"]
            .compactMap(computerChipFixture)
            .map { computerRowValue($0, activeName: nil) }
        XCTAssertEqual(all.count, 7)
        for value in all {
            XCTAssertNil(value.rangeOfCharacter(from: .decimalDigits), value)
        }
        for status in ["online", "busy", "offline", "error"] {
            XCTAssertNil(computerStatusLabel(status: status, wakeable: false).rangeOfCharacter(from: .decimalDigits))
        }
    }

    func testComputerStatusWords() {
        XCTAssertEqual(computerStatusLabel(status: "online", wakeable: false), "Online")
        XCTAssertEqual(computerStatusLabel(status: "offline", wakeable: false), "Offline")
        XCTAssertEqual(computerStatusLabel(status: "offline", wakeable: true), "Asleep · wakes when you send")
    }

    // MARK: - Chat ⋯ menu

    func testNewChatMenuKeepsTheItemsThatLeftThePlusSheet() {
        let items = chatMenuItems(ChatMenuInput(hasBrain: true))
        XCTAssertEqual(items, [.runOnPhone, .captureToBrain])
    }

    func testSavedChatAddsShareSaveAndArchive() {
        let items = chatMenuItems(ChatMenuInput(hasSession: true, hasMessages: true, pinned: false, hasBrain: true))
        XCTAssertEqual(items, [.runOnPhone, .captureToBrain, .shareLink, .pin, .archive])
        let pinned = chatMenuItems(ChatMenuInput(hasSession: true, hasMessages: true, pinned: true, hasBrain: false))
        XCTAssertEqual(pinned, [.runOnPhone, .shareLink, .unpin, .archive])
    }

    func testArtifactSessionHasNoPhoneRun() {
        let items = chatMenuItems(ChatMenuInput(hasSession: true, hasMessages: false, allowsPhoneRun: false))
        XCTAssertEqual(items, [.pin, .archive])
    }
}
