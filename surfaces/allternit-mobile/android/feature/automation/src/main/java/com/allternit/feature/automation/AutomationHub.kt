package com.allternit.feature.automation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.relativeTime
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.bots.RoutineRule
import com.allternit.feature.bots.RoutineScheduleEditor
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** What the shell gives Automation (iOS AutomationTasksPhoneView's bindings): the relay, the menu button, sign-in. */
class AutomationHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val menuButton: @Composable () -> Unit,
    val openSignIn: () -> Unit,
)

private enum class Top(val label: String) { Scheduled("Scheduled"), Board("Board") }
private enum class TaskTab(val label: String, val type: TaskType?) {
    All("All", null), Goals("Goals", TaskType.Goal), Routines("Routines", TaskType.Routine), Loops("Loops", TaskType.Loop), Heartbeats("Agent Heartbeats", TaskType.Heartbeat)
}
internal enum class RoutinesView(val label: String) { List("List"), Graph("Graph"), Calendar("Calendar") }
private enum class TaskSort(val label: String) { Date("Date"), Name("Name"), Type("Type") }

/** The drawer's Automation Tasks row (iOS AutomationTasksPhoneView; the Scheduled / Board pair is iOS's Tasks bar). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AutomationRoot(host: AutomationHost) {
    val c = AllternitTheme.colors
    val store = remember(host.backend) { AutomationStore(AutomationApi(host.backend)) }
    val scope = rememberCoroutineScope()
    var top by remember { mutableStateOf(Top.Scheduled) }
    var tab by remember { mutableStateOf(TaskTab.All) }
    var view by remember { mutableStateOf(RoutinesView.List) }
    var sort by remember { mutableStateOf(TaskSort.Date) }
    var query by remember { mutableStateOf("") }
    var selected by remember { mutableStateOf<AutoTask?>(null) }
    var creating by remember { mutableStateOf(false) }
    var slotCron by remember { mutableStateOf<String?>(null) }
    var local by remember { mutableStateOf(false) }
    var hbAgent by remember { mutableStateOf<String?>(null) }
    var refreshing by remember { mutableStateOf(false) }

    LaunchedEffect(store, host.signedIn) { if (host.signedIn) store.load() }

    if (local) {
        AutomationLocalScreen(host, onBack = { local = false })
        return
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("automationHub")) {
        LibraryHeader(
            title = "Automation Tasks", onBack = null, leading = host.menuButton, glass = true,
            trailing = { LibraryIconButton(DesignR.drawable.ph_plus, "New task", "automationNew") { slotCron = null; creating = true } },
        )
        PillRow(Top.entries, top, { it.label }, { "automationTop${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { top = it }
        when {
            !host.signedIn -> EmptyState("Sign in to see your automations", "Routines, goals and loops run on your bots, so they belong to your account.", Modifier.padding(horizontal = 20.dp), "Sign in", host.openSignIn)
            top == Top.Board -> TasksBoardScreen(host, onScheduled = { top = Top.Scheduled })
            else -> {
                PillRow(TaskTab.entries, tab, { t -> t.label + (t.type?.let { ty -> " " + store.of(ty).size } ?: if (store.tasks.isNotEmpty()) " " + store.tasks.size else "") }, { "automationTab${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { tab = it }
                if (tab != TaskTab.Goals) PillRow(RoutinesView.entries, view, { it.label }, { "automationView${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { view = it }
                if (view == RoutinesView.List || tab == TaskTab.Goals) {
                    LibrarySearchField(query, "Search tasks", "automationSearch", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { query = it }
                    PillRow(TaskSort.entries, sort, { "Sort: " + it.label }, { "automationSort${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { sort = it }
                }
                val shown = run {
                    val base = store.tasks.filter { tab.type == null || it.type == tab.type }.filter { query.isBlank() || it.name.contains(query, true) || it.instruction.contains(query, true) }
                    when (sort) {
                        TaskSort.Name -> base.sortedBy { it.name.lowercase() }
                        TaskSort.Type -> base.sortedBy { it.type.ordinal }
                        TaskSort.Date -> base.sortedByDescending { parseTime(it.createdAt) }
                    }
                }
                store.actionError?.let { Text(it, color = com.allternit.core.design.AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp).testTag("automationActionError")) }
                PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; store.load(); refreshing = false } }, modifier = Modifier.weight(1f).fillMaxWidth()) {
                    when {
                        store.loading && !store.loaded -> Loading()
                        store.noComputer -> NoComputerState { scope.launch { store.load() } }
                        store.error != null && store.tasks.isEmpty() -> ErrorState("Couldn't load your automations", store.error.orEmpty(), { scope.launch { store.load() } })
                        tab == TaskTab.Heartbeats && view == RoutinesView.List -> HeartbeatsTab(store, hbAgent) { hbAgent = it }
                        view == RoutinesView.Graph && tab != TaskTab.Goals -> AutomationGraph(store, shown.filter { it.type != TaskType.Goal }, tab == TaskTab.All, onOpen = { selected = it })
                        view == RoutinesView.Calendar && tab != TaskTab.Goals -> AutomationCalendar(store, shown.filter { it.type != TaskType.Goal }, onOpen = { selected = it }, onNewAt = { cron -> slotCron = cron; creating = true })
                        shown.isEmpty() -> EmptyState(
                            if (query.isBlank()) "No ${(tab.type?.plural ?: "tasks").lowercase()} yet" else "No matches",
                            if (query.isBlank()) "Schedule a task and a bot runs it for you on time." else "Nothing matches that search.",
                            Modifier.padding(horizontal = 20.dp), if (query.isBlank()) "New task" else null, { slotCron = null; creating = true },
                        )
                        else -> LazyColumn(Modifier.fillMaxSize().testTag("automationList"), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            items(shown, key = { it.key }) { t -> TaskCard(store, t, onOpen = { selected = t }) }
                            item("local") { LocalLink { local = true } }
                            if (store.heartbeatsAway && (tab == TaskTab.Heartbeats || tab == TaskTab.All)) item("away") {
                                Text("Agent heartbeats live on your computer. Connect it to see them here.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(8.dp))
                            }
                        }
                    }
                }
            }
        }
    }

    selected?.let { t ->
        val current = store.tasks.firstOrNull { it.key == t.key } ?: t
        AutomationTaskDetailSheet(store, current, onClose = { selected = null })
    }
    if (creating) AutomationWizardSheet(store, kind = null, slotCron = slotCron, onClose = { creating = false })
}

@Composable
private fun LocalLink(onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(12.dp)).testTag("automationLocalLink").clickable(role = Role.Button, onClick = onClick).padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Column {
            Text("On this computer", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            Text("Routines and loops the computer runs itself", color = c.textSecondary, fontSize = 12.sp)
        }
        Text("›", color = c.textTertiary, fontSize = 22.sp)
    }
}

/** A run row (iOS RoutineRunRowView + AutomationTaskCard): type, schedule in words, next run or the live step, the strip, Run now / Stop and the switch. */
@Composable
internal fun TaskCard(store: AutomationStore, t: AutoTask, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val live = store.live[t.id]
    val running = t.key in store.runningKeys
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).testTag("automationTask_${t.id}")
            .clickable(role = Role.Button, onClick = onOpen).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                    TypeTag(t.type)
                    t.badge?.let { Text(it, color = c.textSecondary, fontSize = 11.sp) }
                    if (t.madeByAgent) Text("Made by a bot", color = c.textTertiary, fontSize = 11.sp)
                }
                Text(displayName(t.name), color = if (t.isActive) c.textPrimary else c.textTertiary, fontSize = 16.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (t.canRun) AutoSwitch(t.isActive, "${t.name} on or off", "automationToggle_${t.id}") { scope.launch { store.toggle(t) } }
        }
        if (t.type == TaskType.Goal) {
            LinearProgressIndicator(progress = { ((t.progress ?: 0.0) / 100.0).toFloat().coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth(), color = c.accentCode, trackColor = c.borderSubtle)
            Text(listOfNotNull("${(t.progress ?: 0.0).toInt()}%", t.priority, t.targetDate?.let { "by $it" }, store.botName(t.agentId)).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
        } else {
            Text(CronWords.of(t), color = c.textSecondary, fontSize = 13.sp)
            val line = when {
                live?.running == true -> "Running · " + (live.steps.lastOrNull() ?: "starting")
                else -> listOfNotNull(CronWords.nextLabel(t.nextRunAt)?.let { "Next $it" }, store.botName(t.agentId)?.let { "by $it" }).joinToString(" · ")
            }
            if (line.isNotEmpty()) Text(line, color = if (live?.running == true) stateColor("running") else c.textTertiary, fontSize = 12.sp)
            if (t.runs.isNotEmpty()) RunStrip(t.runs)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ActionPill(if (running) "Running…" else "Run now", "automationRun_${t.id}", enabled = !running && t.isActive) { scope.launch { store.runNow(t) } }
                if (live?.running == true && live.canStop) ActionPill("Stop", "automationStop_${t.id}", destructive = true) { scope.launch { store.stop(t) } }
            }
        }
    }
}

