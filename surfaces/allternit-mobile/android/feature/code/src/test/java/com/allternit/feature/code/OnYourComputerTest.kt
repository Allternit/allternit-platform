package com.allternit.feature.code

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The "On your computer" rules (port of iOS OnYourComputerTests and allternit-ai on-this-computer.ts) and
 * the poll backoff (terminal-sessions.store.ts): which actions a row offers, the sentence for every adopt
 * error code, the command line shown in the Move confirm, when polling slows or stops, and the wire shapes.
 */
class OnYourComputerTest {
    private val move = OnYourComputer.RowAction.Move
    private val moveWhenIdle = OnYourComputer.RowAction.MoveWhenIdle
    private val copy = OnYourComputer.RowAction.Copy
    private val new = OnYourComputer.RowAction.New

    // MARK: Action matrix

    @Test
    fun actionMatrix() {
        assertEquals(listOf(move, copy, new), OnYourComputer.rowActions("now", canCopy = true).actions)
        assertEquals(listOf(move, new), OnYourComputer.rowActions("now", canCopy = false).actions)
        assertEquals(listOf(moveWhenIdle, copy, new), OnYourComputer.rowActions("busy", canCopy = true).actions)

        val elsewhere = OnYourComputer.rowActions("open_elsewhere", canCopy = true)
        assertEquals("a session open twice never offers Move", listOf(copy, new), elsewhere.actions)
        assertTrue(elsewhere.openElsewhere)

        val noResume = OnYourComputer.rowActions("no_resume", canCopy = true)
        assertEquals(listOf(new), noResume.actions)
        assertTrue(noResume.noResume)

        assertEquals(emptyList<OnYourComputer.RowAction>(), OnYourComputer.rowActions("here", canCopy = true).actions)
        assertEquals("an older runtime can still start fresh", listOf(new), OnYourComputer.rowActions(null, canCopy = true).actions)
        assertEquals(listOf("move", "move", "copy", "new"), listOf(move, moveWhenIdle, copy, new).map { it.mode })
    }

    @Test
    fun externalSessionsSkipThoseAlreadyHere() {
        val rows = listOf(
            LiveSession("a", external = true, movable = "now"),
            LiveSession("b", external = true, movable = "here"),
            LiveSession("c", external = false, movable = "now"),
        )
        assertEquals(listOf("a"), OnYourComputer.externalSessions(rows).map { it.sessionId })
    }

    @Test
    fun aRowWithAJobStaysAfterItsSessionLeavesTheList() {
        // start_failed after the old process stopped: the session is gone from live-sessions, its error is not.
        val listed = listOf(LiveSession("a", external = true, movable = "now"))
        val jobs = mapOf("a" to LiveSession("a", external = true), "gone" to LiveSession("gone", external = true))
        assertEquals(listOf("a", "gone"), OnYourComputer.rowsWithJobs(listed, jobs).map { it.sessionId })
    }

    @Test
    fun openElsewhereNamesEveryHolder() {
        val text = OnYourComputer.openElsewhereText(listOf(LiveSessionHolder(4821, "Terminal"), LiveSessionHolder(5110, "iTerm2")))
        assertEquals("Open in 2 places: Terminal (pid 4821), iTerm2 (pid 5110). Close one to move it.", text)
    }

    @Test
    fun confirmTextSaysTheOldWindowStaysOpen() {
        assertEquals(
            "Allternit stops it in Terminal (pid 4821) and resumes it here. The Terminal window stays open and shows the exit.",
            OnYourComputer.confirmText("Terminal", "Terminal (pid 4821)", whenIdle = false),
        )
        assertTrue(OnYourComputer.confirmText("iTerm2", null, whenIdle = true).startsWith("When it goes idle, Allternit stops it in iTerm2"))
    }

    // MARK: Command display

    @Test
    fun commandLineQuotesAnArgWithASpace() {
        val argv = listOf("claude", "--resume", "abc-123", "--append-system-prompt", "be brief", "it's")
        assertEquals("claude --resume abc-123 --append-system-prompt 'be brief' 'it'\\''s'", OnYourComputer.moveCommandLine(argv))
        assertEquals("echo ''", OnYourComputer.shellLine(listOf("echo", "")))
        assertEquals("a '\$HOME' 'x;y'", OnYourComputer.shellLine(listOf("a", "\$HOME", "x;y")))
        assertNull("no moveCommand: the line is left out", OnYourComputer.moveCommandLine(null))
        assertNull(OnYourComputer.moveCommandLine(emptyList()))
    }

