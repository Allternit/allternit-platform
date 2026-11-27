package com.allternit.feature.code

import android.os.Handler
import android.os.Looper
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.Haptics
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.NoComputerException
import com.allternit.feature.chat.RuntimeSocketOpen
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.WebSocket
import com.allternit.feature.artifacts.str
import com.allternit.core.design.R as DesignR

/*
 * The Code terminal (iOS TerminalSessionView, PhoneCodeTerminal, CodeTerminalStore, PtySession).
 * iOS draws with SwiftTerm, a native emulator, so this does the same with a native VT screen
 * ([VtScreen]); there is no WebView or xterm.js. Shell tiles are terminals in the computer's
 * terminal service (`/terminal/create`, the SSE stream `/terminal/:id/stream`, input and resize by
 * POST), as on Desktop, the web and iOS; a Terminal bot's pane streams `screen` frames over SSE and
 * takes keystrokes by POST. When a coding agent runs in a shell tile, its own input box is hidden
 * and a native composer drives it ([AgentFrameReader], [AgentComposer]).
 */

enum class PtyState { Starting, Live, Ended, Offline, Failed }

/** One shell the terminal view draws and types into: a terminal-service tile or a gizzi-code pty. */
interface ShellSession {
    val screen: VtScreen
    val state: PtyState
    val message: String?
    val version: Int
    /** False when the shell is gone for good, so the way back is a new one (not a reconnect). */
    val canReattach: Boolean get() = true
    fun resize(cols: Int, rows: Int)
    fun send(text: String)
    fun retry()
}

/**
 * One gizzi-code pty (`v1/pty/create`, WebSocket `v1/pty/:id/connect` over the relay). The Code tiles
 * moved to [TerminalSession]; the Computers console terminal (feature/computer) still opens these.
 */
class PtySession(private val backend: ChatBackend, private val scope: CoroutineScope, private val target: String? = null) : ShellSession {
    override val screen = VtScreen(80, 24)
    override var state by mutableStateOf(PtyState.Starting); private set
    override var message by mutableStateOf<String?>(null); private set
    override var version by mutableIntStateOf(0); private set
    var ptyId: String? = null; private set
    private var socket: WebSocket? = null
    private var size: Pair<Int, Int>? = null
    private var closed = false
    private val main = Handler(Looper.getMainLooper())

    init { screen.onReply = { send(it) } }

    fun start(title: String?) {
        scope.launch {
            when (val r = CodeApi.createPty(backend, title, target)) {
                CodeResult.NoComputer -> state = PtyState.Offline
                is CodeResult.Failed -> { state = PtyState.Failed; message = r.message }
                is CodeResult.Ok -> { ptyId = r.value; connect(r.value) }
            }
        }
    }

    private suspend fun connect(id: String) {
        val open = backend.openRuntimeSocket(
            path = "/v1/pty/${encode(id)}/connect",
            onText = { text -> main.post { screen.feed(text); version++ } },
            // The only binary frame is the meta frame (0x00 + JSON): the end of the scrollback replay.
            onBinary = { bytes -> if (bytes.isNotEmpty() && bytes[0] == 0.toByte()) main.post { markLive() } },
            onEnd = { main.post { if (!closed && state != PtyState.Failed) state = PtyState.Ended } },
            targetRuntimeId = target,
        )
        when (open) {
            is RuntimeSocketOpen.Opened -> socket = open.socket
            RuntimeSocketOpen.NoComputer -> state = PtyState.Offline
            is RuntimeSocketOpen.Failed -> { state = PtyState.Failed; message = open.message }
        }
    }

    private fun markLive() {
        state = PtyState.Live
        // The first size is reported during layout, before the pty exists: send it now.
        size?.let { (c, r) -> pushSize(c, r) }
    }

    private fun pushSize(cols: Int, rows: Int) {
        val id = ptyId ?: return
        scope.launch { CodeApi.resizePty(backend, id, cols, rows, target) }
    }

    override fun resize(cols: Int, rows: Int) {
        if (size == cols to rows) return
        size = cols to rows
        screen.resize(cols, rows)
        version++
        if (state == PtyState.Live) pushSize(cols, rows)
    }

    override fun send(text: String) {
        if (state == PtyState.Live) socket?.send(text)
    }

    override fun retry() {
        socket?.cancel(); socket = null
        screen.reset(); state = PtyState.Starting; message = null
        ptyId?.let { id -> scope.launch { connect(id) } } ?: start(null)
    }

    /** Leaves the shell running on the computer and only drops this view's socket (Computers console: foreground only). */
    fun detach() {
        closed = true
        socket?.close(1000, null)
        socket = null
    }

    fun close() {
        closed = true
        socket?.close(1000, null)
        socket = null
        ptyId?.let { id -> scope.launch { CodeApi.deletePty(backend, id, target) } }
    }
}

/**
 * One Code shell tile in the computer's terminal service (cmd/allternit-api terminal_routes.rs; web
 * UnifiedTerminal, iOS PtySession(service:)). The shell lives in the pane engine, so it survives this
 * view, a reconnect and the runtime restarting; the stream replays the kept scrollback on every
 * attach, then live output, then `{"type":"exit"}`. Input is batched: one POST in flight, the keys typed
 * meanwhile go out together next, in order. A terminal the computer no longer has (404) is gone: the
 * tile says so and its button starts a new one.
 */
