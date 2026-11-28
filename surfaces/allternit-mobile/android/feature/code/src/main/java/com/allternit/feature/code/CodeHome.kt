package com.allternit.feature.code

import androidx.compose.ui.text.font.FontWeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Spacer
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.heading
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.width
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.customActions
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.friendly
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.Haptics
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.rememberDockOffset
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RelayDevice
import com.allternit.feature.chat.DeviceRow
import com.allternit.feature.chat.sortedForPicker
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.InlineEmpty

/*
 * Code mode (iOS CodeModeView, CodeOverviewSheet, PhoneCodeSessionsView, CodeRuntimePairingView).
 * The shell hosts the thread itself (its chat screen, opened on a code session) and the composer;
 * this module supplies the Code home under the composer, the sessions overview, the computer
 * picker and every tool a thread opens: Changes, Files, Terminal, Dev servers and Permissions.
 */

/** iOS CodeSessionFilterStatus: first match wins (archived, needs input, working, ready, done, idle). */
enum class CodeSessionStatus(val label: String) {
    Archived("Archived"), NeedsInput("Needs input"), Working("Working"), ReadyForReview("Ready for review"), Done("Done"), Idle("");
}

/** A Code thread as the shell knows it. */
data class CodeSessionRow(val id: String, val title: String, val updatedMillis: Long, val status: CodeSessionStatus, val model: String? = null, val messages: Int = 0)

/** What Code mode needs from the shell. */
class CodeHost(
    val backend: ChatBackend,
    /** The Code threads, newest first. */
    val sessions: () -> List<CodeSessionRow>,
    val reloadSessions: suspend () -> Unit,
    /** Opens a thread in the shell's chat screen. */
    val openSession: (String) -> Unit,
    /** The thread on screen, if any (Changes and Permissions act on it). */
    val currentSessionId: () -> String?,
    /** Apps & Extensions, where a computer is connected. */
    val openConnections: () -> Unit,
)

/**
 * Owned by the shell for the life of Code mode: the shells, and the computer's terminals and live agent
 * sessions. [sessions] lives here, one level above the threads screen, so a Move keeps being followed
 * after that screen closes and its terminal still opens when it is done (iOS CodeModeView owns its
 * TerminalSessionsStore the same way).
 */
class CodeStore(val backend: ChatBackend) {
    val terminal = CodeTerminalStore(backend)
    val sessions = TerminalSessionsStore(backend)

    /**
     * Opens the computer's terminal [terminalId] in a Terminal tile and shows the Terminal ([onRoute] is the
     * shell's route setter, which outlives the screen that asked). Says so on the threads screen when every
     * tile is taken.
     */
    fun openTerminal(terminalId: String, name: String, onRoute: (CodeRoute) -> Unit) {
        if (terminal.open(terminalId, name)) {
            sessions.notice = null
            onRoute(CodeRoute.Terminal)
        } else {
            sessions.notice = "All ${CodeTerminalStore.MAX_TILES} terminal tabs are in use. Close one in Terminal to open this."
        }
    }
}

/** Every screen a Code tool can open. */
sealed interface CodeRoute {
    data object Terminal : CodeRoute
    data object Files : CodeRoute
    data class Changes(val sessionId: String) : CodeRoute
    data class Ports(val ptyId: String) : CodeRoute
    data object Overview : CodeRoute
    data object Computer : CodeRoute
    data object Usage : CodeRoute
    data object Scheduled : CodeRoute
    data class Permissions(val sessionId: String) : CodeRoute
}

/** iOS CodeModeView.greetings: one is picked each time the launchpad opens. */
private val CodeGreetings = listOf(
    "What's up next?",
    "Ready to ship something?",
    "What are we building today?",
    "Pick a task and let's run it.",
    "Need a hand with the codebase?",
)

