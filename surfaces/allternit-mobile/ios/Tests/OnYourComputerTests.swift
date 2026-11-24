import XCTest
@testable import Allternit

/// The "On your computer" rules (port of allternit-ai on-this-computer.ts) and
/// the terminal poll backoff (terminal-sessions.store.ts): which actions a row
/// offers, the sentence for every adopt error code, the command line shown
/// in the Move confirm, and when polling slows or stops.
final class OnYourComputerTests: XCTestCase {
    typealias A = OnYourComputer.RowAction

    // MARK: Action matrix

    func testActionMatrix() {
        XCTAssertEqual(OnYourComputer.rowActions("now", canCopy: true).actions, [A.move, .copy, .new])
        XCTAssertEqual(OnYourComputer.rowActions("now", canCopy: false).actions, [A.move, .new])
        XCTAssertEqual(OnYourComputer.rowActions("busy", canCopy: true).actions, [A.moveWhenIdle, .copy, .new])

        let elsewhere = OnYourComputer.rowActions("open_elsewhere", canCopy: true)
        XCTAssertEqual(elsewhere.actions, [A.copy, .new], "a session open twice never offers Move")
        XCTAssertTrue(elsewhere.openElsewhere)

        let noResume = OnYourComputer.rowActions("no_resume", canCopy: true)
        XCTAssertEqual(noResume.actions, [A.new])
        XCTAssertTrue(noResume.noResume)

        XCTAssertEqual(OnYourComputer.rowActions("here", canCopy: true).actions, [])
        XCTAssertEqual(OnYourComputer.rowActions(nil, canCopy: true).actions, [A.new], "an older runtime can still start fresh")
    }

    func testExternalSessionsSkipThoseAlreadyHere() {
        let rows = [
            LiveSession(sessionId: "a", external: true, movable: "now"),
            LiveSession(sessionId: "b", external: true, movable: "here"),
            LiveSession(sessionId: "c", external: false, movable: "now"),
        ]
        XCTAssertEqual(OnYourComputer.externalSessions(rows).map(\.sessionId), ["a"])
    }

    func testOpenElsewhereNamesEveryHolder() {
        let text = OnYourComputer.openElsewhereText([
            LiveSessionHolder(pid: 4821, app: "Terminal"),
            LiveSessionHolder(pid: 5110, app: "iTerm2"),
        ])
        XCTAssertEqual(text, "Open in 2 places: Terminal (pid 4821), iTerm2 (pid 5110). Close one to move it.")
    }

    // MARK: Command display

    func testCommandLineQuotesAnArgWithASpace() {
        let argv = ["claude", "--resume", "abc-123", "--append-system-prompt", "be brief", "it's"]
        XCTAssertEqual(OnYourComputer.moveCommandLine(argv),
                       "claude --resume abc-123 --append-system-prompt 'be brief' 'it'\\''s'")
        XCTAssertEqual(OnYourComputer.shellLine(["echo", ""]), "echo ''")
        XCTAssertNil(OnYourComputer.moveCommandLine(nil), "no moveCommand: the line is left out")
        XCTAssertNil(OnYourComputer.moveCommandLine([]))
    }

    func testFlagChipsKeepValuesWithTheirFlag() {
        XCTAssertEqual(OnYourComputer.flagChips(["--model", "opus", "--dangerously-skip-permissions"]),
                       ["--model opus", "--dangerously-skip-permissions"])
    }

    // MARK: Move errors