    @Test
    fun flagChipsKeepValuesWithTheirFlag() {
        assertEquals(
            listOf("--model opus", "--dangerously-skip-permissions"),
            OnYourComputer.flagChips(listOf("--model", "opus", "--dangerously-skip-permissions")),
        )
    }

    // MARK: Move errors

    @Test
    fun moveErrorTextForEveryCode() {
        val expected = mapOf(
            "session_open_elsewhere" to "This session is open in more than one place. Close all but one, then move it.",
            "already_here" to "This session already runs in an Allternit terminal.",
            "adopt_in_progress" to "This session is already being moved or opened. Wait for that to finish.",
            "holder_changed" to "The process holding this session changed, so nothing was stopped. Try again.",
            "holder_unverified" to "Allternit couldn't confirm which process holds this session, so nothing was stopped.",
            "no_holder" to "No running process holds this session, so there is nothing to move.",
            "not_movable" to "This session can't move right now.",
            "still_busy" to "It was still working after 5 minutes. Nothing was stopped; try again when it is idle.",
            "did_not_exit" to "It didn't exit when asked to stop. Nothing was started; quit it in its own window and try again.",
            "session_gone" to "It stopped running before it was moved. Nothing was started.",
            "too_late" to OnYourComputer.TOO_LATE_NOTICE,
            "stop_refused" to "Allternit couldn't stop its process. Nothing was started.",
            "cancelled" to "Move cancelled. Nothing was stopped.",
            "no_resume" to "This CLI can't reopen a session by id. Use New session here instead.",
            "no_copy" to "This CLI can't open a copy of a session.",
            "no_cwd" to "This session's folder isn't known, so it can't open in the same place.",
            "start_failed" to "The new terminal didn't start.",
            "unsupported" to "Move isn't available on this computer's system yet. Nothing was stopped.",
            "unknown_harness" to "Allternit doesn't know how to reopen this CLI.",
            "unreachable" to "Couldn't reach this computer. Check the session in its own window before trying again.",
        )
        for ((code, text) in expected) {
            assertEquals(code, text, OnYourComputer.moveErrorText(code).text)
            assertNull(code, OnYourComputer.moveErrorText(code).command)
        }
        assertEquals("Disk full", OnYourComputer.moveErrorText("weird", "  Disk full  ").text)
        assertEquals("Something went wrong. Check the session in its own window before trying again.", OnYourComputer.moveErrorText(null).text)
    }

    @Test
    fun startFailedCarriesTheCommandToRunByHand() {
        val failure = OnYourComputer.moveErrorText("start_failed", "Terminal did not start. Run it by hand in /Users/me/app: claude --resume 'abc 1'\n")
        assertEquals("The new terminal didn't start. Run this by hand:", failure.text)
        assertEquals("/Users/me/app", failure.cwd)
        assertEquals("claude --resume 'abc 1'", failure.command)

        val gone = OnYourComputer.moveErrorText("session_gone", "Gone. Resume it in ~/x: codex resume 9")
        assertEquals("It stopped running before it was moved. Resume it by hand:", gone.text)
        assertEquals("codex resume 9", gone.command)
        assertEquals("~/x", gone.cwd)

        assertEquals("spawn failed", OnYourComputer.moveErrorText("start_failed", "spawn failed").text)
    }

    // MARK: Poll backoff

    @Test
    fun pollBackoff() {
        assertEquals(2_000L, TerminalPoll.nextDelayMs(TerminalPoll.TERMINAL_MS, 0))
        assertEquals(2_000L, TerminalPoll.nextDelayMs(TerminalPoll.TERMINAL_MS, 2))
        assertEquals(15_000L, TerminalPoll.nextDelayMs(TerminalPoll.TERMINAL_MS, 3))
        assertEquals(15_000L, TerminalPoll.nextDelayMs(TerminalPoll.LIVE_SESSION_MS, 3))
        assertEquals(5_000L, TerminalPoll.nextDelayMs(TerminalPoll.LIVE_SESSION_MS, 0))
        assertEquals("backoff never speeds a slow endpoint up", 30_000L, TerminalPoll.nextDelayMs(30_000L, 5))

        assertTrue(TerminalPoll.shouldPoll(onScreen = true, active = true, support = TerminalPoll.Support.Unknown))
        assertFalse("a 404 stops the endpoint", TerminalPoll.shouldPoll(onScreen = true, active = true, support = TerminalPoll.Support.No))
        assertFalse("the app in the background", TerminalPoll.shouldPoll(onScreen = true, active = false, support = TerminalPoll.Support.Yes))
        assertFalse(TerminalPoll.shouldPoll(onScreen = false, active = true, support = TerminalPoll.Support.Yes))
    }