class TerminalSession(
    private val backend: ChatBackend,
    internal val scope: CoroutineScope,
    existingId: String? = null,
    /** The terminal id once the computer made one (null when it is gone), for the store to remember. */
    private val onId: (String?) -> Unit = {},
    /** Start anywhere: the computer a bound chat thread's terminal lives on (null = the picked computer). */
    private val target: String? = null,
) : ShellSession {
    override val screen = VtScreen(80, 24)
    override var state by mutableStateOf(PtyState.Starting); private set
    override var message by mutableStateOf<String?>(null); private set
    override var version by mutableIntStateOf(0); private set
    override var canReattach by mutableStateOf(true); private set
    var terminalId: String? = existingId; private set
    private var stream: Job? = null
    private var size: Pair<Int, Int>? = null
    private var pushed: Pair<Int, Int>? = null
    private val pendingInput = StringBuilder()
    private var sending = false
    private var closed = false

    /**
     * Report replies (cursor position, device attributes) answer the emulator's own queries. The
     * stream replays the kept scrollback on every attach, and old queries in it must not be answered
     * again: a shell at a continuation prompt echoes them as "30;3R30;3R" (UI feedback 11-26 K3, web
     * createTerminalReplyGuard). Replies are dropped from attach until 400 ms after the last replayed
     * chunk, or 1.5 s after the stream opens; a shell this tile just created has no replay.
     */
    private var replayOpenedAt = 0L
    private var replayLastChunkAt = 0L
    private var freshShell = false

    private fun replaying(): Boolean {
        if (freshShell) return false
        val now = android.os.SystemClock.uptimeMillis()
        if (replayOpenedAt == 0L) return true
        return now - replayOpenedAt < 1500L && now - replayLastChunkAt < 400L
    }

    init { screen.onReply = { if (!replaying()) send(it) } }

    /** Creates the terminal (sized to the view once it is known) or re-attaches the remembered one. */
    fun start() {
        scope.launch { open() }
    }

    private suspend fun open() {
        state = PtyState.Starting; message = null; canReattach = true; freshShell = false
        // A bound chat thread's terminal is the CLI the start made; never open a plain shell in its place.
        if (terminalId == null && target != null) {
            state = PtyState.Ended; message = "This session ended on your computer. Start a new chat to run it again."; canReattach = false
            return
        }
        val id = terminalId ?: when (val r = CodeApi.createTerminal(backend, size?.first ?: screen.cols, size?.second ?: screen.rows)) {
            CodeResult.NoComputer -> { state = PtyState.Offline; return }
            is CodeResult.Failed -> { state = PtyState.Failed; message = r.message; return }
            is CodeResult.Ok -> r.value.also { terminalId = it; onId(it); freshShell = true }
        }
        attach(id)
    }

    private fun attach(id: String) {
        stream?.cancel()
        // The stream replays the scrollback from the start: draw it on a clean screen.
        screen.reset(); version++
        replayOpenedAt = 0L; replayLastChunkAt = 0L
        pushed = null
        size?.let { (c, r) -> pushSize(c, r) }
        stream = scope.launch {
            try {
                // Collected on the main thread: each event's output lands on the screen as it arrives.
                backend.runtimeEvents(CodeApi.terminalStreamPath(id), target).collect { e ->
                    if (state == PtyState.Starting) state = PtyState.Live
                    if (replayOpenedAt == 0L) { replayOpenedAt = android.os.SystemClock.uptimeMillis(); replayLastChunkAt = replayOpenedAt }
                    // Keep-alives (`event: ping`) and any other named event are not output.
                    if (e.event != "message") return@collect
                    when (val out = streamOutput(e.data)) {
                        StreamOutput.Exit -> exited()
                        is StreamOutput.Text -> { replayLastChunkAt = android.os.SystemClock.uptimeMillis(); screen.feed(out.text); version++ }
                        null -> Unit
                    }
                }
                if (!closed && state != PtyState.Ended) { state = PtyState.Ended; message = null }
            } catch (e: NoComputerException) {
                state = PtyState.Offline
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                if (closed) return@launch
                // A remembered id the computer no longer has (an old tile, a restarted engine) is gone, not an error.
                if (isGone(id)) gone() else { state = PtyState.Failed; message = e.message ?: "The terminal stopped streaming." }
            }
        }
    }

    /** Resizing doubles as the existence check: the service answers 404 for an unknown terminal. */
    private suspend fun isGone(id: String): Boolean =
        (CodeApi.resizeTerminal(backend, id, screen.cols, screen.rows, target) as? CodeResult.Failed)?.status == 404

    private fun exited() {
        // The shell exited; the stream stays open with pings only. Typing goes nowhere now.
        stream?.cancel(); stream = null
        state = PtyState.Ended; message = "The shell exited."; canReattach = false
    }

    private fun gone() {
        if (closed) return // detached or closed: the tile that owns it now decides
        stream?.cancel(); stream = null
        terminalId = null; onId(null)
        state = PtyState.Ended; message = "This terminal closed on your computer."; canReattach = false
        pendingInput.setLength(0)
    }

    private fun pushSize(cols: Int, rows: Int) {
        val id = terminalId ?: return
        if (pushed == cols to rows) return
        pushed = cols to rows
        scope.launch {
            val r = CodeApi.resizeTerminal(backend, id, cols, rows, target)
            if (r is CodeResult.Failed) {
                if (pushed == cols to rows) pushed = null // a later identical size retries
                if (r.status == 404 && terminalId == id) gone()
            }
        }
    }

    override fun resize(cols: Int, rows: Int) {
        if (size == cols to rows) return
        size = cols to rows
        screen.resize(cols, rows)
        version++
        if (state == PtyState.Live || state == PtyState.Starting) pushSize(cols, rows)
    }

    override fun send(text: String) {
        if (terminalId == null || closed || state == PtyState.Ended || state == PtyState.Failed || state == PtyState.Offline) return
        pendingInput.append(text)
        flushInput()
    }

    private fun flushInput() {
        val id = terminalId ?: return
        if (sending || pendingInput.isEmpty()) return
        val text = pendingInput.toString()
        pendingInput.setLength(0)
        sending = true
        scope.launch {
            val r = CodeApi.terminalInput(backend, id, text, target)
            sending = false
            if (r is CodeResult.Failed && r.status == 404 && terminalId == id) gone() else flushInput()
        }
    }

    /** Reconnect re-attaches the same shell (its scrollback replays); after an exit or a 404 it starts a new one. */
    override fun retry() {
        stream?.cancel(); stream = null
        closed = false
        if (!canReattach) { terminalId?.let { id -> scope.launch { CodeApi.closeTerminal(backend, id, target) } }; terminalId = null; onId(null) }
        screen.reset(); version++
        start()
    }

    /** Stops reading the stream and leaves the shell running on the computer (another view took it over). */
    fun detach() {
        closed = true
        stream?.cancel(); stream = null
    }

    /** Ends the shell on the computer (closing a tile). */
    fun close() {
        closed = true
        stream?.cancel(); stream = null
        terminalId?.let { id -> scope.launch { CodeApi.closeTerminal(backend, id, target) } }
    }

    private sealed interface StreamOutput {
        data class Text(val text: String) : StreamOutput
        data object Exit : StreamOutput
    }

    /**
     * What one SSE `data:` payload means, like the web and iOS: JSON is read by its `type` (`data` prints,
     * `exit` ends; `{}` keep-alives and unknown types are ignored); a payload that isn't JSON prints as is.
     */
    private fun streamOutput(payload: String): StreamOutput? {
        val o = runCatching { Json.parseToJsonElement(payload) }.getOrNull()
        if (o == null) return if (payload.isEmpty()) null else StreamOutput.Text(payload)
        val obj = o as? JsonObject ?: return null
        return when (obj.str("type")) {
            "data" -> obj.str("data")?.takeIf { it.isNotEmpty() }?.let { StreamOutput.Text(it) }
            "exit" -> StreamOutput.Exit
            else -> null
        }
    }
}