/**
 * The Code launchpad above the composer (iOS CodeModeView.launchpad): the serif greeting, one
 * line, and Show usage. Terminal, Files and Computer live in the composer's deck row and the
 * thread's Panes, and threads in the Code sessions view (the checklist in the header), as on iOS.
 */
@Composable
fun CodeHome(host: CodeHost, store: CodeStore, onRoute: (CodeRoute) -> Unit, onNewSession: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    var folder by remember { mutableStateOf<CodeResult<CodeFolder>?>(null) }
    val greeting = remember { CodeGreetings.random() }
    LaunchedEffect(Unit) { host.reloadSessions() }
    LaunchedEffect(Unit) { folder = CodeApi.folder(host.backend) }
    val homeScroll = rememberScrollState()
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().verticalScroll(homeScroll).testTag("codeHome")) {
        Spacer(Modifier.height(96.dp))
        if (folder == CodeResult.NoComputer) {
            // iOS ConnectComputerView over the launchpad.
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp).testTag("codeHomeNoComputer"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("No computer connected", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                CodePill("Connect", testTag = "codeHomeConnect") { onRoute(CodeRoute.Computer) }
            }
        }
        Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 28.dp, bottom = 24.dp)) {
            com.allternit.core.design.GizziWalk(Modifier.padding(bottom = 8.dp))
            Text(
                greeting,
                color = c.textPrimary,
                fontSize = 28.sp,
                lineHeight = 34.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = androidx.compose.ui.text.font.FontFamily.Serif,
                letterSpacing = (-0.56).sp,
                modifier = Modifier.testTag("codeGreeting").semantics { heading() },
            )
            Text(
                "Run a command or describe a task to start coding.",
                color = c.textSecondary,
                fontSize = 14.sp,
                lineHeight = 20.sp,
                modifier = Modifier.padding(top = 8.dp).testTag("codeEmpty"),
            )
            if (onNewSession != null) com.allternit.core.design.EmptyPill("New session", Modifier.padding(top = 16.dp).testTag("codeNewSession"), onNewSession)
            Row(
                Modifier
                    .padding(top = 24.dp)
                    .height(32.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(c.bgTertiary)
                    .testTag("codeShowUsage")
                    .clickable { onRoute(CodeRoute.Usage) }
                    .padding(horizontal = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_chart_bar), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(13.dp))
                Text("Show usage", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
    // The block title docks into the bar as the page scrolls (UI feedback 11-25 A4).
    com.allternit.core.design.DockingBlockTitle("Code", homeScroll, modifier = Modifier.fillMaxSize())
    }
}

@Composable
private fun SessionRow(s: CodeSessionRow, onMore: (() -> Unit)? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    CodeRow(
        title = s.title,
        detail = listOf(s.status.label, relativeTime(s.updatedMillis)).filter { it.isNotEmpty() }.joinToString(" · "),
        testTag = "codeSession-${s.id}",
        trailing = if (onMore != null) ({ Text("⋯", color = c.textSecondary, fontSize = 22.sp, modifier = Modifier.testTag("codeSessionMore-${s.id}").clickable(onClick = onMore).padding(horizontal = 12.dp, vertical = 4.dp)) }) else null,
        onClick = onClick,
    )
}

/** Routes the Code tools to their screens, full screen over the shell. */
@Composable
fun CodeRouteScreen(host: CodeHost, store: CodeStore, route: CodeRoute, onRoute: (CodeRoute) -> Unit, onClose: () -> Unit) {
    when (route) {
        CodeRoute.Terminal -> TerminalScreen(store.terminal, host.backend, onBack = onClose, onOpenPorts = { onRoute(CodeRoute.Ports(it)) }, onConnect = { onRoute(CodeRoute.Computer) })
        CodeRoute.Files -> FileBrowserScreen(host.backend, null, onBack = onClose)
        is CodeRoute.Changes -> SessionDiffScreen(host.backend, route.sessionId, onBack = onClose)
        is CodeRoute.Ports -> DevServerPortsScreen(host.backend, route.ptyId, onBack = { onRoute(CodeRoute.Terminal) })
        CodeRoute.Overview -> CodeOverviewScreen(host, store, onRoute = onRoute, onBack = onClose, onOpen = { host.openSession(it); onClose() })
        CodeRoute.Computer -> CodeComputerScreen(host, onBack = onClose)
        CodeRoute.Usage -> CodeUsageScreen(host, onBack = onClose)
        CodeRoute.Scheduled -> CodeScheduledScreen(host, onBack = onClose)
        is CodeRoute.Permissions -> PermissionsScreen(host.backend, route.sessionId, onBack = onClose)
    }
}

/**
 * The sessions overview (iOS PhoneCodeSessionsView / CodeOverviewSheet): the computer's terminals with live
 * previews, "On your computer" (agent sessions running outside Allternit, with Move here / Open a copy /
 * New session here), then the threads with filters, counts and row actions. The two computer sections
 * poll only while this screen is on screen and the app is in the foreground, and hide on a runtime
 * without their routes, with no computer, or signed out.
 */
@Composable
internal fun CodeOverviewScreen(host: CodeHost, store: CodeStore, onRoute: (CodeRoute) -> Unit, onBack: () -> Unit, onOpen: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var filter by remember { mutableStateOf<CodeSessionStatus?>(null) }
    var query by remember { mutableStateOf("") }
    var acting by remember { mutableStateOf<CodeSessionRow?>(null) }
    val context = androidx.compose.ui.platform.LocalContext.current
    // List · Grid · Deck (iOS OverviewLayoutSwitch), kept across launches; the computer sections follow it.
    var layout by remember { mutableStateOf(LibraryLayout.load(context, "code_overview", LibraryLayout.List)) }
    // Row whose long-press opened "Hold to end and delete"; terminals just closed (the 2 s poll catches up).
    var armed by remember { mutableStateOf<String?>(null) }
    var hiddenTerminals by remember { mutableStateOf(emptySet<String>()) }
    var deleteError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { host.reloadSessions() }
    val live = store.sessions
    val owner = LocalLifecycleOwner.current
    DisposableEffect(owner) {
        // Adding the observer replays ON_RESUME when the screen is already in the foreground.
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) live.setActive(true) else if (event == Lifecycle.Event.ON_PAUSE) live.setActive(false)
        }
        owner.lifecycle.addObserver(observer)
        onDispose {
            owner.lifecycle.removeObserver(observer)
            live.setActive(false)
        }
    }
    val openTerminal: (String, String) -> Unit = { id, name -> store.openTerminal(id, name, onRoute) }
    val all = host.sessions()
    val listState = androidx.compose.foundation.lazy.rememberLazyListState()
    val shown = all.filter { (filter == null && it.status != CodeSessionStatus.Archived || it.status == filter) && it.title.contains(query, ignoreCase = true) }
    // The computer sections sit with All (a status filter is about threads) and follow the search.
    val terminals = if (filter != null) emptyList() else live.terminals.filter { it.sessionId !in hiddenTerminals }.filter {
        query.isEmpty() || TerminalPreview.title(it).contains(query, ignoreCase = true) || it.cwd.contains(query, ignoreCase = true)
    }
    val external = if (filter != null) emptyList() else live.rows.filter {
        query.isEmpty() || OnYourComputer.displayName(it).contains(query, ignoreCase = true) || it.cwd.contains(query, ignoreCase = true)
    }
    val computerSections = terminals.isNotEmpty() || external.isNotEmpty()
    CodeScaffold(title = "Code threads", subtitle = "${all.count { it.status != CodeSessionStatus.Archived }} threads", onBack = onBack, backTag = "codeOverviewBack",
        trailing = { com.allternit.feature.artifacts.LayoutCycleButton(LibraryLayout.entries, layout, { it.label }, { layout = it; LibraryLayout.save(context, "code_overview", it) }, tag = "codeOverviewLayoutCycle") },
        dockScroll = rememberDockOffset(listState)) {
        Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
            if (query.isEmpty()) Text("Search threads", color = c.textTertiary, fontSize = 15.sp)
            BasicTextField(query, { query = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.accentCode), modifier = Modifier.fillMaxWidth().testTag("codeOverviewSearch"))
        }
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CodePill("All", testTag = "codeFilter-all", filled = filter == null) { filter = null }
            listOf(CodeSessionStatus.NeedsInput, CodeSessionStatus.Working, CodeSessionStatus.ReadyForReview, CodeSessionStatus.Done, CodeSessionStatus.Archived).forEach { st ->
                val n = all.count { it.status == st }
                CodePill("${st.label} $n", testTag = "codeFilter-${st.name}", filled = filter == st) { filter = st }
            }
        }
        deleteError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp).testTag("codeOverviewDeleteError")) }
        live.notice?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp).testTag("codeOverviewNotice")) }
        if (shown.isEmpty() && !computerSections) {
            EmptyState(
                if (filter == null) "No threads yet" else "Nothing ${filter?.label?.lowercase()}",
                "Start a thread from the composer and it appears here.",
                art = EmptyArtKind.Envelope,
            )
        } else {
            LazyColumn(Modifier.weight(1f).fillMaxWidth().testTag("codeOverviewList"), state = listState, contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 96.dp)) {
                if (terminals.isNotEmpty()) {
                    item(key = "terminals-label") { SectionLabel("Terminals") }
                    // List keeps the 2-row preview; Grid and Deck show more rows on a card.
                    overviewCells(layout, terminals, { "terminal-${it.sessionId}" }) { t, cell ->
                        val key = "terminal-${t.sessionId}"
                        TerminalPreviewRow(
                            t, if (cell == LibraryLayout.List) TerminalPreview.Layout.List else if (cell == LibraryLayout.Grid) TerminalPreview.Layout.Grid else TerminalPreview.Layout.Deck,
                            padded = cell == LibraryLayout.List,
                            onDelete = { armed = null; scope.launch { CodeApi.closeTerminal(host.backend, t.sessionId); hiddenTerminals = hiddenTerminals + t.sessionId } },
                            armed = armed == key, onArm = { armed = key }, onDisarm = { armed = null },
                        ) { openTerminal(t.sessionId, TerminalPreview.title(t)) }
                    }
                }
                if (external.isNotEmpty()) {
                    item(key = "computer-label") { SectionLabel("On your computer") }
                    overviewCells(layout, external, { "live-${it.sessionId}" }) { s, _ -> OnYourComputerRow(s, live, openTerminal) }
                }
                if (computerSections) item(key = "threads-label") { SectionLabel("Threads") }
                if (shown.isEmpty()) {
                    item(key = "threads-empty") {
                        InlineEmpty(
                            if (filter == null) "No threads yet. Start one from the composer." else "Nothing ${filter?.label?.lowercase()}",
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp).testTag("codeOverviewNoThreads"),
                            art = EmptyArtKind.Envelope,
                        )
                    }
                } else {
                    overviewCells(layout, shown, { it.id }) { s, cell ->
                        ThreadItem(
                            host.backend, s, card = cell != LibraryLayout.List, deck = cell == LibraryLayout.Deck,
                            armed = armed == s.id, onArm = { armed = s.id }, onDisarm = { armed = null },
                            onDelete = {
                                armed = null
                                scope.launch {
                                    deleteError = null
                                    val r = host.backend.deleteSession(s.id)
                                    if (r is ApiResult.Ok) host.reloadSessions() else deleteError = r.friendly("delete the thread")
                                }
                            },
                            onMore = { acting = s }, onClick = { onOpen(s.id) },
                        )
                    }
                }
            }
        }
    }
    acting?.let { s ->
        ThreadActionsSheet(host, s, onDismiss = { acting = null }, onChanged = { scope.launch { host.reloadSessions() } })
    }
}

