package com.allternit.feature.code

import android.os.Handler
import android.os.Looper
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.NoComputerException
import com.allternit.feature.chat.RuntimeSocketOpen
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.WebSocket
import com.allternit.feature.artifacts.str
import com.allternit.core.design.R as DesignR

/*
 * The Code terminal (iOS TerminalSessionView, PhoneCodeTerminal, CodeTerminalStore, PtySession).
 * iOS draws with SwiftTerm, a native emulator, so this does the same with a native VT screen
 * ([VtScreen]); there is no WebView or xterm.js. Shell tiles are ptys on the computer's
 * gizzi-code (`v1/pty/create`, WebSocket `v1/pty/:id/connect` over the relay); a Terminal bot's
 * pane streams `screen` frames over SSE and takes keystrokes by POST.
 */

enum class PtyState { Starting, Live, Ended, Offline, Failed }

/** One shell: the pty, its socket and its screen. */
class PtySession(private val backend: ChatBackend, private val scope: CoroutineScope) {
    val screen = VtScreen(80, 24)
    var state by mutableStateOf(PtyState.Starting); private set
    var message by mutableStateOf<String?>(null); private set
    var version by mutableIntStateOf(0); private set
    var ptyId: String? = null; private set
    private var socket: WebSocket? = null
    private var size: Pair<Int, Int>? = null
    private var closed = false
    private val main = Handler(Looper.getMainLooper())