/** A Terminal bot's pane, mirrored from its SSE `screen` frames. */
class PaneMirror(private val backend: ChatBackend, private val scope: CoroutineScope, val bot: TerminalBot) {
    val screen = VtScreen(80, 24)
    var version by mutableIntStateOf(0); private set
    var gone by mutableStateOf<String?>(null); private set
    var noComputer by mutableStateOf(false); private set
    var error by mutableStateOf<String?>(null); private set
    private var job: Job? = null

    fun start() {
        job?.cancel()
        gone = null; error = null; noComputer = false
        job = scope.launch {
            try {
                backend.runtimeEvents("/api/factory/agents/${encode(bot.id)}/stream").collect { e ->
                    if (e.event == "gone") {
                        gone = runCatching { (Json.parseToJsonElement(e.data) as? JsonObject).str("reason") }.getOrNull() ?: ""
                    } else {
                        val ansi = runCatching { (Json.parseToJsonElement(e.data) as? JsonObject).str("ansi") }.getOrNull() ?: return@collect
                        screen.reset()
                        screen.feed(ansi.replace("\r\n", "\n").replace("\n", "\r\n"))
                        version++
                    }
                }
            } catch (e: NoComputerException) {
                noComputer = true
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "The pane stopped streaming."
            }
        }
    }

    fun send(data: String) {
        scope.launch { for ((text, keys) in paneInput(data)) CodeApi.sendPaneInput(backend, bot.id, text, keys) }
    }

    fun stop() { job?.cancel() }
}

/**
 * The Code tiles' terminal ids for this app process. Code mode's store is remembered by the shell
 * screen, so an activity restart (rotation, a dark-mode switch) makes a new store; it re-attaches these
 * instead of starting fresh shells and leaving the old ones running on the computer. An id the computer
 * no longer has is treated as gone (the tile offers a new terminal), never an error.
 */
internal object CodeTerminalTiles {
    /**
     * [owned]: the phone started this shell, so closing its tile ends it. A terminal opened from the sessions
     * view (one the computer already had, or a moved session) is not: closing the tile only stops showing it.
     */
    class Saved(val name: String, var terminalId: String?, val owned: Boolean = true) {
        /** The session showing it now; a new store's session takes over its stream. */
        var attached: TerminalSession? = null
    }
    val saved = ArrayList<Saved>()
    var next = 1
}

