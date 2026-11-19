package com.allternit.feature.cowork

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.material3.Icon
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** What the shell gives Cowork: the relay, the menu button, sign-in and the chat for a session. */
class CoworkHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val menuButton: @Composable () -> Unit,
    val openSignIn: () -> Unit,
    /** Opens a Cowork session's conversation in the shell's chat (iOS ChatContentView on a cowork session). */
    val openChat: (sessionId: String) -> Unit,
)

private sealed interface CoworkPage {
    data object Launchpad : CoworkPage
    data class Session(val id: String) : CoworkPage
    data object Approvals : CoworkPage
    data class Task(val id: String) : CoworkPage
}

private fun statusColor(s: TaskStatus, secondary: Color): Color = when (s) {
    TaskStatus.Backlog, TaskStatus.Todo -> secondary
    TaskStatus.InProgress -> AllternitStatic.StatusInfo
    TaskStatus.InReview -> AllternitStatic.StatusWarning
    TaskStatus.Done -> AllternitStatic.StatusSuccess
}

/** The drawer's Cowork section (iOS CoworkWorkspaceView). */
@Composable
fun CoworkRoot(host: CoworkHost, initialTasks: Boolean = false) {
    val api = remember(host.backend) { CoworkApi(host.backend) }
    val tasks = remember(api) { CoworkTasksState(api) }
    var page by remember { mutableStateOf<CoworkPage>(CoworkPage.Launchpad) }
    var tasksOpen by remember(initialTasks) { mutableStateOf(initialTasks) }
    BackHandler(enabled = page != CoworkPage.Launchpad && !tasksOpen) { page = CoworkPage.Launchpad }
    if (tasksOpen) {
        CoworkTasksScreen(api, tasks, onBack = { tasksOpen = false })
        return
    }
    when (val p = page) {
        CoworkPage.Launchpad -> CoworkLaunchpad(host, api, onOpen = { page = CoworkPage.Session(it) }, onApprovals = { page = CoworkPage.Approvals })
        is CoworkPage.Session -> CoworkSessionScreen(host, api, p.id, onBack = { page = CoworkPage.Launchpad }, onTasks = { tasksOpen = true })
        CoworkPage.Approvals -> FabricApprovalsScreen(api, onBack = { page = CoworkPage.Launchpad })
        is CoworkPage.Task -> Unit
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun CoworkLaunchpad(host: CoworkHost, api: CoworkApi, onOpen: (String) -> Unit, onApprovals: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var sessions by remember { mutableStateOf<List<CoworkSession>>(emptyList()) }
    var loading by remember { mutableStateOf(false) }
    var loaded by remember { mutableStateOf(false) }
    var noComputer by remember { mutableStateOf(false) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var createError by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var attempt by remember { mutableStateOf(0) }

    LaunchedEffect(host.signedIn, attempt) {
        if (!host.signedIn) return@LaunchedEffect
        loading = true
        when (val r = api.sessions()) {
            is CRes.Ok -> { sessions = r.value; loadError = null; noComputer = false }
            CRes.NoComputer -> { noComputer = true; loadError = null }
            is CRes.Failed -> loadError = r.message
        }
        loading = false
        loaded = true
    }
    fun startNew() {
        creating = true; createError = null
        scope.launch {
            when (val r = api.createSession("New Cowork Session")) {
                is CRes.Ok -> { creating = false; onOpen(r.value) }
                CRes.NoComputer -> { creating = false; noComputer = true }
                is CRes.Failed -> { creating = false; createError = r.message }
            }
        }
    }

    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("coworkWorkspace")) {
        LibraryHeader(title = "Cowork", onBack = null, leading = host.menuButton, trailing = {
            LibraryIconButton(DesignR.drawable.ph_arrows_down_up, "Fabric Transport", "coworkFabricButton", onClick = onApprovals)
        })
        if (!host.signedIn) {
            FriendlyState(FriendlyStyle.Empty, "Sign in to use Cowork", "Your sessions and tasks live in your account.",
                Modifier.padding(horizontal = 20.dp), "Sign in", host.openSignIn) { Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary) }
            return@Column
        }
        PullToRefreshBox(isRefreshing = loading && loaded, onRefresh = { attempt++ }, modifier = Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("Start collaborating", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).testTag("coworkNewSession")
                            .clickable(enabled = !creating, role = Role.Button) { startNew() }.padding(horizontal = 16.dp, vertical = 14.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.accentPrimary, modifier = Modifier.size(24.dp))
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text("New Cowork Session", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                            Text("Plan, build, and review with an agent.", color = c.textSecondary, fontSize = 14.sp)
                        }
                        if (creating) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
                        else Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
                    }
                }
                createError?.let {
                    FriendlyState(FriendlyStyle.Error, "Couldn't create session", it, Modifier, "Retry", { startNew() }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError) }
                }
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Recent Sessions", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        if (loading) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    }
                    when {
                        noComputer -> FriendlyState(FriendlyStyle.Empty, "No computer yet", "Cowork sessions run on your computer. Connect one and they show up here.") { Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary) }
                        loadError != null && sessions.isEmpty() -> FriendlyState(FriendlyStyle.Error, "Couldn't load sessions", loadError.orEmpty(), Modifier, "Retry", { attempt++ }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError) }
                        sessions.isEmpty() && loaded -> FriendlyState(FriendlyStyle.Empty, "No sessions yet", "Start your first Cowork session above.") { Icon(painterResource(DesignR.drawable.ph_book_open), null, tint = c.textSecondary) }
                        else -> sessions.forEach { s ->
                            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                Column(Modifier.weight(1f).clickable(role = Role.Button) { onOpen(s.id) }.testTag("coworkSession${s.id}"), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                    Text(s.displayTitle, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    Text(s.createdAt, color = c.textSecondary, fontSize = 12.sp)
                                }
                                LibraryIconButton(DesignR.drawable.ph_trash, "Delete session", "coworkDelete${s.id}", tint = AllternitStatic.StatusError, iconSize = 18) {
                                    scope.launch { if (api.deleteSession(s.id) is CRes.Ok) sessions = sessions.filterNot { it.id == s.id } else createError = "Couldn't delete the session. Try again." }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** iOS CoworkSessionWorkspaceView: the session's bar, its conversation (the shell chat) and the tasks sheet. */
@Composable
private fun CoworkSessionScreen(host: CoworkHost, api: CoworkApi, id: String, onBack: () -> Unit, onTasks: () -> Unit) {
    val c = AllternitTheme.colors
    var title by remember { mutableStateOf("Cowork Session") }
    LaunchedEffect(id) { (api.sessions() as? CRes.Ok)?.value?.firstOrNull { it.id == id }?.let { title = it.displayTitle } }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("coworkSessionWorkspace")) {
        LibraryHeader(title = title, subtitle = "Cowork", onBack = onBack, backTag = "coworkSessionClose", trailing = {
            LibraryIconButton(DesignR.drawable.ph_check_circle_fill, "Tasks", "coworkSessionTasks", onClick = onTasks)
        })
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).clickable(role = Role.Button) { host.openChat(id) }.testTag("coworkOpenConversation").padding(16.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text("Open the conversation", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text("Plan, build and review with your agent in this session's chat.", color = c.textSecondary, fontSize = 14.sp)
                }
                Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            }
        }
    }
}

