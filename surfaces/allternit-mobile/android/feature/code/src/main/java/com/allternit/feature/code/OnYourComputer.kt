package com.allternit.feature.code

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/*
 * "On your computer" and the terminal previews in the Code sessions view: a port of iOS
 * Features/Code/OnYourComputer.swift (itself a port of allternit-ai on-this-computer.ts,
 * terminal-overview.ts and terminal-sessions.store.ts). Pure rules first, then the store.
 */

// MARK: - Rules (pure)

object OnYourComputer {
    enum class RowAction(val label: String, val mode: String) {
        Move("Move here", "move"),
        MoveWhenIdle("Move when idle", "move"),
        Copy("Open a copy", "copy"),
        New("New session here", "new");

        val isMove: Boolean get() = mode == "move"
    }

    /**
     * The actions a row offers. [openElsewhere]: Move is refused while more than one process holds the
     * session. [noResume]: the CLI can't reopen a session by id.
     */
    data class ActionSet(val actions: List<RowAction>, val openElsewhere: Boolean = false, val noResume: Boolean = false)

    /** The actions a row offers, from the runtime's `movable` and `canCopy`. `here` offers none (it is already an Allternit terminal). */
    fun rowActions(movable: String?, canCopy: Boolean): ActionSet {
        val copy = if (canCopy) listOf(RowAction.Copy) else emptyList()
        return when (movable) {
            "now" -> ActionSet(listOf(RowAction.Move) + copy + RowAction.New)
            "busy" -> ActionSet(listOf(RowAction.MoveWhenIdle) + copy + RowAction.New)
            "open_elsewhere" -> ActionSet(copy + RowAction.New, openElsewhere = true)
            "no_resume" -> ActionSet(listOf(RowAction.New), noResume = true)
            "here" -> ActionSet(emptyList())
            // A runtime that sends no `movable` can still start a fresh session in the folder.
            else -> ActionSet(listOf(RowAction.New))
        }
    }

    /** Sessions running outside Allternit that can be brought in. */
    fun externalSessions(sessions: List<LiveSession>): List<LiveSession> = sessions.filter { it.external && it.movable != "here" }

    /**
     * The rows to show: the external sessions, then any session a Move or adopt started from that has left
     * the list (its process stopped) while its progress or error still needs showing.
     */
    fun rowsWithJobs(sessions: List<LiveSession>, jobSessions: Map<String, LiveSession>): List<LiveSession> {
        val shown = externalSessions(sessions)
        val listed = sessions.mapTo(HashSet()) { it.sessionId }
        return shown + jobSessions.values.filter { it.sessionId !in listed }
    }

    private const val PLAIN_ARG = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_./:=@+,"

    /** One argv as a shell line: an arg with a space or a shell character is single-quoted. */
    fun shellLine(argv: List<String>): String = argv.joinToString(" ") { arg ->
        if (arg.isNotEmpty() && arg.all { it in PLAIN_ARG }) arg else "'" + arg.replace("'", "'\\''") + "'"
    }

    /** The command a Move runs, from the runtime's own argv (`moveCommand`), quoted for display. Null on a runtime that sends none. */
    fun moveCommandLine(argv: List<String>?): String? = argv?.takeIf { it.isNotEmpty() }?.let(::shellLine)

    /** Carried flags as chips: a flag with its values is one chip (`--model opus`). */
    fun flagChips(flags: List<String>): List<String> {
        val chips = ArrayList<String>()
        for (flag in flags) {
            if (flag.startsWith("-") || chips.isEmpty()) chips.add(flag) else chips[chips.lastIndex] = chips.last() + " " + flag
        }
        return chips
    }

    /** "Terminal (pid 4821)". */
    fun holderLabel(app: String, pid: Int): String = "$app (pid $pid)"

    /** "Terminal · pid 4821" for the row's detail line. */
    fun appLine(session: LiveSession): String? =
        session.holders.firstOrNull()?.let { "${it.app} · pid ${it.pid}" } ?: session.pid?.let { "pid $it" }