/** Owned by Code mode, so closing and reopening the terminal keeps every shell (iOS CodeTerminalStore). */
class CodeTerminalStore(private val backend: ChatBackend) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    class Tile internal constructor(val name: String, val session: TerminalSession, internal val saved: CodeTerminalTiles.Saved) {
        /** The coding agent on the tile's screen (kept with the tile, so switching tabs keeps the crop). */
        val reader = AgentFrameReader(session.screen, session.scope)
    }

    val tiles = mutableStateListOf<Tile>()
    var selected by mutableIntStateOf(0)
    var bots by mutableStateOf<TerminalBots?>(null); private set
    var botsNoComputer by mutableStateOf(false); private set
    var botsError by mutableStateOf<String?>(null); private set
    private var started = false

    private val mirrors = HashMap<String, PaneMirror>()

    /** One mirror per bot while the store lives. */
    fun mirror(bot: TerminalBot): PaneMirror = mirrors.getOrPut(bot.id) { PaneMirror(backend, scope, bot) }

    fun startIfNeeded() {
        if (started) return
        restoreSaved()
        if (tiles.isEmpty()) addTile()
    }

    /** Re-attaches this process's remembered tiles (an activity restart) without starting a new shell. */
    private fun restoreSaved() {
        started = true
        for (saved in CodeTerminalTiles.saved.toList()) tiles.add(Tile(saved.name, session(saved).also { it.start() }, saved))
        selected = selected.coerceIn(0, maxOf(0, tiles.lastIndex))
    }

    /**
     * Shows the computer's terminal [terminalId] (a preview tap in the sessions view, a finished Move, Open a
     * copy, New session here): selects the tile already attached to it, else attaches a new tile by id, so
     * its scrollback replays. False when every tile is taken.
     */
    fun open(terminalId: String, name: String): Boolean {
        if (!started) restoreSaved()
        val index = tiles.indexOfFirst { it.session.terminalId == terminalId }
        if (index >= 0) { selected = index; return true }
        if (tiles.size >= MAX_TILES) return false
        val saved = CodeTerminalTiles.Saved(name.ifBlank { "Terminal ${CodeTerminalTiles.next++}" }, terminalId, owned = false)
        CodeTerminalTiles.saved.add(saved)
        val s = session(saved)
        tiles.add(Tile(saved.name, s, saved))
        selected = tiles.lastIndex
        s.start()
        return true
    }

    private fun session(saved: CodeTerminalTiles.Saved): TerminalSession {
        // The previous store (before an activity restart) stops reading the stream this one re-attaches.
        saved.attached?.detach()
        return TerminalSession(backend, scope, saved.terminalId, onId = { saved.terminalId = it }).also { saved.attached = it }
    }

    fun addTile() {
        if (tiles.size >= MAX_TILES) return
        val saved = CodeTerminalTiles.Saved("Terminal ${CodeTerminalTiles.next++}", null)
        CodeTerminalTiles.saved.add(saved)
        val s = session(saved)
        tiles.add(Tile(saved.name, s, saved))
        selected = tiles.lastIndex
        s.start()
    }

    fun closeTile(index: Int) {
        val t = tiles.getOrNull(index) ?: return
        if (t.saved.owned) t.session.close() else t.session.detach()
        tiles.removeAt(index)
        CodeTerminalTiles.saved.remove(t.saved)
        t.saved.attached = null
        selected = selected.coerceIn(0, maxOf(0, tiles.lastIndex))
        if (tiles.isEmpty()) { started = false }
    }

    suspend fun refreshBots() {
        when (val r = CodeApi.terminalBots(backend)) {
            is CodeResult.Ok -> { bots = r.value; botsNoComputer = false; botsError = null }
            CodeResult.NoComputer -> { botsNoComputer = true }
            is CodeResult.Failed -> botsError = r.message
        }
    }

    companion object { const val MAX_TILES = 8 }
}

// MARK: - Agent frame

/**
 * Reads a coding agent's frame from a shell's screen after output settles, and how many rows of its input
 * box to hide (UnifiedTerminal.tsx `parseAgentScreen`, iOS AgentFrameReader: same timing and caps). The
 * hidden rows are drawn past the bottom of the view: the pty gets them on top of the visible rows, so the
 * agent keeps its room. An unrecognised screen gives null and the full terminal at its normal size.
 */
class AgentFrameReader(private val screen: VtScreen, private val scope: CoroutineScope) {
    var frame by mutableStateOf<AgentFrame?>(null); private set
    /** Rows of the agent's own input box kept below the visible frame. */
    var hiddenRows by mutableIntStateOf(0); private set
    private var pendingRows = 0
    private var pendingSeen = 0
    private var timer: Job? = null

    /** After an output batch: read the screen 120 ms later (at most ~8 times a second). */
    fun schedule() {
        if (timer?.isActive == true) return
        timer = scope.launch {
            delay(120L)
            timer = null
            read()
        }
    }

    private fun read() {
        // The view always follows the output (TerminalSurface), so the rows read are the rows shown.
        val rows = screen.visibleRows()
        val rowCount = rows.size
        val next = AgentFrameParser.parse(rows, screen.title)
        if (next != frame) frame = next
        // Hide the agent's box: rows from hideFrom down, capped so a misread can never hide much.
        val want = next?.let { maxOf(0, minOf(rowCount - it.hideFrom, 14, rowCount / 2)) } ?: 0
        if (want == hiddenRows) {
            pendingRows = want; pendingSeen = 0
            return
        }
        // Change only after two reads agree, so a redraw mid-frame never makes it jump.
        pendingSeen = if (pendingRows == want) pendingSeen + 1 else 1
        pendingRows = want
        if (pendingSeen < 2) {
            schedule()
            return
        }
        hiddenRows = want
    }
}

private sealed interface TermTarget {
    data class Shell(val index: Int) : TermTarget
    data class Bot(val bot: TerminalBot) : TermTarget
}

/**
 * The terminal screen: shell tiles and Terminal bots on a strip, the screen, and a key bar.
 * [onOpenPorts] opened a gizzi-code pty's dev servers (`/v1/pty/:id/ports`). The tiles are terminal-service
 * shells now, which have no gizzi-code pty (and the relayed runtime serves no `/v1/pty`), so the strip has
 * no Dev servers button; the route stays for when the ports move to the terminal service.
 */