/**
 * One section of an overview in the chosen layout: List = full-width rows; Grid = rows of two cards;
 * Deck = one sideways row of tall cards (iOS OverviewCards, deck cards 226 wide).
 */
private fun <T> androidx.compose.foundation.lazy.LazyListScope.overviewCells(
    layout: LibraryLayout,
    rows: List<T>,
    key: (T) -> String,
    cell: @Composable (T, LibraryLayout) -> Unit,
) {
    when (layout) {
        LibraryLayout.List -> items(rows, key = key) { cell(it, LibraryLayout.List) }
        LibraryLayout.Grid -> items(rows.chunked(2), key = { pair -> key(pair.first()) }) { pair ->
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 5.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                pair.forEach { Box(Modifier.weight(1f).overviewCard()) { cell(it, LibraryLayout.Grid) } }
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }
        LibraryLayout.Deck -> item(key = "deck-" + (rows.firstOrNull()?.let(key) ?: "")) {
            androidx.compose.foundation.lazy.LazyRow(contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                items(rows, key = key) { Box(Modifier.width(226.dp).overviewCard()) { cell(it, LibraryLayout.Deck) } }
            }
        }
    }
}

@Composable
private fun Modifier.overviewCard(): Modifier {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    return clip(shape).background(c.bgPanel).border(1.dp, c.borderSubtle, shape).padding(horizontal = 12.dp, vertical = 4.dp)
}