    /** The one-writer warning, naming every holder. */
    fun openElsewhereText(holders: List<LiveSessionHolder>): String {
        if (holders.size < 2) return "Another process may hold this session. Close it to move the session."
        val names = holders.joinToString(", ") { holderLabel(it.app, it.pid) }
        return "Open in ${holders.size} places: $names. Close one to move it."
    }

    fun statusWord(status: String): String = when (status) {
        "busy" -> "Working"
        "idle" -> "Idle"
        "offline" -> "Offline"
        else -> status
    }

    /** The row's progress line while a Move job runs. */
    fun jobStateText(state: String): String? = when (state) {
        "waiting_idle" -> "Waiting for it to go idle…"
        "stopping" -> "Stopping…"
        "starting" -> "Starting…"
        else -> null
    }

    /** Shown while a move that could no longer be cancelled finishes. */
    const val TOO_LATE_NOTICE = "Too late to cancel; finishing the move"

    /** The confirm sheet's sentence about the old window. */
    fun confirmText(app: String?, holder: String?, whenIdle: Boolean): String {
        val lead = if (whenIdle) "When it goes idle, Allternit" else "Allternit"
        val where = holder ?: app ?: "its window"
        return "$lead stops it in $where and resumes it here. The ${app ?: "old"} window stays open and shows the exit."
    }

    /** A failure's sentence, plus the command to run by hand (start_failed, session_gone) and the folder to run it in. */
    data class MoveErrorText(val text: String, val command: String? = null, val cwd: String? = null)

    private val BY_HAND = Regex("(?:Run it by hand|Resume it) in (.*?): (.+)\\z", RegexOption.DOT_MATCHES_ALL)

    /** Pulls the "Run it by hand in <cwd>: <command>" (or "Resume it in …") tail out of a runtime message. */
    private fun commandFrom(message: String?): Pair<String, String>? {
        val m = message?.let { BY_HAND.find(it) } ?: return null
        return m.groupValues[1] to m.groupValues[2].trim()
    }

    /** One plain sentence per adopt error code. Unknown codes fall back to the runtime's own words. */
    fun moveErrorText(code: String?, message: String? = null): MoveErrorText {
        val own = message?.trim()?.takeIf { it.isNotEmpty() }
        return when (code) {
            "session_open_elsewhere" -> MoveErrorText("This session is open in more than one place. Close all but one, then move it.")
            "already_here" -> MoveErrorText("This session already runs in an Allternit terminal.")
            "adopt_in_progress" -> MoveErrorText("This session is already being moved or opened. Wait for that to finish.")
            "holder_changed" -> MoveErrorText("The process holding this session changed, so nothing was stopped. Try again.")
            "holder_unverified" -> MoveErrorText("Allternit couldn't confirm which process holds this session, so nothing was stopped.")
            "no_holder" -> MoveErrorText("No running process holds this session, so there is nothing to move.")
            "not_movable" -> MoveErrorText("This session can't move right now.")
            "still_busy" -> MoveErrorText("It was still working after 5 minutes. Nothing was stopped; try again when it is idle.")
            "did_not_exit" -> MoveErrorText("It didn't exit when asked to stop. Nothing was started; quit it in its own window and try again.")
            "session_gone" -> commandFrom(message)
                ?.let { (cwd, command) -> MoveErrorText("It stopped running before it was moved. Resume it by hand:", command, cwd) }
                ?: MoveErrorText("It stopped running before it was moved. Nothing was started.")
            "too_late" -> MoveErrorText(TOO_LATE_NOTICE)
            "stop_refused" -> MoveErrorText("Allternit couldn't stop its process. Nothing was started.")
            "cancelled" -> MoveErrorText("Move cancelled. Nothing was stopped.")
            "no_resume" -> MoveErrorText("This CLI can't reopen a session by id. Use New session here instead.")
            "no_copy" -> MoveErrorText("This CLI can't open a copy of a session.")
            "no_cwd" -> MoveErrorText("This session's folder isn't known, so it can't open in the same place.")
            "start_failed" -> commandFrom(message)
                ?.let { (cwd, command) -> MoveErrorText("The new terminal didn't start. Run this by hand:", command, cwd) }
                ?: MoveErrorText(own ?: "The new terminal didn't start.")
            "unsupported" -> MoveErrorText("Move isn't available on this computer's system yet. Nothing was stopped.")
            "unknown_harness" -> MoveErrorText("Allternit doesn't know how to reopen this CLI.")
            "unreachable" -> MoveErrorText("Couldn't reach this computer. Check the session in its own window before trying again.")
            else -> MoveErrorText(own ?: "Something went wrong. Check the session in its own window before trying again.")
        }
    }