@Suppress("UNUSED_PARAMETER")
@Composable
fun TerminalScreen(store: CodeTerminalStore, backend: ChatBackend, onBack: () -> Unit, onOpenPorts: (ptyId: String) -> Unit, onConnect: () -> Unit) {
    var target by remember { mutableStateOf<TermTarget>(TermTarget.Shell(store.selected)) }
    LaunchedEffect(Unit) { store.startIfNeeded() }
    // A terminal opened from the sessions view (a preview, a finished Move) selects its tile: show it.
    LaunchedEffect(store.selected) { target = TermTarget.Shell(store.selected) }
    LaunchedEffect(Unit) {
        while (true) {
            store.refreshBots()
            kotlinx.coroutines.delay(5_000L)
        }
    }
    val bg = TerminalColors.background
    Column(Modifier.fillMaxSize().background(bg).statusBarsPadding().navigationBarsPadding().imePadding().testTag("codeTerminal")) {
        Row(Modifier.fillMaxWidth().background(TerminalColors.strip).heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
            StripButton(DesignR.drawable.ph_caret_left, "Back", "codeTerminalBack", onBack)
            Row(Modifier.weight(1f).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                store.tiles.forEachIndexed { i, tile ->
                    // A coding agent running in the tile names it ("Claude Code"), as on the web and iOS.
                    val label = tile.reader.frame?.agent ?: tile.name
                    TabChip(label, (target as? TermTarget.Shell)?.index == i, "codeTerminalTab-$i") { target = TermTarget.Shell(i); store.selected = i }
                }
                store.bots?.bots?.forEach { b ->
                    TabChip(b.name.ifEmpty { b.slug }, (target as? TermTarget.Bot)?.bot?.id == b.id, "codeTerminalBot-${b.slug}") { target = TermTarget.Bot(b) }
                }
            }
            if (store.tiles.size < CodeTerminalStore.MAX_TILES) {
                StripButton(DesignR.drawable.ph_plus, "New shell", "codeTerminalNew") { store.addTile(); target = TermTarget.Shell(store.tiles.lastIndex) }
            }
            (target as? TermTarget.Shell)?.let { t ->
                // A terminal the computer already had keeps running there; only its tab closes.
                val owned = store.tiles.getOrNull(t.index)?.saved?.owned != false
                val closeTile = {
                    store.closeTile(t.index)
                    target = TermTarget.Shell(store.selected)
                }
                // Ending a shell is a hold, never a tap or a dialog; closing a tab only hides it.
                if (owned) HoldStripButton(DesignR.drawable.ph_x, "Hold to end shell", "codeTerminalClose", closeTile)
                else StripButton(DesignR.drawable.ph_x, "Close tab", "codeTerminalClose", closeTile)
            }
        }
        when (val t = target) {
            is TermTarget.Shell -> {
                val tile = store.tiles.getOrNull(t.index)
                if (tile == null) {
                    Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text("No terminal sessions", color = TerminalColors.foreground, fontSize = 17.sp)
                            Text("Start a shell for this session.", color = TerminalColors.secondary, fontSize = 14.sp)
                            CodePill("New shell", testTag = "codeTerminalStart", filled = true) { store.addTile(); target = TermTarget.Shell(store.tiles.lastIndex) }
                        }
                    }
                } else ShellPane(tile.session, onConnect, Modifier.weight(1f), reader = tile.reader)
            }
            is TermTarget.Bot -> BotPane(store, t.bot, Modifier.weight(1f))
        }
    }
}

@Composable
private fun StripButton(icon: Int, label: String, tag: String, onClick: () -> Unit) {
    Box(
        Modifier.size(44.dp).testTag(tag).clickable(onClickLabel = label, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), label, tint = TerminalColors.foreground, modifier = Modifier.size(20.dp)) }
}