/** "Hold to ..." strip a long-press opens under a row or card: the 1 s hold fills it, Cancel closes it. */
@Composable
internal fun DeleteStrip(title: String, testTag: String, onHeld: () -> Unit, onCancel: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(top = 4.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        com.allternit.core.design.HoldButton(title, testTag, modifier = Modifier.weight(1f), danger = true, onHeld = onHeld)
        Text("Cancel", color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, modifier = Modifier.testTag("$testTag-cancel").clickable(role = Role.Button, onClick = onCancel).padding(horizontal = 10.dp, vertical = 12.dp))
    }
}

/** A Code thread as a list row or a card; tap opens, long-press arms "Hold to delete", ⋯ opens the actions sheet. */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun ThreadItem(
    backend: ChatBackend,
    s: CodeSessionRow,
    card: Boolean,
    deck: Boolean,
    armed: Boolean,
    onArm: () -> Unit,
    onDisarm: () -> Unit,
    onDelete: () -> Unit,
    onMore: () -> Unit,
    onClick: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = androidx.compose.ui.platform.LocalView.current
    val age = relativeTime(s.updatedMillis)
    val detail = listOf(s.status.label, age).filter { it.isNotEmpty() }.joinToString(" · ")
    // The newest message, one line (loaded when the row scrolls into view; empty threads have none).
    var lastLine by remember(s.id) { mutableStateOf<String?>(null) }
    LaunchedEffect(s.id, s.messages) {
        if (s.messages > 0) {
            val r = backend.listMessages(s.id)
            if (r is ApiResult.Ok) lastLine = r.value.lastOrNull { it.content.isNotBlank() }?.content?.replace('\n', ' ')?.trim()
        }
    }
    val dot = when (s.status) {
        CodeSessionStatus.NeedsInput -> AllternitStatic.StatusWarning
        CodeSessionStatus.Working -> c.accentCode
        CodeSessionStatus.ReadyForReview -> c.accentCode
        CodeSessionStatus.Done -> AllternitStatic.StatusSuccess
        else -> c.textTertiary
    }
    Column(
        Modifier.fillMaxWidth()
            .then(if (deck) Modifier.heightIn(min = 200.dp) else Modifier)
            .testTag("codeSession-${s.id}")
            .combinedClickable(role = Role.Button, onLongClickLabel = "Delete this thread", onLongClick = { Haptics.perform(view, Haptics.Kind.Tap); onArm() }) {
                Haptics.perform(view, Haptics.Kind.Tap)
                if (armed) onDisarm() else onClick()
            }
            .semantics { customActions = listOf(CustomAccessibilityAction("Delete") { onArm(); true }) }
            .padding(horizontal = if (card) 0.dp else 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.padding(top = 7.dp).size(8.dp).clip(androidx.compose.foundation.shape.CircleShape).background(dot).testTag("codeSessionDot-${s.id}"))
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(s.title, color = c.textPrimary, fontSize = 16.sp, maxLines = if (card) 3 else 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                    if (!card && age.isNotEmpty()) Text(age, color = c.textTertiary, fontSize = 12.sp, maxLines = 1)
                }
                Text(lastLine ?: detail, color = c.textSecondary, fontSize = 13.sp, maxLines = if (card) 3 else 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                if (card) Text(detail, color = c.textTertiary, fontSize = 11.sp, maxLines = 1)
            }
            Text("⋯", color = c.textSecondary, fontSize = 22.sp, modifier = Modifier.testTag("codeSessionMore-${s.id}").clickable(onClick = onMore).padding(horizontal = 8.dp, vertical = 2.dp))
        }
        if (armed) DeleteStrip("Hold to delete", "codeThreadDelete-${s.id}", onHeld = onDelete, onCancel = onDisarm)
    }
}