    func testMoveErrorTextForEveryCode() {
        let expected: [String: String] = [
            "session_open_elsewhere": "This session is open in more than one place. Close all but one, then move it.",
            "already_here": "This session already runs in an Allternit terminal.",
            "adopt_in_progress": "This session is already being moved or opened. Wait for that to finish.",
            "holder_changed": "The process holding this session changed, so nothing was stopped. Try again.",
            "holder_unverified": "Allternit couldn't confirm which process holds this session, so nothing was stopped.",
            "no_holder": "No running process holds this session, so there is nothing to move.",
            "still_busy": "It was still working after 5 minutes. Nothing was stopped; try again when it is idle.",
            "did_not_exit": "It didn't exit when asked to stop. Nothing was started; quit it in its own window and try again.",
            "session_gone": "It stopped running before it was moved. Nothing was started.",
            "too_late": OnYourComputer.tooLateNotice,
            "stop_refused": "Allternit couldn't stop its process. Nothing was started.",
            "no_resume": "This CLI can't reopen a session by id. Use New session here instead.",
            "no_copy": "This CLI can't open a copy of a session.",
            "no_cwd": "This session's folder isn't known, so it can't open in the same place.",
            "start_failed": "The new terminal didn't start.",
            "unsupported": "Move isn't available on this computer's system yet. Nothing was stopped.",
        ]
        for (code, text) in expected {
            XCTAssertEqual(OnYourComputer.moveErrorText(code: code).text, text, code)
            XCTAssertNil(OnYourComputer.moveErrorText(code: code).command, code)
        }
        XCTAssertEqual(OnYourComputer.moveErrorText(code: "weird", message: "  Disk full  ").text, "Disk full")
        XCTAssertEqual(OnYourComputer.moveErrorText(code: nil).text,
                       "Something went wrong. Check the session in its own window before trying again.")
    }

    func testStartFailedCarriesTheCommandToRunByHand() {
        let message = "Terminal did not start. Run it by hand in /Users/me/app: claude --resume 'abc 1'\n"
        let failure = OnYourComputer.moveErrorText(code: "start_failed", message: message)
        XCTAssertEqual(failure.text, "The new terminal didn't start. Run this by hand:")
        XCTAssertEqual(failure.cwd, "/Users/me/app")
        XCTAssertEqual(failure.command, "claude --resume 'abc 1'")

        let gone = OnYourComputer.moveErrorText(code: "session_gone", message: "Gone. Resume it in ~/x: codex resume 9")
        XCTAssertEqual(gone.text, "It stopped running before it was moved. Resume it by hand:")
        XCTAssertEqual(gone.command, "codex resume 9")
        XCTAssertEqual(gone.cwd, "~/x")

        XCTAssertEqual(OnYourComputer.moveErrorText(code: "start_failed", message: "spawn failed").text, "spawn failed")
    }

    // MARK: Poll backoff

    func testPollBackoff() {
        XCTAssertEqual(TerminalPoll.nextDelay(base: TerminalPoll.terminalSeconds, failures: 0), 2)
        XCTAssertEqual(TerminalPoll.nextDelay(base: TerminalPoll.terminalSeconds, failures: 2), 2)
        XCTAssertEqual(TerminalPoll.nextDelay(base: TerminalPoll.terminalSeconds, failures: 3), 15)
        XCTAssertEqual(TerminalPoll.nextDelay(base: TerminalPoll.liveSessionSeconds, failures: 3), 15)
        XCTAssertEqual(TerminalPoll.nextDelay(base: 30, failures: 5), 30, "backoff never speeds a slow endpoint up")

        XCTAssertTrue(TerminalPoll.shouldPoll(onScreen: true, active: true, support: .unknown))
        XCTAssertFalse(TerminalPoll.shouldPoll(onScreen: true, active: true, support: .no), "a 404 stops the endpoint")
        XCTAssertFalse(TerminalPoll.shouldPoll(onScreen: true, active: false, support: .yes), "the app in the background")
        XCTAssertFalse(TerminalPoll.shouldPoll(onScreen: false, active: true, support: .yes))
    }

    // MARK: Decoding

    func testLiveSessionDecodesTheContract() throws {
        let json = """
        [{"address":"claude@mac","name":"fix login","harness":"claude","machine":"m","machineName":"Mac",
          "sessionId":"s1","cwd":"/Users/me/app","status":"idle","since":"x","tier":"t","unread":0,
          "holders":[{"pid":4821,"app":"Terminal","tty":"/dev/ttys003"}],
          "carriedFlags":["--model","opus"],"canCopy":true,"external":true,"movable":"now",
          "moveCommand":["claude","--resume","s1"],
          "move":{"jobId":"j1","state":"stopping"}}]
        """
        let rows = try JSONDecoder().decode([LiveSession].self, from: Data(json.utf8))
        XCTAssertEqual(rows.first?.holders.first?.app, "Terminal")
        XCTAssertEqual(rows.first?.move?.state, "stopping")
        XCTAssertEqual(rows.first?.moveCommand, ["claude", "--resume", "s1"])
        XCTAssertTrue(rows.first?.canCopy == true)
    }
}