    // MARK: Previews

    @Test
    fun previewDropsBlankEndsAndKeepsTheLastRows() {
        val rows = listOf("", "one  ", "two", "three", "", "")
        assertEquals(listOf("two", "three"), TerminalPreview.rows(rows, max = 2))
        assertEquals(listOf("one", "two", "three"), TerminalPreview.rows(rows))
        assertEquals(emptyList<String>(), TerminalPreview.rows(null))
        assertEquals(listOf(2, 5, 8), TerminalPreview.Layout.entries.map(TerminalPreview::rowCount))
    }

    @Test
    fun aPlainShellIsRunningOrExited() {
        assertEquals("Running", TerminalPreview.status(TerminalListItem("t", preview = listOf("$ ls"))).word)
        assertEquals(TerminalPreview.Tone.Done, TerminalPreview.status(TerminalListItem("t", running = false)).tone)
        assertEquals("app", TerminalPreview.title(TerminalListItem("t", cwd = "/Users/me/app/")))
    }

    // MARK: Wire

    @Test
    fun liveSessionReadsTheContract() {
        val body = Json.parseToJsonElement(
            """
            [{"address":"claude@mac","name":"fix login","harness":"claude","machine":"m","machineName":"Mac",
              "sessionId":"s1","cwd":"/Users/me/app","status":"idle","since":"x","unread":0,"pid":4821,
              "holders":[{"pid":4821,"app":"Terminal","tty":"/dev/ttys003"}],
              "carriedFlags":["--model","opus"],"canCopy":true,"external":true,"movable":"now",
              "moveCommand":["claude","--resume","s1"],
              "move":{"jobId":"j1","state":"stopping"}},
             {"name":"no id"}]
            """,
        )
        val rows = LiveWire.sessions(body)
        assertEquals("a row without a session id is skipped", 1, rows.size)
        val s = rows.single()
        assertEquals("Terminal", s.holders.single().app)
        assertEquals(4821, s.pid)
        assertEquals("stopping", s.move?.state)
        assertEquals(listOf("claude", "--resume", "s1"), s.moveCommand)
        assertTrue(s.canCopy && s.external)
        assertNull(s.copyCommand)
    }

    @Test
    fun terminalListReadsPreviews() {
        val body = Json.parseToJsonElement(
            """{"terminals":[{"session_id":"t1","pane_id":"%3","title":"zsh","cwd":"/x","command":["zsh"],"running":false,"exit_code":1,"pid":9,"created_at":"c","preview":["a","b"]},
                {"session_id":"t1"},{"title":"no id"}]}""",
        )
        val rows = LiveWire.terminals(body)
        assertEquals("one row per id", 1, rows.size)
        assertEquals(TerminalListItem("t1", "%3", "zsh", "/x", listOf("zsh"), false, 1, 9, "c", listOf("a", "b")), rows.single())
    }

    @Test
    fun adoptReplyNeedsAJobForAMoveAndATerminalOtherwise() {
        val job = Json.parseToJsonElement("""{"jobId":"j1","state":"waiting_idle"}""") as JsonObject
        assertEquals(LiveCall.Ok(AdoptStart.Job(AdoptJob("j1", "waiting_idle"))), LiveWire.adoptReply("move", job))
        assertEquals("bad_response", (LiveWire.adoptReply("move", JsonObject(emptyMap())) as LiveCall.Failed).code)
        val terminal = Json.parseToJsonElement("""{"terminalId":"t9"}""") as JsonObject
        assertEquals(LiveCall.Ok(AdoptStart.Terminal("t9")), LiveWire.adoptReply("copy", terminal))
        assertEquals("bad_response", (LiveWire.adoptReply("new", null) as LiveCall.Failed).code)
        assertEquals(LiveCall.Ok(AdoptStart.Cancelled), LiveWire.adoptReply("cancel", null))
    }
    // MARK: Factory panes (blocked, Open, steer, push)