/** Rename, Mark reviewed, Archive for one thread (iOS CodeSessionsClient), all by tap. */
@Composable
private fun ThreadActionsSheet(host: CodeHost, s: CodeSessionRow, onDismiss: () -> Unit, onChanged: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(s.title) }
    var error by remember { mutableStateOf<String?>(null) }
    fun act(block: suspend () -> String?) {
        scope.launch {
            val failure = block()
            if (failure == null) { onChanged(); onDismiss() } else error = failure
        }
    }
    LibrarySheet(title = "Thread", testTag = "codeThreadSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
                BasicTextField(name, { name = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp), cursorBrush = SolidColor(c.accentCode), modifier = Modifier.fillMaxWidth().testTag("codeThreadName"))
            }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                CodePill("Rename", testTag = "codeThreadRename", filled = true, enabled = name.isNotBlank() && name != s.title) { act { CodeApi.rename(s.id, name.trim()) } }
                if (s.status == CodeSessionStatus.ReadyForReview) CodePill("Mark reviewed", testTag = "codeThreadReviewed") { act { CodeApi.markReviewed(s.id) } }
            }
            // Deleting a thread is a hold, never a dialog (the web ConfirmMorph hold mode).
            if (s.status != CodeSessionStatus.Archived) com.allternit.core.design.HoldButton("Hold to delete", "codeThreadArchive", danger = true) { act { CodeApi.archive(s.id) } }
            Box(Modifier.padding(bottom = 16.dp))
        }
    }
}