/** iOS CoworkTasksListView (+ detail, new-task sheet). Segments Tasks / Agent Tasks; tap a row for its detail. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CoworkTasksScreen(api: CoworkApi, state: CoworkTasksState, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var agentSegment by remember { mutableStateOf(false) }
    var detailId by remember { mutableStateOf<String?>(null) }
    var creating by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { if (!state.loaded) state.refresh() }
    BackHandler { if (detailId != null) detailId = null else onBack() }

    detailId?.let { id ->
        val task = state.task(id)
        if (task == null) detailId = null else {
            CoworkTaskDetail(state, task, onBack = { detailId = null })
            return
        }
    }
    val visible = state.tasks.filter { if (agentSegment) it.isAgent else !it.isAgent }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("coworkTasks")) {
        LibraryHeader(title = "Cowork Tasks", onBack = onBack, backTag = "coworkTasksClose", trailing = {
            LibraryIconButton(DesignR.drawable.ph_plus, "New task", "coworkNewTask") { creating = true }
        })
        PillRow(listOf(false, true), agentSegment, { if (it) "Agent Tasks" else "Tasks" }, { if (it) "coworkSegAgent" else "coworkSegTasks" }, Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { agentSegment = it }
        actionError?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp)) }
        when {
            state.loading && state.tasks.isEmpty() -> Box(Modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator(color = c.textSecondary) }
            state.noComputer && state.tasks.isEmpty() -> FriendlyState(FriendlyStyle.Empty, "No computer yet", "Tasks show up here once a computer is connected.", Modifier.padding(horizontal = 20.dp)) { Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary) }
            state.error != null && state.tasks.isEmpty() -> FriendlyState(FriendlyStyle.Offline, "Couldn't load tasks", state.error.orEmpty(), Modifier.padding(horizontal = 20.dp), "Retry", { scope.launch { state.refresh() } }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusWarning) }
            visible.isEmpty() -> FriendlyState(FriendlyStyle.Empty, if (agentSegment) "No agent tasks yet" else "No tasks yet", "Create a task to get started.", Modifier.padding(horizontal = 20.dp), "Create task", { creating = true }) { Icon(painterResource(DesignR.drawable.ph_check_circle_fill), null, tint = c.textSecondary) }
            else -> PullToRefreshBox(isRefreshing = state.loading, onRefresh = { scope.launch { state.refresh() } }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    visible.forEach { t ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 64.dp).clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp))
                                .clickable(role = Role.Button) { detailId = t.id }.testTag("coworkTask${t.id}").padding(horizontal = 14.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(t.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                                    StatusBadge(t.status)
                                    if (t.priority != 0) Text("P${t.priority}", color = c.textSecondary, fontSize = 12.sp)
                                }
                            }
                            Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                        }
                    }
                }
            }
        }
    }
    if (creating) CreateCoworkTaskSheet(agentSegment, onDismiss = { creating = false }) { title, desc, assignee ->
        creating = false
        scope.launch { actionError = state.create(title, desc, assignee)?.let { "Couldn't create the task: $it" } }
    }
}

@Composable
private fun StatusBadge(status: TaskStatus) {
    val c = AllternitTheme.colors
    val tint = statusColor(status, c.textSecondary)
    Text(status.label, color = tint, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clip(CircleShape).background(tint.copy(alpha = 0.15f)).padding(horizontal = 8.dp, vertical = 3.dp))
}

/** iOS CoworkTaskDetailView: description, a status row that opens the five statuses, the assignee, delete with confirm. */
@Composable
private fun CoworkTaskDetail(state: CoworkTasksState, task: CoworkTask, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var statusOpen by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("coworkTaskDetail")) {
        LibraryHeader(title = task.title, onBack = onBack, backTag = "coworkTaskBack", trailing = {
            LibraryIconButton(DesignR.drawable.ph_trash, "Delete task", "coworkTaskDelete", tint = AllternitStatic.StatusWarning) { confirmDelete = true }
        })
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
            error?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp) }
            if (!task.description.isNullOrEmpty()) Text(task.description, color = c.textSecondary, fontSize = 14.sp)
            DetailRow("Status", if (busy) "Updating…" else task.status.label, "coworkStatusRow") { if (!busy) statusOpen = true }
            task.assigneeType?.let { type ->
                Row(Modifier.fillMaxWidth().height48().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Icon(painterResource(if (type == "agent") DesignR.drawable.ph_cpu else DesignR.drawable.ph_identification_card), null, tint = AllternitStatic.AccentCowork, modifier = Modifier.size(18.dp))
                    Text(if (type == "agent") "Agent" else "Human", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                    task.assigneeName?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
                }
            }
        }
    }
    if (statusOpen) LibrarySheet("Status", "coworkStatusSheet", onDismiss = { statusOpen = false }) {
        Column(Modifier.padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            TaskStatus.entries.forEach { s ->
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.Button) {
                    statusOpen = false
                    if (s != task.status) { busy = true; scope.launch { error = state.updateStatus(task.id, s)?.let { "Couldn't update status: $it" }; busy = false } }
                }.testTag("coworkStatus${s.raw}"), verticalAlignment = Alignment.CenterVertically) {
                    Text(s.label, color = c.textPrimary, fontSize = 16.sp, modifier = Modifier.weight(1f))
                    if (s == task.status) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.accentPrimary, modifier = Modifier.size(18.dp))
                }
            }
            Spacer(Modifier.height16())
        }
    }
    if (confirmDelete) AlertDialog(
        onDismissRequest = { confirmDelete = false },
        title = { Text("Delete this task?") },
        text = { Text("This can't be undone.") },
        confirmButton = { TextButton(onClick = { confirmDelete = false; scope.launch { val e = state.delete(task.id); if (e == null) onBack() else error = "Couldn't delete the task: $e" } }, modifier = Modifier.testTag("coworkConfirmDelete")) { Text("Delete", color = AllternitStatic.StatusError) } },
        dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
    )
}

