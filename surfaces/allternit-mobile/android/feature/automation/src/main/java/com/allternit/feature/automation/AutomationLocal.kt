package com.allternit.feature.automation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.relativeTime
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/*
 * The computer's own routines and loops (iOS RoutinesListView, RoutineDetailView, LoopsListView, LoopDetailView,
 * CreateRoutineSheet, CreateLoopSheet): `v1/automations/…` on the user's gizzi-code, through the relay.
 */

private enum class LocalTab(val label: String) { Routines("Routines"), Loops("Loops") }

/** Mutable view of what the list screens load. */
private class LocalState(val api: AutomationApi) {
    var routines by mutableStateOf<AResult<List<LocalRoutine>>?>(null)
    var loops by mutableStateOf<AResult<List<LocalLoop>>?>(null)
    var actionError by mutableStateOf<String?>(null)
    suspend fun load() { routines = api.localRoutines(); loops = api.localLoops() }
    suspend fun act(r: AResult<*>): Boolean {
        actionError = when (r) { is AResult.Failed -> r.message; AResult.NoComputer -> "Connect a computer to do that."; else -> null }
        if (r is AResult.Ok) load()
        return r is AResult.Ok
    }
}

@Composable
internal fun AutomationLocalScreen(host: AutomationHost, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val st = remember(host.backend) { LocalState(AutomationApi(host.backend)) }
    val scope = rememberCoroutineScope()
    var tab by remember { mutableStateOf(LocalTab.Routines) }
    var query by remember { mutableStateOf("") }
    var routine by remember { mutableStateOf<LocalRoutine?>(null) }
    var loop by remember { mutableStateOf<LocalLoop?>(null) }
    var newRoutine by remember { mutableStateOf(false) }
    var newLoop by remember { mutableStateOf(false) }
    var tick by remember { mutableIntStateOf(0) }
    LaunchedEffect(tick) { st.load() }

    AutoScaffold("On this computer", onBack, "automationLocalBack", trailing = {
        LibraryIconButton(DesignR.drawable.ph_plus, if (tab == LocalTab.Routines) "New routine" else "New loop", "automationLocalNew") { if (tab == LocalTab.Routines) newRoutine = true else newLoop = true }
    }) {
        PillRow(LocalTab.entries, tab, { it.label }, { "automationLocal${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { tab = it }
        LibrarySearchField(query, "Search", "automationLocalSearch", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { query = it }
        st.actionError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp)) }
        val state: AResult<*>? = if (tab == LocalTab.Routines) st.routines else st.loops
        when (state) {
            null -> Loading()
            AResult.NoComputer -> NoComputerState { tick++ }
            is AResult.Failed -> ErrorState("Couldn't load ${tab.label.lowercase()}", state.message, { tick++ })
            is AResult.Ok -> if (tab == LocalTab.Routines) {
                val rows = st.routines?.orNull().orEmpty().filter { query.isBlank() || it.name.contains(query, true) }
                if (rows.isEmpty()) EmptyState(if (query.isBlank()) "No routines" else "No matches", if (query.isBlank()) "Chain a few commands into a named routine you can run on demand, anytime." else "No routines match.", Modifier.padding(horizontal = 20.dp), if (query.isBlank()) "New routine" else null) { newRoutine = true }
                else LazyColumn(Modifier.fillMaxSize().testTag("automationLocalRoutines"), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    items(rows, key = { it.id }) { r ->
                        LocalRow(r.name, listOfNotNull(r.state, "${r.steps.size} steps", r.schedule).joinToString(" · "), r.state, "automationRoutine_${r.id}") { routine = r }
                    }
                }
            } else {
                val rows = st.loops?.orNull().orEmpty().filter { query.isBlank() || it.command.contains(query, true) }
                if (rows.isEmpty()) EmptyState(if (query.isBlank()) "No loops" else "No matches", if (query.isBlank()) "Run a command on repeat until it exits clean or hits its iteration cap." else "No loops match.", Modifier.padding(horizontal = 20.dp), if (query.isBlank()) "New loop" else null) { newLoop = true }
                else LazyColumn(Modifier.fillMaxSize().testTag("automationLocalLoops"), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    items(rows, key = { it.id }) { l ->
                        LocalRow(l.command, "${l.state} · ${l.iterations.size}/${l.maxIterations} iterations", l.state, "automationLoop_${l.id}") { loop = l }
                    }
                }
            }
        }
    }

    routine?.let { r ->
        val cur = st.routines?.orNull()?.firstOrNull { it.id == r.id } ?: r
        LocalDetail(
            title = cur.name, tag = "automationRoutineDetail", state = cur.state, onClose = { routine = null },
            info = listOfNotNull("State" to cur.state, cur.trigger?.takeIf { it.isNotBlank() }?.let { "Trigger" to it }, cur.schedule?.takeIf { it.isNotBlank() }?.let { "Schedule" to it }, relativeTime(cur.updated.toLong()).let { "Updated" to it }),
            onRun = { scope.launch { st.act(st.api.runLocalRoutine(cur.id)) } },
            onDelete = { scope.launch { if (st.act(st.api.deleteLocalRoutine(cur.id))) routine = null } },
            deleteWord = "routine", runLabel = "Run Now",
        ) {
            SectionLabel("Steps")
            if (cur.steps.isEmpty()) Text("No steps.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
            cur.steps.forEachIndexed { i, (cmd, status) ->
                Row(Modifier.padding(horizontal = 20.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("${i + 1}", color = c.textTertiary, fontSize = 13.sp)
                    Text(cmd, color = c.textPrimary, fontSize = 13.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
                    Text(status, color = stateColor(status), fontSize = 12.sp)
                }
            }
        }
    }
    loop?.let { l0 ->
        val cur = st.loops?.orNull()?.firstOrNull { it.id == l0.id } ?: l0
        LocalDetail(
            title = cur.command, tag = "automationLoopDetail", state = cur.state, onClose = { loop = null },
            info = listOfNotNull("State" to cur.state, "Iterations" to "${cur.iterations.size} of ${cur.maxIterations}", cur.exitCondition?.takeIf { it.isNotBlank() }?.let { "Exit when" to it }, relativeTime(cur.updated.toLong()).let { "Updated" to it }),
            onRun = { scope.launch { st.act(st.api.runLocalLoop(cur.id)) } },
            onDelete = { scope.launch { if (st.act(st.api.deleteLocalLoop(cur.id))) loop = null } },
            deleteWord = "loop", runLabel = if (cur.state == "running") "Running" else "Restart",
        ) {
            if (cur.state == "exhausted" || cur.state == "maxed") {
                Text("Ran out of iterations", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
                Text("Exit condition never fired within ${cur.maxIterations} tries. Restart to try again.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
            }
            cur.metric?.let { m ->
                SectionLabel("Loop Card")
                Text("${m.direction.replaceFirstChar { it.uppercase() }} is better, keep at ${(m.keepBar * 100).toInt()}%, ${m.minPairs} pairs", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
                cur.ledger.forEach { Text("#${it.iteration} ${it.verdict}", color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 2.dp)) }
            }
            SectionLabel("Iterations")
            if (cur.iterations.isEmpty()) Text("Loop iterations will appear here as they run.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
            cur.iterations.takeLast(20).reversed().forEach { it2 ->
                Column(Modifier.padding(horizontal = 20.dp, vertical = 6.dp)) {
                    Text("Iteration ${it2.iteration} · exit ${it2.exitCode}", color = if (it2.exitCode == 0) stateColor("done") else stateColor("failed"), fontSize = 13.sp)
                    if (it2.output.isNotBlank()) Text(it2.output.take(240), color = c.textSecondary, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                }
            }
        }
    }
    if (newRoutine) CreateRoutineSheet(st.api, onClose = { newRoutine = false }, onCreated = { tick++ })
    if (newLoop) CreateLoopSheet(st.api, onClose = { newLoop = false }, onCreated = { tick++ })
}

@Composable
private fun LocalRow(title: String, detail: String, state: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).testTag(tag).clickable(role = Role.Button, onClick = onClick).padding(14.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Text(detail, color = stateColor(state), fontSize = 12.sp)
    }
}

@Composable
private fun LocalDetail(
    title: String, tag: String, state: String, onClose: () -> Unit, info: List<Pair<String, String>>, onRun: () -> Unit, onDelete: () -> Unit,
    deleteWord: String, runLabel: String, body: @Composable () -> Unit,
) {
    val c = AllternitTheme.colors
    var confirm by remember { mutableStateOf(false) }
    LibrarySheet(title = title, testTag = tag, onDismiss = onClose, leading = LibrarySheetAction("Close", testTag = "${tag}Close", onClick = onClose)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            info.forEach { (k, v) -> InfoRow(k, v.replaceFirstChar { it.uppercase() }, if (k == "State") stateColor(state) else null) }
            Row(Modifier.padding(horizontal = 20.dp, vertical = 14.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                ActionPill(runLabel, "${tag}Run", filled = true, enabled = state != "running", onClick = onRun)
                ActionPill("Delete", "${tag}Delete", destructive = true) { confirm = true }
            }
            body()
        }
    }
    if (confirm) LibrarySheet(title = "Delete this $deleteWord?", testTag = "${tag}Confirm", onDismiss = { confirm = false }, leading = LibrarySheetAction("Cancel") { confirm = false }) {
        Text("This can't be undone.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 8.dp))
        Row(Modifier.padding(20.dp)) { ActionPill("Delete", "${tag}ConfirmYes", filled = true, destructive = true) { confirm = false; onDelete() } }
    }
}

/** iOS CreateRoutineSheet: a name and the commands to run, in order; trigger and schedule are optional. */
@Composable
private fun CreateRoutineSheet(api: AutomationApi, onClose: () -> Unit, onCreated: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf("") }
    var steps by remember { mutableStateOf(listOf("")) }
    var trigger by remember { mutableStateOf("") }
    var schedule by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val cmds = steps.map { it.trim() }.filter { it.isNotEmpty() }
    val can = name.isNotBlank() && cmds.isNotEmpty() && !saving
    LibrarySheet(
        title = "New routine", testTag = "createRoutine", onDismiss = onClose,
        leading = LibrarySheetAction("Cancel", testTag = "createRoutineClose", onClick = onClose),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Create routine", enabled = can, bold = true, testTag = "createRoutineSubmit") {
            saving = true; error = null
            scope.launch {
                when (val r = api.createLocalRoutine(name.trim(), cmds, trigger.trim().ifEmpty { null }, schedule.trim().ifEmpty { null })) {
                    is AResult.Ok -> { onCreated(); onClose() }
                    is AResult.Failed -> { error = r.message; saving = false }
                    AResult.NoComputer -> { error = "Connect a computer to create a routine."; saving = false }
                }
            }
        },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            LibraryField("Name", name, "Deploy checks", "createRoutineName") { name = it }
            steps.forEachIndexed { i, s -> LibraryField("Step ${i + 1}", s, "Command", "createRoutineStep$i") { v -> steps = steps.toMutableList().also { it[i] = v } } }
            Text("Add step", color = c.accentCode, fontSize = 15.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("createRoutineAddStep").clickable(role = Role.Button) { steps = steps + "" }.padding(vertical = 10.dp))
            LibraryField("Trigger (optional)", trigger, "e.g. on push", "createRoutineTrigger") { trigger = it }
            LibraryField("Schedule (optional)", schedule, "0 9 * * 1-5", "createRoutineSchedule") { schedule = it }
            Text("A routine can also be run manually. Trigger and schedule are both optional.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp)) }
        }
    }
}

/** iOS CreateLoopSheet: a command to repeat, an optional exit condition, the iteration cap and an optional Loop Card. */
@Composable
private fun CreateLoopSheet(api: AutomationApi, onClose: () -> Unit, onCreated: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var command by remember { mutableStateOf("") }
    var exit by remember { mutableStateOf("") }
    var max by remember { mutableIntStateOf(10) }
    var instrument by remember { mutableStateOf("") }
    var lower by remember { mutableStateOf(true) }
    var keep by remember { mutableIntStateOf(5) }
    var pairs by remember { mutableIntStateOf(5) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val can = command.isNotBlank() && !saving
    LibrarySheet(
        title = "New loop", testTag = "createLoop", onDismiss = onClose,
        leading = LibrarySheetAction("Close", testTag = "createLoopClose", onClick = onClose),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Create loop", enabled = can, bold = true, testTag = "createLoopSubmit") {
            saving = true; error = null
            val metric = instrument.trim().takeIf { it.isNotEmpty() }?.let { LoopMetric(it, if (lower) "lower" else "higher", keep / 100.0, pairs) }
            scope.launch {
                when (val r = api.createLocalLoop(command.trim(), exit.trim().ifEmpty { null }, max, metric)) {
                    is AResult.Ok -> { onCreated(); onClose() }
                    is AResult.Failed -> { error = r.message; saving = false }
                    AResult.NoComputer -> { error = "Connect a computer to create a loop."; saving = false }
                }
            }
        },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            LibraryField("Command", command, "Command to run each iteration", "createLoopCommand") { command = it }
            LibraryField("Exit condition (optional)", exit, "e.g. exit_code_zero, or text to match in output", "createLoopExit") { exit = it }
            Text("Left blank, the loop stops the first time the command exits 0.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
            Stepper("Max iterations", max, 1..100, "createLoopMax") { max = it }
            SectionLabel("Loop Card (optional)")
            Text("Name the one number to move. Each attempt is measured, kept or reverted, and written to the ledger.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
            LibraryField("Instrument", instrument, "Instrument command that prints the number", "createLoopInstrument") { instrument = it }
            PillRow(listOf(true, false), lower, { if (it) "Lower is better" else "Higher is better" }, { "createLoopDirection_$it" }, Modifier.padding(horizontal = 16.dp)) { lower = it }
            Stepper("Keep bar %", keep, 0..100, "createLoopKeep") { keep = it }
            Stepper("Paired runs", pairs, 2..50, "createLoopPairs") { pairs = it }
            Text("A loop starts running the moment it's created. There's no separate Run step.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp)) }
        }
    }
}

@Composable
private fun Stepper(label: String, value: Int, range: IntRange, tag: String, onChange: (Int) -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        Text("$label: $value", color = c.textPrimary, fontSize = 15.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ActionPill("−", "${tag}Minus", enabled = value > range.first) { onChange(value - 1) }
            ActionPill("+", "${tag}Plus", enabled = value < range.last) { onChange(value + 1) }
        }
    }
}