/** The permission mode of a thread (iOS CodePermissionMode picker): Manual, Auto-accept, Plan, Read only. */
@Composable
internal fun PermissionsScreen(backend: ChatBackend, sessionId: String, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var applied by remember { mutableStateOf<CodePermissionMode?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    CodeScaffold(title = "Permissions", subtitle = "What this thread may do without asking", onBack = onBack, backTag = "codePermissionsBack") {
        CodePermissionMode.entries.forEach { m ->
            CodeRow(
                title = m.label, detail = m.blurb, testTag = "codePermission-${m.raw}",
                trailing = if (applied == m) ({ Icon(painterResource(DesignR.drawable.ph_check), "Applied", tint = c.accentCode, modifier = Modifier.size(18.dp)) }) else null,
                onClick = { scope.launch { error = CodeApi.applyPermission(sessionId, m); if (error == null) applied = m } },
            )
        }
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }
    }
}

/** The computer Code runs on (iOS CodeRuntimePairingView): pick one, or leave it automatic. */
@Composable
internal fun CodeComputerScreen(host: CodeHost, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    var tick by remember { mutableStateOf(0) }
    var devices by remember { mutableStateOf<List<RelayDevice>?>(null) }
    var failed by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf(host.backend.relay.selectedRuntimeId) }
    LaunchedEffect(tick) {
        val list = host.backend.relay.listDevices()
        failed = list == null
        devices = list
    }
    val listState = androidx.compose.foundation.lazy.rememberLazyListState()
    CodeScaffold(title = "Computer", subtitle = "Where Code runs your commands", onBack = onBack, backTag = "codeComputerBack", dockScroll = rememberDockOffset(listState)) {
        val list = devices
        when {
            list == null && !failed -> Loading()
            failed -> ErrorState("Couldn't load your computers", "Check your connection, then try again.", onRetry = { devices = null; failed = false; tick++ })
            list.isNullOrEmpty() -> Column(Modifier.fillMaxSize()) {
                NoComputerState(onRetry = { devices = null; tick++ })
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.Center) {
                    CodePill("Open Apps & Extensions", testTag = "codeComputerConnect") { host.openConnections() }
                }
            }
            else -> LazyColumn(Modifier.fillMaxSize().testTag("codeComputerList"), state = listState) {
                item {
                    CodeRow(
                        title = "Automatic", detail = "Use whichever of your computers is online", testTag = "codeComputerAuto",
                        trailing = if (selected == null) ({ Icon(painterResource(DesignR.drawable.ph_check), "Selected", tint = c.accentCode, modifier = Modifier.size(18.dp)) }) else null,
                        onClick = { host.backend.relay.selectedRuntimeId = null; selected = null },
                    )
                }
                // Start anywhere D1: the same device rows as the chat's + sheet and chip (shared DeviceRow).
                items(list.sortedForPicker(), key = { it.id }) { d ->
                    DeviceRow(d, selected = selected == d.id, testTag = "codeComputer-${d.id}") {
                        host.backend.relay.selectedRuntimeId = d.id; selected = d.id
                    }
                }
            }
        }
    }
}