    private val HARNESS_NAMES = mapOf(
        "claude" to "Claude Code", "codex" to "Codex", "kimi" to "Kimi", "agy" to "Antigravity", "antigravity" to "Antigravity",
        "gizzi" to "Gizzi", "opencode" to "opencode", "grok" to "Grok", "gemini" to "Gemini CLI", "cursor" to "Cursor",
    )

    /** "claude" → "Claude Code". */
    fun harnessName(harness: String): String = HARNESS_NAMES[harness.lowercase()] ?: harness.ifEmpty { "Agent" }

    /** The last path segment of a folder ("~/code/app" → "app"). */
    fun basename(path: String): String = path.split('/').lastOrNull { it.isNotEmpty() } ?: if (path == "/") "/" else ""

    /** The row's name: the session's own, else its folder, else the CLI. */
    fun displayName(session: LiveSession): String {
        session.name.trim().takeIf { it.isNotEmpty() }?.let { return it }
        return basename(session.cwd).ifEmpty { harnessName(session.harness) }
    }
}

/** Terminal preview rules (terminal-overview.ts, iOS TerminalPreview). */
object TerminalPreview {
    /** Rows asked of the runtime per terminal (web TERMINAL_PREVIEW_ROWS). */
    const val REQUEST_ROWS = 12

    /** The sessions view layouts the web and iOS have. Android's Code threads screen is a single list. */
    enum class Layout { List, Grid, Deck }

    /** Rows a preview shows per layout on a phone: List 2, Grid 5, Deck 8. */
    fun rowCount(layout: Layout): Int = when (layout) {
        Layout.List -> 2
        Layout.Grid -> 5
        Layout.Deck -> 8
    }

    /**
     * The screen rows a preview shows: the rows above the agent's own input box (the same crop the live
     * terminal uses), blank rows at both ends dropped, the last [max] rows kept, trailing spaces trimmed.
     */
    fun rows(rows: List<String>?, title: String = "", max: Int = REQUEST_ROWS): List<String> {
        if (rows.isNullOrEmpty()) return emptyList()
        val frame = AgentFrameParser.parse(rows, title)
        val kept = frame?.let { rows.take(it.hideFrom.coerceAtLeast(0)) } ?: rows
        val shown = kept.map { it.trimEnd() }
        var end = shown.size
        while (end > 0 && shown[end - 1].isEmpty()) end--
        var start = 0
        while (start < end && shown[start].isEmpty()) start++
        return shown.subList(maxOf(start, end - max), end)
    }

    enum class Tone { Working, Waiting, Idle, Done }

    /** [agent]: the coding agent on the screen, null for a plain shell. */
    data class Status(val word: String, val tone: Tone, val agent: String?)

    /** The word and tone from the agent on the screen; a plain shell is Running or Exited. */
    fun status(terminal: TerminalListItem): Status {
        val preview = terminal.preview
        if (!preview.isNullOrEmpty()) {
            AgentFrameParser.parse(preview, terminal.title)?.let { frame ->
                return when (frame.status) {
                    AgentStatus.Working -> Status("Working", Tone.Working, frame.agent)
                    AgentStatus.NeedsYou -> Status("Needs you", Tone.Waiting, frame.agent)
                    AgentStatus.Idle -> Status("Idle", Tone.Idle, frame.agent)
                }
            }
        }
        return if (terminal.running) Status("Running", Tone.Idle, null) else Status("Exited", Tone.Done, null)
    }