    fun start(title: String?) {
        scope.launch {
            when (val r = CodeApi.createPty(backend, title)) {
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
        scope.launch { CodeApi.resizePty(backend, id, cols, rows) }
    }

    fun resize(cols: Int, rows: Int) {
        if (size == cols to rows) return
        size = cols to rows
        screen.resize(cols, rows)
        version++
        if (state == PtyState.Live) pushSize(cols, rows)
    }

    fun send(text: String) {
        if (state == PtyState.Live) socket?.send(text)
    }

    fun retry() {
        socket?.cancel(); socket = null
        screen.reset(); state = PtyState.Starting; message = null
        ptyId?.let { id -> scope.launch { connect(id) } } ?: start(null)
    }

    fun close() {
        closed = true
        socket?.close(1000, null)
        socket = null
        ptyId?.let { id -> scope.launch { CodeApi.deletePty(backend, id) } }
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

/** Owned by Code mode, so closing and reopening the terminal keeps every shell (iOS CodeTerminalStore). */
class CodeTerminalStore(private val backend: ChatBackend) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    class Tile(val name: String, val session: PtySession)

    val tiles = mutableStateListOf<Tile>()
    var selected by mutableIntStateOf(0)
    var bots by mutableStateOf<TerminalBots?>(null); private set
    var botsNoComputer by mutableStateOf(false); private set
    var botsError by mutableStateOf<String?>(null); private set
    private var next = 1
    private var started = false

    private val mirrors = HashMap<String, PaneMirror>()

    /** One mirror per bot while the store lives. */
    fun mirror(bot: TerminalBot): PaneMirror = mirrors.getOrPut(bot.id) { PaneMirror(backend, scope, bot) }

    fun startIfNeeded() {
        if (started) return
        started = true
        addTile()
    }

    fun addTile() {
        if (tiles.size >= MAX_TILES) return
        val s = PtySession(backend, scope)
        tiles.add(Tile("Terminal ${next++}", s))
        selected = tiles.lastIndex
        s.start("Terminal ${next - 1}")
    }

    fun closeTile(index: Int) {
        val t = tiles.getOrNull(index) ?: return
        t.session.close()
        tiles.removeAt(index)
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

private sealed interface TermTarget {
    data class Shell(val index: Int) : TermTarget
    data class Bot(val bot: TerminalBot) : TermTarget
}

/** The terminal screen: shell tiles and Terminal bots on a strip, the screen, and a key bar. */
@Composable
fun TerminalScreen(store: CodeTerminalStore, backend: ChatBackend, onBack: () -> Unit, onOpenPorts: (ptyId: String) -> Unit, onConnect: () -> Unit) {
    var target by remember { mutableStateOf<TermTarget>(TermTarget.Shell(0)) }
    LaunchedEffect(Unit) { store.startIfNeeded() }
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
                    TabChip(tile.name, (target as? TermTarget.Shell)?.index == i, "codeTerminalTab-$i") { target = TermTarget.Shell(i); store.selected = i }
                }
                store.bots?.bots?.forEach { b ->
                    TabChip(b.name.ifEmpty { b.slug }, (target as? TermTarget.Bot)?.bot?.id == b.id, "codeTerminalBot-${b.slug}") { target = TermTarget.Bot(b) }
                }
            }
            if (store.tiles.size < CodeTerminalStore.MAX_TILES) {
                StripButton(DesignR.drawable.ph_plus, "New shell", "codeTerminalNew") { store.addTile(); target = TermTarget.Shell(store.tiles.lastIndex) }
            }
            (target as? TermTarget.Shell)?.let { t ->
                store.tiles.getOrNull(t.index)?.session?.ptyId?.let { id ->
                    StripButton(DesignR.drawable.ph_globe, "Dev servers", "codeTerminalPorts") { onOpenPorts(id) }
                }
                StripButton(DesignR.drawable.ph_x, "Close shell", "codeTerminalClose") {
                    store.closeTile(t.index)
                    target = TermTarget.Shell(store.selected)
                }
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
                } else ShellPane(tile.session, onConnect, Modifier.weight(1f))
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

@Composable
private fun TabChip(label: String, selected: Boolean, tag: String, onClick: () -> Unit) {
    Text(
        label, color = if (selected) TerminalColors.foreground else TerminalColors.secondary, fontSize = 14.sp, fontFamily = CodeMono, maxLines = 1,
        modifier = Modifier.testTag(tag).clip(RoundedCornerShape(10.dp)).background(if (selected) TerminalColors.keyPressed else Color.Transparent)
            .clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 8.dp),
    )
}

@Composable
private fun ShellPane(session: PtySession, onConnect: () -> Unit, modifier: Modifier) {
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
            TerminalSurface(session.screen, session.version, Modifier.weight(1f)) { cols, rows -> session.resize(cols, rows) }
            if (session.state == PtyState.Ended) {
                Row(Modifier.fillMaxWidth().background(TerminalColors.strip).padding(horizontal = 14.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("The shell ended.", color = TerminalColors.secondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    CodePill("Reconnect", testTag = "codeTerminalReconnect", onClick = session::retry)
                }
            }
            KeyBar(onSend = session::send)
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
                KeyBar(onSend = mirror::send)
            }
        }
    }
}

/** The text grid: sizes the emulator to the available space and follows the output. */
@Composable
private fun TerminalSurface(screen: VtScreen, version: Int, modifier: Modifier, onSize: (cols: Int, rows: Int) -> Unit) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val style = remember { TextStyle(fontFamily = CodeMono, fontSize = 12.sp, lineHeight = 16.sp, color = TerminalColors.foreground) }
    val cell = remember(density) { measurer.measure("M", style).size }
    val scroll = rememberScrollState()
    val text = remember(version) { screen.render() }
    LaunchedEffect(version) { scroll.scrollTo(scroll.maxValue) }
    Box(
        modifier.fillMaxWidth().background(TerminalColors.background)
            .onSizeChanged { s ->
                if (cell.width > 0 && cell.height > 0) onSize((s.width / cell.width).coerceIn(20, 300), (s.height / cell.height).coerceIn(5, 200))
            }
            .padding(horizontal = 0.dp),
    ) {
        Text(text, style = style, softWrap = false, modifier = Modifier.fillMaxSize().verticalScroll(scroll).testTag("codeTerminalScreen"))
    }
}

/** The key bar: Esc, Tab, a Ctrl latch, arrows and an input field that types straight into the shell. */
@Composable
private fun KeyBar(onSend: (String) -> Unit) {
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
            KeyButton("Esc") { onSend("\u001b") }
            KeyButton("Tab") { onSend("\t") }
            KeyButton("Ctrl", pressed = ctrl) { ctrl = !ctrl }
            KeyButton("^C") { onSend("\u0003") }
            KeyButton("↑") { onSend("\u001b[A") }
            KeyButton("↓") { onSend("\u001b[B") }
            KeyButton("←") { onSend("\u001b[D") }
            KeyButton("→") { onSend("\u001b[C") }
            KeyButton("⌫") { onSend("\u007f") }
            KeyButton("|") { onSend("|") }
            KeyButton("/") { onSend("/") }
            KeyButton("~") { onSend("~") }
        }
        BasicTextField(
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
private fun KeyButton(label: String, pressed: Boolean = false, onClick: () -> Unit) {
    Text(
        label, color = TerminalColors.foreground, fontSize = 14.sp, fontFamily = CodeMono,
        modifier = Modifier.heightIn(min = 40.dp).width(keyWidth(label)).clip(RoundedCornerShape(8.dp))
            .background(if (pressed) TerminalColors.keyPressed else TerminalColors.key)
            .clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp).testTag("codeTerminalKey-$label"),
        textAlign = androidx.compose.ui.text.style.TextAlign.Center,
    )
}

private fun keyWidth(label: String) = (24 + label.length * 10).coerceAtLeast(44).dp
