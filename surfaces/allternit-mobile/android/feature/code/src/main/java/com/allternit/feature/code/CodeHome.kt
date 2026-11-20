package com.allternit.feature.code

import androidx.compose.ui.text.font.FontWeight
import androidx.compose.foundation.layout.height
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.heading
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
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
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RelayDevice
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

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

/** Owned by the shell for the life of Code mode: the shells and the folder. */
class CodeStore(val backend: ChatBackend) {
    val terminal = CodeTerminalStore(backend)
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
fun CodeHome(host: CodeHost, store: CodeStore, onRoute: (CodeRoute) -> Unit) {
    val c = AllternitTheme.colors
    var folder by remember { mutableStateOf<CodeResult<CodeFolder>?>(null) }
    val greeting = remember { CodeGreetings.random() }
    LaunchedEffect(Unit) { host.reloadSessions() }
    LaunchedEffect(Unit) { folder = CodeApi.folder(host.backend) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).testTag("codeHome")) {
        if (folder == CodeResult.NoComputer) {
            // iOS ConnectComputerView over the launchpad.
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp).testTag("codeHomeNoComputer"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("No computer connected", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                CodePill("Connect", testTag = "codeHomeConnect") { onRoute(CodeRoute.Computer) }
            }
        }
        Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, top = 28.dp, bottom = 24.dp)) {
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
        CodeRoute.Overview -> CodeOverviewScreen(host, onBack = onClose, onOpen = { host.openSession(it); onClose() })
        CodeRoute.Computer -> CodeComputerScreen(host, onBack = onClose)
        CodeRoute.Usage -> CodeUsageScreen(host, onBack = onClose)
        CodeRoute.Scheduled -> CodeScheduledScreen(host, onBack = onClose)
        is CodeRoute.Permissions -> PermissionsScreen(host.backend, route.sessionId, onBack = onClose)
    }
}

/** The sessions overview (iOS PhoneCodeSessionsView / CodeOverviewSheet): filters, counts, row actions. */
@Composable
internal fun CodeOverviewScreen(host: CodeHost, onBack: () -> Unit, onOpen: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var filter by remember { mutableStateOf<CodeSessionStatus?>(null) }
    var query by remember { mutableStateOf("") }
    var acting by remember { mutableStateOf<CodeSessionRow?>(null) }
    LaunchedEffect(Unit) { host.reloadSessions() }
    val all = host.sessions()
    val shown = all.filter { (filter == null && it.status != CodeSessionStatus.Archived || it.status == filter) && it.title.contains(query, ignoreCase = true) }
    CodeScaffold(title = "Code threads", subtitle = "${all.count { it.status != CodeSessionStatus.Archived }} threads", onBack = onBack, backTag = "codeOverviewBack") {
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
        if (shown.isEmpty()) {
            EmptyState(
                if (filter == null) "No threads yet" else "Nothing ${filter?.label?.lowercase()}",
                "Start a thread from the composer and it appears here.",
            )
        } else {
            LazyColumn(Modifier.weight(1f).fillMaxWidth().testTag("codeOverviewList")) {
                items(shown, key = { it.id }) { s -> SessionRow(s, onMore = { acting = s }, onClick = { onOpen(s.id) }) }
            }
        }
    }
    acting?.let { s ->
        ThreadActionsSheet(host, s, onDismiss = { acting = null }, onChanged = { scope.launch { host.reloadSessions() } })
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
                if (s.status != CodeSessionStatus.Archived) CodePill("Archive", testTag = "codeThreadArchive", tint = AllternitStatic.StatusError) { act { CodeApi.archive(s.id) } }
            }
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
    CodeScaffold(title = "Computer", subtitle = "Where Code runs your commands", onBack = onBack, backTag = "codeComputerBack") {
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
            else -> LazyColumn(Modifier.fillMaxSize().testTag("codeComputerList")) {
                item {
                    CodeRow(
                        title = "Automatic", detail = "Use whichever of your computers is online", testTag = "codeComputerAuto",
                        trailing = if (selected == null) ({ Icon(painterResource(DesignR.drawable.ph_check), "Selected", tint = c.accentCode, modifier = Modifier.size(18.dp)) }) else null,
                        onClick = { host.backend.relay.selectedRuntimeId = null; selected = null },
                    )
                }
                items(list, key = { it.id }) { d ->
                    CodeRow(
                        title = d.name.ifEmpty { "Computer" },
                        detail = listOf(d.runtimeType, d.status).filter { it.isNotEmpty() }.joinToString(" · "),
                        testTag = "codeComputer-${d.id}",
                        trailing = if (selected == d.id) ({ Icon(painterResource(DesignR.drawable.ph_check), "Selected", tint = c.accentCode, modifier = Modifier.size(18.dp)) }) else null,
                        onClick = { host.backend.relay.selectedRuntimeId = d.id; selected = d.id },
                    )
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