    /** The row's title: the terminal's own, else its folder. */
    fun title(terminal: TerminalListItem): String =
        terminal.title.trim().ifEmpty { OnYourComputer.basename(terminal.cwd).ifEmpty { "Terminal" } }
}

/** Polling rules (terminal-sessions.store.ts): 2 s terminals, 5 s sessions, only while on screen, 15 s after 3 failures in a row, stop on 404. */
object TerminalPoll {
    enum class Support { Yes, No, Unknown }

    const val TERMINAL_MS = 2_000L
    const val LIVE_SESSION_MS = 5_000L
    const val BACKOFF_MS = 15_000L
    const val BACKOFF_AFTER_FAILURES = 3
    const val JOB_MS = 1_000L
    const val JOB_MAX_ERRORS = 5

    /** Whether an endpoint should poll right now. */
    fun shouldPoll(onScreen: Boolean, active: Boolean, support: Support): Boolean = onScreen && active && support != Support.No

    /** Milliseconds before the next poll of an endpoint, given its consecutive failures. */
    fun nextDelayMs(baseMs: Long, failures: Int): Long = if (failures >= BACKOFF_AFTER_FAILURES) maxOf(baseMs, BACKOFF_MS) else baseMs
}

// MARK: - Store

/**
 * Terminals and live agent sessions on the connected computer for the Code sessions view (iOS
 * TerminalSessionsStore). Owned by [CodeStore], so it lives as long as Code mode does: a Move's
 * follow-up keeps running after the sessions view closes, and a finished Move still opens its
 * terminal. It polls only while the view is on screen and the app is in the foreground
 * ([setActive]). Nothing here throws into a view: an unreachable or older runtime leaves the lists
 * empty and the sections hidden.
 */