    private val blockedJson = """{"address":"dev/allternit-claude","name":"allternit-claude","harness":"claude","machine":"m_4f2a",
        "machineName":"Dev MacBook","sessionId":"ao-pane-w3:p1","paneId":"w3:p1","cwd":"allternit","repo":"Allternit/allternit-platform","branch":"main",
        "status":"blocked","since":"2026-11-27T09:00:00+00:00","blockedSince":"2026-11-27T09:12:30+00:00","terminalId":"term_7c1e9a40","steer":"terminal",
        "tier":"push","unread":0,"holders":[],"carriedFlags":[],"canCopy":false,"movable":"here","external":false}"""
    private val legacyJson = """{"address":"dev/allternit-gizzi","name":"allternit-gizzi","harness":"gizzi","machine":"m_4f2a","machineName":"Dev MacBook",
        "sessionId":"ao-build","paneId":"w1:p1","cwd":"allternit","status":"idle","since":"2026-11-27T09:00:00+00:00","tier":"push","unread":0,
        "holders":[],"carriedFlags":[],"canCopy":false,"movable":"here","external":false}"""

    private fun decode(json: String): LiveSession = LiveWire.session(Json.parseToJsonElement(json) as JsonObject)!!

    @Test
    fun blockedPaneFixtureDecodesAndShowsAsARow() {
        val s = decode(blockedJson)
        assertEquals("blocked", s.status)
        assertEquals("term_7c1e9a40", s.terminalId)
        assertEquals("terminal", s.steer)
        assertEquals("2026-11-27T09:12:30+00:00", s.blockedSince)
        assertTrue(OnYourComputer.isBlocked(s))
        assertEquals("Needs you", OnYourComputer.statusWord(s.status))
        assertEquals(listOf(s), OnYourComputer.rowSessions(listOf(s)))
        assertTrue(OnYourComputer.canSteer(s))
    }

    @Test
    fun legacyPaneFixtureAddsNothing() {
        val s = decode(legacyJson)
        assertNull(s.terminalId)
        assertNull(s.steer)
        assertFalse(OnYourComputer.isBlocked(s))
        assertFalse(OnYourComputer.canSteer(s))
        assertEquals("an older runtime shows no new rows", emptyList<LiveSession>(), OnYourComputer.rowSessions(listOf(s)))
        assertEquals("Idle", OnYourComputer.statusWord(s.status))
    }

    @Test
    fun steerIsHiddenForNoneAndNeedsItsTarget() {
        val base = decode(blockedJson)
        assertFalse(OnYourComputer.canSteer(base.copy(steer = "none")))
        assertFalse(OnYourComputer.canSteer(base.copy(steer = "terminal", terminalId = null)))
        assertTrue(OnYourComputer.canSteer(base.copy(steer = "delivery")))
        assertFalse(OnYourComputer.canSteer(base.copy(steer = "delivery", address = "")))
        assertFalse(OnYourComputer.canSteer(base.copy(steer = null)))
        // A blocked pane with steer none is still a row (the badge shows), without Steer.
        assertEquals(1, OnYourComputer.rowSessions(listOf(base.copy(steer = "none", terminalId = null))).size)
    }

    @Test
    fun blockedLabelCountsFromBlockedSince() {
        val since = "2026-11-27T09:12:30+00:00"
        val t0 = java.time.OffsetDateTime.parse(since).toInstant().toEpochMilli()
        assertEquals("blocked just now", OnYourComputer.blockedLabel(since, t0 + 30_000))
        assertEquals("blocked 3m", OnYourComputer.blockedLabel(since, t0 + 3 * 60_000 + 5_000))
        assertEquals("blocked 5h", OnYourComputer.blockedLabel(since, t0 + 5 * 3_600_000L))
        assertEquals("blocked 3d", OnYourComputer.blockedLabel(since, t0 + 3 * 86_400_000L))
        assertEquals("blocked", OnYourComputer.blockedLabel(null, t0))
        assertEquals("blocked", OnYourComputer.blockedLabel("soon", t0))
    }

    @Test
    fun pushTargetReadsTheTerminalOrTheFactoryAddress() {
        assertEquals(OnYourComputer.PushTarget.Terminal("term_7c1e9a40"), OnYourComputer.pushTarget("/terminal/term_7c1e9a40"))
        assertEquals(OnYourComputer.PushTarget.FactorySession("dev/allternit-claude"), OnYourComputer.pushTarget("/factory/sessions/dev/allternit-claude"))
        assertEquals(OnYourComputer.PushTarget.FactorySession("dev/allternit-claude"), OnYourComputer.pushTarget("/factory/sessions/dev%2Fallternit-claude?x=1"))
        assertNull(OnYourComputer.pushTarget("/computers/c1"))
        assertNull(OnYourComputer.pushTarget("/terminal/"))
        assertNull(OnYourComputer.pushTarget(null))
    }
}