/** A strip icon that acts only after a 0.8 s hold; a ring fills while held. Screen readers get a plain activate. */
@Composable
private fun HoldStripButton(icon: Int, label: String, tag: String, onHeld: () -> Unit) {
    val progress = remember { androidx.compose.animation.core.Animatable(0f) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    Box(
        Modifier.size(44.dp).testTag(tag)
            .semantics { onClick(label = label) { onHeld(); true } }
            .pointerInput(Unit) {
                detectTapGestures(onPress = {
                    val hold = scope.launch {
                        progress.animateTo(1f, androidx.compose.animation.core.tween(800, easing = androidx.compose.animation.core.LinearEasing))
                        onHeld()
                        progress.snapTo(0f)
                    }
                    tryAwaitRelease()
                    hold.cancel()
                    scope.launch { progress.animateTo(0f, androidx.compose.animation.core.tween(150)) }
                })
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(36.dp).background(com.allternit.core.design.AllternitStatic.StatusError.copy(alpha = 0.5f * progress.value), androidx.compose.foundation.shape.CircleShape))
        Icon(painterResource(icon), label, tint = TerminalColors.foreground, modifier = Modifier.size(20.dp))
    }
}

@Composable
private fun TabChip(label: String, selected: Boolean, tag: String, onClick: () -> Unit) {
    Text(
        label, color = if (selected) TerminalColors.foreground else TerminalColors.secondary, fontSize = 14.sp, fontFamily = CodeMono, maxLines = 1,
        modifier = Modifier.testTag(tag).clip(RoundedCornerShape(10.dp)).background(if (selected) TerminalColors.keyPressed else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

/** One shell's screen + key bar for another module (Computers console Terminal tab). */
/**
 * Start anywhere (step 20): a chat thread bound to a CLI in a pane terminal on [runtimeId]. The thread hosts
 * the same view as a Code tile: the terminal, the native composer over the agent's input box ([AgentComposer]),
 * and the key row. The terminal stays on the computer when the view goes away (detach, never close).
 */
@Composable
fun BoundTerminalThread(backend: ChatBackend, runtimeId: String, terminalId: String, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    val session = remember(runtimeId, terminalId) { TerminalSession(backend, scope, existingId = terminalId, target = runtimeId) }
    val reader = remember(session) { AgentFrameReader(session.screen, scope) }
    DisposableEffect(session) {
        session.start()
        onDispose { session.detach() }
    }
    ShellPane(session, onConnect = session::retry, modifier = modifier.testTag("boundTerminalThread"), reader = reader)
}

@Composable
fun PtyTerminalView(session: PtySession, modifier: Modifier = Modifier) {
    ShellPane(session, onConnect = {}, modifier = modifier)
}

/**
 * A shell's screen, its status and the key bar. With a [reader] (the Code tiles), a coding agent on the
 * screen gets its input box hidden and the native composer instead; the key row stays and its keys go to
 * the agent.
 */
@Composable
private fun ShellPane(session: ShellSession, onConnect: () -> Unit, modifier: Modifier, reader: AgentFrameReader? = null) {
    when (session.state) {
        PtyState.Offline -> Column(modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterVertically)) {
            Text("Connect a computer to use Shell", color = TerminalColors.foreground, fontSize = 17.sp, modifier = Modifier.testTag("codeTerminalOffline"))
            Text("Shell is your computer's own command line. Connect one, or start Allternit Cloud, and it opens here.", color = TerminalColors.secondary, fontSize = 14.sp)
            CodePill("Connect", testTag = "codeTerminalConnect", filled = true, onClick = onConnect)
        }
        PtyState.Failed -> Column(modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterVertically)) {
            Text(session.message ?: "The shell didn't start.", color = TerminalColors.foreground, fontSize = 15.sp)
            CodePill("Retry", testTag = "codeTerminalRetry", onClick = session::retry)
        }
        else -> Column(modifier) {
            if (reader != null) LaunchedEffect(session, session.version) { reader.schedule() }
            val ended = session.state == PtyState.Ended
            val frame = if (ended) null else reader?.frame
            TerminalSurface(session.screen, session.version, Modifier.weight(1f), hiddenRows = if (reader != null) reader.hiddenRows else 0) { cols, rows -> session.resize(cols, rows) }
            if (ended) {
                Row(Modifier.fillMaxWidth().background(TerminalColors.strip).padding(horizontal = 14.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(session.message ?: "The shell ended.", color = TerminalColors.secondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    if (session.canReattach) CodePill("Reconnect", testTag = "codeTerminalReconnect", onClick = session::retry)
                    else CodePill("New terminal", testTag = "codeTerminalRenew", onClick = session::retry)
                }
            }
            if (frame != null) AgentComposer(frame, onSend = session::send)
            KeyBar(screen = session.screen, onSend = session::send, typing = frame == null)
        }
    }
}

@Composable
private fun BotPane(store: CodeTerminalStore, bot: TerminalBot, modifier: Modifier) {
    val mirror = remember(bot.id) { store.mirror(bot) }
    DisposableEffect(mirror) {
        mirror.start()
        onDispose { mirror.stop() }
    }
    Column(modifier) {
        Row(Modifier.fillMaxWidth().background(TerminalColors.strip).padding(horizontal = 14.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(28.dp).clip(CircleShape).background(TerminalColors.keyPressed), contentAlignment = Alignment.Center) { Text(bot.initial, color = TerminalColors.foreground, fontSize = 13.sp) }
            Column(Modifier.weight(1f)) {
                Text(bot.name, color = TerminalColors.foreground, fontSize = 14.sp)
                Text(bot.address + (bot.remoteMachine?.let { " on $it" } ?: ""), color = TerminalColors.secondary, fontSize = 11.sp, fontFamily = CodeMono, maxLines = 1)
            }
            Text(bot.stateLabel, color = TerminalColors.secondary, fontSize = 12.sp)
        }
        when {
            mirror.noComputer -> Text("Terminal bots need a computer. Connect one to see them here.", color = TerminalColors.secondary, fontSize = 14.sp, modifier = Modifier.padding(24.dp))
            mirror.gone != null -> Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("The pane closed.", color = TerminalColors.foreground, fontSize = 16.sp)
                mirror.gone?.takeIf { it.isNotEmpty() }?.let { Text(it, color = TerminalColors.secondary, fontSize = 13.sp) }
            }
            mirror.error != null -> Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(mirror.error.orEmpty(), color = TerminalColors.foreground, fontSize = 14.sp)
                CodePill("Retry", testTag = "codeTerminalBotRetry") { mirror.start() }
            }
            else -> {
                TerminalSurface(mirror.screen, mirror.version, Modifier.weight(1f)) { cols, rows -> mirror.screen.resize(cols, rows) }
                KeyBar(screen = mirror.screen, onSend = mirror::send)
            }
        }
    }
}

/**
 * The text grid: sizes the emulator to the available space and follows the output. [hiddenRows] more
 * rows than fit are given to the shell and left undrawn at the bottom (a coding agent's own input box).
 */
@Composable
private fun TerminalSurface(screen: VtScreen, version: Int, modifier: Modifier, hiddenRows: Int = 0, onSize: (cols: Int, rows: Int) -> Unit) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val style = remember { TextStyle(fontFamily = CodeMono, fontSize = 12.sp, lineHeight = 16.sp, color = TerminalColors.foreground) }
    val cell = remember(density) { measurer.measure("M", style).size }
    val scroll = rememberScrollState()
    val text = remember(version, hiddenRows) { screen.render(hideLast = hiddenRows) }
    // The grid that fits; keyed by the screen too, so a tile switched into the same space gets its size.
    var grid by remember { mutableStateOf<Pair<Int, Int>?>(null) }
    val report by rememberUpdatedState(onSize)
    LaunchedEffect(version, hiddenRows) { scroll.scrollTo(scroll.maxValue) }
    LaunchedEffect(screen, grid, hiddenRows) { grid?.let { (cols, rows) -> report(cols, rows + hiddenRows) } }
    Box(
        modifier.fillMaxWidth().background(TerminalColors.background)
            .onSizeChanged { s ->
                if (cell.width > 0 && cell.height > 0) grid = (s.width / cell.width).coerceIn(20, 300) to (s.height / cell.height).coerceIn(5, 200)
            }
            .padding(horizontal = 0.dp),
    ) {
        Text(text, style = style, softWrap = false, modifier = Modifier.fillMaxSize().verticalScroll(scroll).testTag("codeTerminalScreen"))
    }
}

/**
 * The key bar: Esc, Tab, a Ctrl latch, arrows and an input field that types straight into the shell. While
 * a coding agent's composer shows ([typing] false), the composer takes the text and only the keys stay.
 */
@Composable
private fun KeyBar(screen: VtScreen, onSend: (String) -> Unit, typing: Boolean = true) {
    var ctrl by remember { mutableStateOf(false) }
    var field by remember { mutableStateOf("") }
    fun out(s: String) {
        if (ctrl && s.isNotEmpty()) {
            val b = ctrlByte(s[0])
            ctrl = false
            onSend(if (b != null) b.toString() + s.drop(1) else s)
        } else onSend(s)
    }
    Column(Modifier.fillMaxWidth().background(TerminalColors.strip)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            KeyButton("Esc", "Escape") { onSend("\u001b") }
            KeyButton("Tab", "Tab") { onSend("\t") }
            KeyButton("Ctrl", "Control", pressed = ctrl) { ctrl = !ctrl }
            KeyButton("^C", "Control C") { onSend("\u0003") }
            // Full-screen apps (vim, htop) switch the arrows to application mode (ESC O x).
            val arrow = { c: Char -> if (screen.appCursor) "\u001bO$c" else "\u001b[$c" }
            KeyButton("↑", "Arrow up") { onSend(arrow('A')) }
            KeyButton("↓", "Arrow down") { onSend(arrow('B')) }
            KeyButton("←", "Arrow left") { onSend(arrow('D')) }
            KeyButton("→", "Arrow right") { onSend(arrow('C')) }
            KeyButton("PgUp", "Page up") { onSend("\u001b[5~") }
            KeyButton("PgDn", "Page down") { onSend("\u001b[6~") }
            KeyButton("⌫", "Delete") { onSend("\u007f") }
            KeyButton("|", "Pipe") { onSend("|") }
            KeyButton("/", "Slash") { onSend("/") }
            KeyButton("~", "Tilde") { onSend("~") }
        }
        if (typing) BasicTextField(
            value = field,
            onValueChange = { v -> if (v.isNotEmpty()) out(v); field = "" },
            textStyle = TextStyle(color = TerminalColors.foreground, fontSize = 15.sp, fontFamily = CodeMono),
            cursorBrush = SolidColor(TerminalColors.foreground),
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Ascii, imeAction = ImeAction.Send),
            keyboardActions = KeyboardActions(onSend = { onSend("\r") }),
            modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).padding(horizontal = 14.dp, vertical = 10.dp).testTag("codeTerminalInput")
                .onPreviewKeyEvent { e ->
                    if (e.key == Key.Backspace && e.type == KeyEventType.KeyDown && field.isEmpty()) { onSend("\u007f"); true } else false
                },
            decorationBox = { inner ->
                Box { if (field.isEmpty()) Text("Type a command", color = TerminalColors.secondary, fontSize = 15.sp, fontFamily = CodeMono); inner() }
            },
        )
    }
}