class TerminalSessionsStore(private val backend: ChatBackend) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    var terminals by mutableStateOf<List<TerminalListItem>>(emptyList()); private set
    var liveSessions by mutableStateOf<List<LiveSession>>(emptyList()); private set
    var listSupport by mutableStateOf(TerminalPoll.Support.Unknown); private set
    var adoptSupport by mutableStateOf(TerminalPoll.Support.Unknown); private set

    /** Move jobs (and failed copies / news) started here or elsewhere, by live session id. */
    private val jobs = mutableStateMapOf<String, AdoptJob>()
    val adoptJobs: Map<String, AdoptJob> get() = jobs

    /** Copy / New in flight, by live session id. */
    private val pendingActions = mutableStateMapOf<String, OnYourComputer.RowAction>()
    val pending: Map<String, OnYourComputer.RowAction> get() = pendingActions

    /** The row each job started from, so its progress and error stay after the session leaves the list. */
    private val jobSessions = mutableStateMapOf<String, LiveSession>()

    /** A one-line note for the sessions view (a terminal that could not get a tab). */
    var notice by mutableStateOf<String?>(null)

    private val followers = HashMap<String, Job>()
    private val loops = ArrayList<Job>()
    private var active = false
    private var runtimeKey: String? = null
    private var terminalFailures = 0
    private var liveFailures = 0

    /** The rows "On your computer" shows. */
    val rows: List<LiveSession> get() = OnYourComputer.rowsWithJobs(liveSessions, jobSessions)

    /** Start (or stop) polling: the view is on screen and the app is in the foreground. */
    fun setActive(on: Boolean) {
        if (on == active) return
        active = on
        loops.forEach { it.cancel() }
        loops.clear()
        if (!on) return
        // A 404 stops an endpoint for this visit; the next visit asks once more (the runtime may have updated).
        if (listSupport == TerminalPoll.Support.No) listSupport = TerminalPoll.Support.Unknown
        if (adoptSupport == TerminalPoll.Support.No) adoptSupport = TerminalPoll.Support.Unknown
        loops += scope.launch { loop(terminalsLoop = true) }
        loops += scope.launch { loop(terminalsLoop = false) }
    }

    private suspend fun loop(terminalsLoop: Boolean) {
        while (currentCoroutineContext().isActive) {
            val support = if (terminalsLoop) listSupport else adoptSupport
            if (!TerminalPoll.shouldPoll(onScreen = true, active = active, support = support)) return
            if (terminalsLoop) pollTerminals() else pollLiveSessions()
            delay(
                if (terminalsLoop) TerminalPoll.nextDelayMs(TerminalPoll.TERMINAL_MS, terminalFailures)
                else TerminalPoll.nextDelayMs(TerminalPoll.LIVE_SESSION_MS, liveFailures),
            )
        }
    }

    /** A different computer answered: what the last one listed no longer applies. */
    private fun noteRuntime() {
        val id = backend.relay.activeRuntimeId ?: return
        if (id == runtimeKey) return
        if (runtimeKey != null) {
            terminals = emptyList()
            liveSessions = emptyList()
            terminalFailures = 0
            liveFailures = 0
        }
        runtimeKey = id
    }

    private suspend fun pollTerminals() {
        val r = CodeApi.listTerminals(backend, TerminalPreview.REQUEST_ROWS)
        noteRuntime()
        when (r) {
            is LiveCall.Ok -> {
                terminalFailures = 0
                if (r.value != terminals) terminals = r.value
                listSupport = TerminalPoll.Support.Yes
            }
            LiveCall.Unsupported -> { terminals = emptyList(); listSupport = TerminalPoll.Support.No }
            // No computer: hide, and look again at the slow cadence.
            LiveCall.NoComputer -> { terminals = emptyList(); terminalFailures = TerminalPoll.BACKOFF_AFTER_FAILURES }
            is LiveCall.Failed -> {
                terminalFailures++
                if (terminalFailures >= TerminalPoll.BACKOFF_AFTER_FAILURES) terminals = emptyList()
            }
        }
    }

    private suspend fun pollLiveSessions() {
        val r = CodeApi.liveSessions(backend)
        noteRuntime()
        when (r) {
            is LiveCall.Ok -> {
                liveFailures = 0
                if (r.value != liveSessions) liveSessions = r.value
                adoptSupport = TerminalPoll.Support.Yes
                resumeJobsFromRuntime(r.value)
            }
            LiveCall.Unsupported -> { liveSessions = emptyList(); adoptSupport = TerminalPoll.Support.No }
            LiveCall.NoComputer -> { liveSessions = emptyList(); liveFailures = TerminalPoll.BACKOFF_AFTER_FAILURES }
            is LiveCall.Failed -> {
                liveFailures++
                if (liveFailures >= TerminalPoll.BACKOFF_AFTER_FAILURES) liveSessions = emptyList()
            }
        }
    }

    /** A move started elsewhere (Desktop, the web, another phone) shows its progress here too. */
    private fun resumeJobsFromRuntime(rows: List<LiveSession>) {
        for (s in rows) {
            val job = s.move ?: continue
            if (job.jobId.isEmpty() || job.finished || jobs[s.sessionId] != null) continue
            jobs[s.sessionId] = job
            jobSessions[s.sessionId] = s
            follow(s.sessionId, job.jobId, OnYourComputer.displayName(s), open = null)
        }
    }

    // MARK: Adopt

    /**
     * Move, Open a copy or New session here, run in the store's scope so a Move keeps going after the
     * sessions view closes. [open] gets the new terminal: at once for a copy or a new session, when the
     * job is done for a move.
     */
    fun start(session: LiveSession, action: OnYourComputer.RowAction, open: (terminalId: String, name: String) -> Unit) {
        val id = session.sessionId
        if (pendingActions[id] != null || jobs[id]?.finished == false) return
        followers.remove(id)?.cancel()
        jobs.remove(id)
        jobSessions.remove(id)
        notice = null
        if (!action.isMove) pendingActions[id] = action
        val name = OnYourComputer.displayName(session)
        scope.launch {
            val r = try {
                CodeApi.adopt(backend, session, action.mode, PHONE_COLS, PHONE_ROWS)
            } finally {
                pendingActions.remove(id)
            }
            when (r) {
                is LiveCall.Ok -> when (val v = r.value) {
                    is AdoptStart.Terminal -> { open(v.id, name); pollTerminals() }
                    is AdoptStart.Job -> {
                        jobs[id] = v.job
                        jobSessions[id] = session
                        follow(id, v.job.jobId, name, open)
                    }
                    AdoptStart.Cancelled -> Unit
                }
                // An older runtime: hide the section quietly.
                LiveCall.Unsupported -> { adoptSupport = TerminalPoll.Support.No; liveSessions = emptyList() }
                LiveCall.NoComputer -> fail(session, "unreachable", null)
                is LiveCall.Failed -> fail(session, r.code, r.message)
            }
        }
    }

    private fun fail(session: LiveSession, code: String, message: String?) {
        jobs[session.sessionId] = AdoptJob("", "failed", code = code, error = message)
        jobSessions[session.sessionId] = session
    }

    /** Cancel a waiting move. A `too_late` answer keeps following the job and says so. */
    fun cancel(session: LiveSession) {
        val id = session.sessionId
        val job = jobs[id] ?: return
        scope.launch {
            val r = CodeApi.adopt(backend, session, "cancel", 80, 24)
            if (r is LiveCall.Failed && r.code == "too_late") {
                jobs[id]?.takeIf { !it.finished }?.let { current ->
                    jobs[id] = current.copy(notice = OnYourComputer.TOO_LATE_NOTICE)
                    if (followers[id] == null && job.jobId.isNotEmpty()) follow(id, job.jobId, OnYourComputer.displayName(session), open = null)
                }
                return@launch
            }
            // Cancelled, or anything else: drop the row's progress, as the web does.
            dismiss(id)
        }
    }

    /** Clears a finished job (the row's Dismiss). */
    fun dismiss(sessionId: String) {
        followers.remove(sessionId)?.cancel()
        jobs.remove(sessionId)
        jobSessions.remove(sessionId)
    }

    private fun follow(id: String, jobId: String, name: String, open: ((String, String) -> Unit)?) {
        followers.remove(id)?.cancel()
        followers[id] = scope.launch {
            val self = currentCoroutineContext()[Job]
            var errors = 0
            while (currentCoroutineContext().isActive) {
                delay(TerminalPoll.JOB_MS)
                when (val r = CodeApi.adoptJob(backend, jobId)) {
                    is LiveCall.Ok -> {
                        errors = 0
                        val job = r.value.copy(notice = jobs[id]?.notice)
                        jobs[id] = job
                        if (job.finished) {
                            if (followers[id] === self) followers.remove(id)
                            if (job.state == "done") {
                                // Done: the session is an Allternit terminal now; open it.
                                jobs.remove(id)
                                jobSessions.remove(id)
                                job.terminalId?.let { open?.invoke(it, name) }
                            }
                            pollTerminals()
                            pollLiveSessions()
                            return@launch
                        }
                    }
                    else -> if (++errors >= TerminalPoll.JOB_MAX_ERRORS) {
                        if (followers[id] === self) followers.remove(id)
                        jobs[id] = AdoptJob(jobId, "failed", code = "unreachable", error = "Lost contact with this computer")
                        return@launch
                    }
                }
            }
        }
    }

    companion object {
        /** Phone terminal size for a new or moved session (web phone 48×30). */
        const val PHONE_COLS = 48
        const val PHONE_ROWS = 30
    }
}