private fun Modifier.height48(): Modifier = this.heightIn(min = 48.dp)
private fun Modifier.height16(): Modifier = this.heightIn(min = 16.dp)

@Composable
private fun DetailRow(title: String, value: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp))
            .clickable(role = Role.Button, onClick = onClick).testTag(tag).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_sliders_horizontal), null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(18.dp))
        Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
        Text(value, color = c.textSecondary, fontSize = 13.sp, maxLines = 1)
        Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
    }
}

/** iOS CreateCoworkTaskSheet: title, description, Human / Agent, "Create task". */
@Composable
private fun CreateCoworkTaskSheet(defaultAgent: Boolean, onDismiss: () -> Unit, onCreate: (String, String?, String) -> Unit) {
    val c = AllternitTheme.colors
    var title by remember { mutableStateOf("") }
    var desc by remember { mutableStateOf("") }
    var agent by remember { mutableStateOf(defaultAgent) }
    val can = title.trim().isNotEmpty()
    LibrarySheet("New task", "coworkCreateSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", testTag = "coworkCreateClose", onClick = onDismiss)) {
        Column(Modifier.padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            LibraryField("Title", title, "Task title", "coworkTitleField") { title = it }
            LibraryField("Description", desc, "What needs to happen? (optional)", "coworkDescField", singleLine = false, minHeight = 88) { desc = it }
            Text("Assignee", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
            PillRow(listOf(false, true), agent, { if (it) "Agent" else "Human" }, { if (it) "coworkAssigneeAgent" else "coworkAssigneeHuman" }) { agent = it }
            Box(
                Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(if (can) c.accentPrimary else c.bgSecondary)
                    .clickable(enabled = can, role = Role.Button) { onCreate(title.trim(), desc.trim().ifEmpty { null }, if (agent) "agent" else "human") }.testTag("coworkCreateTask"),
                contentAlignment = Alignment.Center,
            ) { Text("Create task", color = if (can) c.onAccent else c.textSecondary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
            Spacer(Modifier.width(1.dp))
        }
    }
}

/** iOS FabricApprovalsView: pending approvals with Grant / Deny, then recent runs that open their events. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun FabricApprovalsScreen(api: CoworkApi, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var pending by remember { mutableStateOf<List<Approval>>(emptyList()) }
    var runs by remember { mutableStateOf<List<FabricRun>>(emptyList()) }
    var events by remember { mutableStateOf<Map<String, List<RunEvent>>>(emptyMap()) }
    var selected by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var noComputer by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(true) }
    suspend fun refresh() {
        loading = true; error = null
        when (val a = api.approvals()) {
            is CRes.Ok -> pending = a.value.filter { it.status == "pending" }
            CRes.NoComputer -> noComputer = true
            is CRes.Failed -> error = a.message
        }
        (api.runs() as? CRes.Ok)?.let { runs = it.value }
        loading = false
    }
    LaunchedEffect(Unit) { refresh() }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("fabricApprovals")) {
        LibraryHeader(title = "Fabric Transport", onBack = onBack, backTag = "fabricBack")
        PullToRefreshBox(isRefreshing = loading, onRefresh = { scope.launch { refresh() } }, modifier = Modifier.fillMaxSize()) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Pending approvals", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                if (pending.isEmpty()) Text(if (noComputer) "Connect a computer to see approvals." else error ?: "No approvals waiting.", color = c.textSecondary, fontSize = 13.sp)
                pending.forEach { a ->
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row { Text(a.capability, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f)); Text(a.status, color = AllternitStatic.StatusWarning, fontSize = 12.sp) }
                        Text(a.target, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            for (grant in listOf(true, false)) {
                                Text(
                                    if (grant) "Grant" else "Deny", color = if (grant) Color.White else AllternitStatic.StatusError, fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
                                    modifier = Modifier.clip(CircleShape).background(if (grant) AllternitStatic.StatusSuccess.copy(alpha = 0.9f) else Color.Transparent)
                                        .border(BorderStroke(1.dp, if (grant) Color.Transparent else AllternitStatic.StatusError), CircleShape)
                                        .clickable(role = Role.Button) { scope.launch { if (api.decide(a.id, grant) is CRes.Ok) refresh() else error = "Couldn't answer the approval. Try again." } }
                                        .testTag(if (grant) "fabricGrant${a.id}" else "fabricDeny${a.id}").padding(horizontal = 18.dp, vertical = 8.dp),
                                )
                            }
                        }
                    }
                }
                Text("Recent runs", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                if (runs.isEmpty()) Text("No runs yet.", color = c.textSecondary, fontSize = 13.sp)
                runs.forEach { run ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(role = Role.Button) {
                        selected = if (selected == run.id) null else run.id
                        if (selected != null) scope.launch { events = events + (run.id to api.events(run.id)) }
                    }.testTag("fabricRun${run.id}"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Box(Modifier.size(8.dp).clip(CircleShape).background(when (run.state) { "completed" -> AllternitStatic.StatusSuccess; "failed" -> AllternitStatic.StatusError; else -> AllternitStatic.StatusWarning }))
                        Text(run.id.take(8), color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Text(run.state, color = c.textSecondary, fontSize = 13.sp)
                    }
                    if (selected == run.id) (events[run.id] ?: emptyList()).forEach { e ->
                        Column(Modifier.padding(start = 12.dp)) {
                            Text(e.eventType, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                            e.executor?.let { Text("executor $it", color = c.textSecondary, fontSize = 11.sp) }
                        }
                    }
                }
            }
        }
    }
}