@Composable
private fun KeyButton(label: String, spoken: String, pressed: Boolean = false, onClick: () -> Unit) {
    Text(
        label, color = TerminalColors.foreground, fontSize = 14.sp, fontFamily = CodeMono,
        modifier = Modifier.heightIn(min = 40.dp).width(keyWidth(label)).clip(RoundedCornerShape(8.dp))
            .background(if (pressed) TerminalColors.keyPressed else TerminalColors.key)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics { contentDescription = spoken; if (label == "Ctrl") stateDescription = if (pressed) "On" else "Off" }
            .padding(horizontal = 12.dp, vertical = 10.dp).testTag("codeTerminalKey-$label"),
        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
    )
}

private fun keyWidth(label: String) = (24 + label.length * 10).coerceAtLeast(44).dp

// MARK: - Agent composer

/**
 * Drives a coding agent in the shell (iOS AgentComposerBar, web agent-composer.tsx): its question as one
 * button per answer, the agent · status edge with Stop (Esc) while it works and its mode (tap: shift+tab),
 * and a "Message <agent>…" prompt that sends as a bracketed paste, then Enter 60 ms later.
 */
@Composable
private fun AgentComposer(frame: AgentFrame, onSend: (String) -> Unit) {
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    val agentName = frame.agent ?: "Agent"
    val trimmed = text.trim()
    fun send() {
        if (trimmed.isEmpty()) return
        val value = trimmed
        text = ""
        Haptics.perform(view, Haptics.Kind.Tap)
        onSend(AgentKeys.promptPaste(value))
        scope.launch { delay(60L); onSend(AgentKeys.ENTER) }
    }
    Column(
        Modifier.fillMaxWidth().background(TerminalColors.background).drawTopLine().padding(horizontal = 10.dp).padding(bottom = 4.dp)
            .testTag("codeAgentComposer"),
    ) {
        frame.dialog?.let { AgentQuestion(frame, it, onSend) }
        // Agent · status, Stop while it works; the agent's own mode on the right.
        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(agentName, color = TerminalColors.secondary, fontSize = 11.sp, fontFamily = CodeMono, maxLines = 1, modifier = Modifier.weight(1f, fill = false))
            Text("·", color = TerminalColors.tertiary, fontSize = 11.sp, fontFamily = CodeMono, modifier = Modifier.clearAndSetSemantics { })
            val status = AgentComposerText.status(frame.status)
            val statusColor = when (frame.status) {
                AgentStatus.Idle -> TerminalColors.success
                AgentStatus.Working -> TerminalColors.coral
                AgentStatus.NeedsYou -> TerminalColors.warning
            }
            Row(
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
                modifier = Modifier.clearAndSetSemantics { contentDescription = "$agentName: $status"; liveRegion = LiveRegionMode.Polite }.testTag("codeAgentStatus"),
            ) {
                Box(Modifier.size(6.dp).clip(CircleShape).background(statusColor))
                Text(status, color = statusColor, fontSize = 11.sp, fontFamily = CodeMono)
            }
            if (frame.status == AgentStatus.Working) {
                Row(
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                    modifier = Modifier.heightIn(min = 44.dp).clickable(role = Role.Button, onClickLabel = "Send Escape") { Haptics.perform(view, Haptics.Kind.Tap); onSend(AgentKeys.ESCAPE) }
                        .semantics { contentDescription = "Stop $agentName" }.testTag("codeAgentStop"),
                ) {
                    Row(
                        Modifier.clearAndSetSemantics { }.border(1.dp, TerminalColors.keyBorder, RoundedCornerShape(6.dp)).padding(horizontal = 8.dp, vertical = 4.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text("Stop", color = TerminalColors.foreground, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, fontFamily = CodeMono)
                        Text("esc", color = TerminalColors.tertiary, fontSize = 10.sp, fontFamily = CodeMono)
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            frame.mode?.let { mode ->
                Text(
                    AgentComposerText.shortMode(mode), color = TerminalColors.coral, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, fontFamily = CodeMono, maxLines = 1,
                    modifier = Modifier.heightIn(min = 44.dp).clickable(role = Role.Button, onClickLabel = "Switch the agent's mode") { Haptics.perform(view, Haptics.Kind.Tap); onSend(AgentKeys.CYCLE_MODE) }
                        .semantics { contentDescription = "Mode: $mode" }.padding(horizontal = 6.dp, vertical = 14.dp).testTag("codeAgentMode"),
                )
            }
        }
        // The prompt: ❯ then the message, Send once there is text.
        Row(
            Modifier.fillMaxWidth().border(1.dp, TerminalColors.keyBorder, RoundedCornerShape(8.dp)).padding(start = 10.dp, end = 4.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("❯", color = TerminalColors.coral, fontSize = 15.sp, fontWeight = FontWeight.Bold, fontFamily = CodeMono, modifier = Modifier.clearAndSetSemantics { })
            val placeholder = AgentComposerText.placeholder(frame)
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                textStyle = TextStyle(color = TerminalColors.foreground, fontSize = 16.sp, lineHeight = 22.sp, fontFamily = CodeMono),
                cursorBrush = SolidColor(TerminalColors.coral),
                maxLines = 4,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None, autoCorrectEnabled = false, imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(onSend = { send() }),
                modifier = Modifier.weight(1f).heightIn(min = 44.dp).padding(vertical = 11.dp).semantics { contentDescription = placeholder }.testTag("codeAgentInput"),
                decorationBox = { inner ->
                    Box { if (text.isEmpty()) Text(placeholder, color = TerminalColors.tertiary, fontSize = 16.sp, lineHeight = 22.sp, fontFamily = CodeMono, maxLines = 1); inner() }
                },
            )
            if (trimmed.isNotEmpty()) {
                Box(
                    Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).background(TerminalColors.coral)
                        .clickable(role = Role.Button, onClickLabel = "Send") { send() }.testTag("codeAgentSend"),
                    contentAlignment = Alignment.Center,
                ) { Icon(painterResource(DesignR.drawable.ph_arrow_elbow_down_left), "Send to $agentName", tint = Color.White, modifier = Modifier.size(18.dp)) }
            }
        }
    }
}

/** A permission / choice question from the agent, one button per answer (digit, or arrows + Enter). */
@Composable
private fun AgentQuestion(frame: AgentFrame, dialog: AgentDialog, onSend: (String) -> Unit) {
    val view = LocalView.current
    val isPermission = AgentComposerText.isPermission(dialog.question)
    Column(Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 4.dp).testTag("codeAgentQuestion"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.semantics(mergeDescendants = true) { }) {
            Icon(
                painterResource(if (isPermission) DesignR.drawable.ph_lock_fill else DesignR.drawable.ph_question), null,
                tint = TerminalColors.coral, modifier = Modifier.padding(top = 2.dp).size(14.dp),
            )
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(if (isPermission) "Permission required" else "${frame.agent ?: "The agent"} asks", color = TerminalColors.foreground, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                if (dialog.question.isNotEmpty()) Text(dialog.question, color = TerminalColors.secondary, fontSize = 13.sp)
            }
        }
        val answers = if (dialog.options.size > 4) Modifier.heightIn(max = 196.dp).verticalScroll(rememberScrollState()) else Modifier
        Column(answers, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            for (option in dialog.options) {
                val shape = RoundedCornerShape(8.dp)
                Row(
                    Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(shape)
                        .background(if (option.selected) TerminalColors.coral.copy(alpha = 0.14f) else TerminalColors.key)
                        .border(1.dp, if (option.selected) TerminalColors.coral.copy(alpha = 0.36f) else TerminalColors.keyBorder, shape)
                        .clickable(role = Role.Button, onClickLabel = "Pick this answer") { Haptics.perform(view, Haptics.Kind.Tap); onSend(AgentKeys.dialogKeys(dialog.options, option)) }
                        .semantics {
                            contentDescription = option.label + if (option.key.isEmpty()) "" else ", ${option.key}"
                            if (option.selected) stateDescription = "Selected"
                        }
                        .padding(horizontal = 12.dp).testTag("codeAgentOption-${option.index}"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Text(
                        AgentComposerText.shortLabel(option.label), color = if (option.selected) TerminalColors.foreground else TerminalColors.secondary,
                        fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, modifier = Modifier.weight(1f).clearAndSetSemantics { },
                    )
                    if (option.key.isNotEmpty()) Text(option.key, color = TerminalColors.tertiary, fontSize = 11.sp, fontFamily = CodeMono, modifier = Modifier.clearAndSetSemantics { })
                }
            }
        }
    }
}

/** The 1 px rule along the top of the composer (phone.css). */
private fun Modifier.drawTopLine(): Modifier = drawBehind {
    drawLine(TerminalColors.line, androidx.compose.ui.geometry.Offset(0f, 0f), androidx.compose.ui.geometry.Offset(size.width, 0f), strokeWidth = 1.dp.toPx())
}