/** iOS AutomationScheduleLogic.displayName: a routine named from a slug reads as words. */
internal fun displayName(name: String): String = name.trim().let { n -> if (n.isNotEmpty() && !n.contains(' ') && (n.contains('-') || n.contains('_'))) n.replace('-', ' ').replace('_', ' ').replaceFirstChar { it.uppercase() } else n }

/** iOS AutomationTaskDetailSheet: what it does, when, who runs it, the run history, and the actions. */
@Composable
internal fun AutomationTaskDetailSheet(store: AutomationStore, t: AutoTask, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var confirmDelete by remember { mutableStateOf(false) }
    var editTime by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    var runs by remember { mutableStateOf<List<RunRecord>>(emptyList()) }
    LaunchedEffect(t.key) { if (t.type == TaskType.Loop) runs = store.api.loopRuns(t.id).orNull().orEmpty() }
    LibrarySheet(title = displayName(t.name), testTag = "automationDetail", onDismiss = onClose, leading = LibrarySheetAction("Close", testTag = "automationDetailClose", onClick = onClose)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            InfoRow("Type", t.type.label)
            InfoRow("State", t.status.replaceFirstChar { it.uppercase() }, stateColor(t.status))
            if (t.type != TaskType.Goal) {
                InfoRow("Schedule", CronWords.of(t))
                CronWords.nextLabel(t.nextRunAt)?.let { InfoRow("Next run", it) }
                if (parseTime(t.lastRunAt) > 0) InfoRow("Last run", relativeTime(parseTime(t.lastRunAt)))
            } else {
                InfoRow("Progress", "${(t.progress ?: 0.0).toInt()}%")
                t.priority?.let { InfoRow("Priority", it.replaceFirstChar { c -> c.uppercase() }) }
                t.targetDate?.let { InfoRow("Target date", it) }
            }
            store.botName(t.agentId)?.let { InfoRow("Run by", it) }
            store.goalName(t.goalId)?.let { InfoRow("Goal", it) }
            val body = t.instruction.ifBlank { t.description }
            if (body.isNotBlank()) { SectionLabel("Instructions"); Text(body, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("automationDetailInstructions")) }
            if (t.runs.isNotEmpty()) {
                SectionLabel("Run history")
                Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    RunStrip(t.runs)
                    t.runs.take(8).forEach { r ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(r.status.replaceFirstChar { it.uppercase() }, color = stateColor(r.status), fontSize = 13.sp, modifier = Modifier.weight(1f))
                            Text(listOfNotNull(parseTime(r.startedAt).takeIf { it > 0 }?.let { relativeTime(it) }, r.durationMs?.let { "${(it / 1000).toInt()}s" }).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                        }
                    }
                }
            }
            if (runs.isNotEmpty()) {
                SectionLabel("Runs")
                Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    runs.take(10).forEach { r ->
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(r.status.replaceFirstChar { it.uppercase() }, color = stateColor(r.status), fontSize = 13.sp, modifier = Modifier.weight(1f))
                            Text(r.error ?: r.output?.take(40).orEmpty(), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(2f))
                        }
                    }
                }
            }
            store.actionError?.let { Text(it, color = com.allternit.core.design.AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp)) }
            Row(Modifier.padding(horizontal = 20.dp, vertical = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (t.canRun) ActionPill("Run now", "automationDetailRun", filled = true, enabled = t.isActive && t.key !in store.runningKeys) { scope.launch { store.runNow(t) } }
                if (t.canRun) ActionPill(if (t.isActive) "Pause" else "Resume", "automationDetailToggle") { scope.launch { store.toggle(t) } }
                if (t.canRun && t.isCron) ActionPill("Change time", "automationDetailTime") { editTime = true }
                if (t.type == TaskType.Goal) ActionPill(if (t.status == "completed") "Reopen" else "Mark done", "automationDetailGoalDone") { scope.launch { store.setGoal(t, if (t.status == "completed") "active" else "completed", if (t.status == "completed") null else 100.0) } }
            }
            Row(Modifier.padding(horizontal = 20.dp)) { ActionPill("Delete", "automationDetailDelete", destructive = true) { confirmDelete = true } }
        }
    }
    if (editTime) {
        RoutineScheduleEditor(
            title = "Change time", initial = RoutineRule.fromCron(t.scheduleExpression) ?: RoutineRule(RoutineRule.Repeat.Daily, 9 * 60, days = RoutineRule.ALL_DAYS),
            saving = saving, error = null, onCancel = { editTime = false },
            onSave = { rule -> scope.launch { saving = true; if (store.setCron(t, rule.cron)) editTime = false; saving = false } },
        )
    }
    if (confirmDelete) {
        LibrarySheet(title = "Delete this ${t.type.label.lowercase()}?", testTag = "automationDeleteConfirm", onDismiss = { confirmDelete = false }, leading = LibrarySheetAction("Cancel", testTag = "automationDeleteCancel") { confirmDelete = false }) {
            Text("This can't be undone.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
            Row(Modifier.padding(20.dp)) { ActionPill("Delete", "automationDeleteYes", filled = true, destructive = true) { scope.launch { if (store.delete(t)) { confirmDelete = false; onClose() } else confirmDelete = false } } }
        }
    }
}