/**
 * The tools a Code thread opens (iOS CodePanesMenu, as a tap sheet), plus the pending-change
 * banner. The shell draws it over a Code thread.
 */
@Composable
fun CodeThreadTools(host: CodeHost, sessionId: String?, onRoute: (CodeRoute) -> Unit, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Column(modifier) {
        PendingChangesBanner(host.backend, sessionId)
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), horizontalArrangement = Arrangement.End) {
            CodePill("Code tools", testTag = "codeThreadTools") { open = true }
        }
    }
    if (open) {
        LibrarySheet(title = "Code tools", testTag = "codeToolsSheet", onDismiss = { open = false }, leading = LibrarySheetAction("Close", onClick = { open = false })) {
            Column(Modifier.fillMaxWidth().padding(bottom = 16.dp)) {
                fun go(r: CodeRoute) { open = false; onRoute(r) }
                if (sessionId != null) {
                    CodeRow("Changes", "Files this thread edited", "codeTool-changes") { go(CodeRoute.Changes(sessionId)) }
                    CodeRow("Permissions", "What this thread may do without asking", "codeTool-permissions") { go(CodeRoute.Permissions(sessionId)) }
                }
                CodeRow("Files", "Browse and edit the working folder", "codeTool-files") { go(CodeRoute.Files) }
                CodeRow("Terminal", "Shell and Terminal bots", "codeTool-terminal") { go(CodeRoute.Terminal) }
                CodeRow("All threads", "Filters, rename, archive", "codeTool-overview") { go(CodeRoute.Overview) }
                CodeRow("Scheduled", "Routines on your computer", "codeTool-scheduled") { go(CodeRoute.Scheduled) }
                CodeRow("Usage", "Your Code activity", "codeTool-usage") { go(CodeRoute.Usage) }
                CodeRow("Computer", "Where Code runs", "codeTool-computer") { go(CodeRoute.Computer) }
            }
        }
    }
}
