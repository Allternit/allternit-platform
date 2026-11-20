package com.allternit.feature.automation

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.bots.RoutineRule
import com.allternit.feature.bots.RoutineScheduleEditor
import kotlinx.coroutines.launch

/*
 * The create flows (iOS AutomationWizardSheet, ScheduledTaskSheet, CreateAutomationTaskSheet and the New goal
 * sheet). One sheet: pick what to make (skipped when the kind is known), then the fields. The time is the
 * wave E routine time editor. Saving calls the real API; nothing is created until the person taps Create.
 */

private val NO_BOT = BotLite("", "No bot")
private val PRIORITIES = listOf("low", "medium", "high")

@Composable
internal fun AutomationWizardSheet(store: AutomationStore, kind: TaskType?, slotCron: String?, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var picked by remember { mutableStateOf(kind) }
    var name by remember { mutableStateOf("") }
    var instruction by remember { mutableStateOf("") }
    var botId by remember { mutableStateOf<String?>(null) }
    var goalId by remember { mutableStateOf("") }
    var priority by remember { mutableStateOf("medium") }
    var target by remember { mutableStateOf("") }
    var rule by remember { mutableStateOf(slotCron?.let { RoutineRule.fromCron(it) } ?: RoutineRule(RoutineRule.Repeat.Daily, 9 * 60, days = RoutineRule.ALL_DAYS)) }
    var editTime by remember { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    val k = picked
    val needsBot = k == TaskType.Routine
    // A routine needs a bot: the first one is the default, like the iOS picker.
    val bot: String? = botId ?: if (needsBot) store.bots.firstOrNull()?.id else null
    val canCreate = k != null && name.isNotBlank() && !saving &&
        (k == TaskType.Goal || (instruction.isNotBlank() && rule.isValid && (!needsBot || !bot.isNullOrEmpty())))

    fun create() {
        val kk = k ?: return
        saving = true; error = null
        scope.launch {
            val n = name.trim(); val p = instruction.trim()
            val gid = goalId.ifEmpty { null }
            val result = when (kk) {
                TaskType.Routine -> store.api.createRoutine(n, p, bot.orEmpty(), rule.cron, gid)
                TaskType.Loop -> store.api.createLoop(n, p, bot?.ifEmpty { null }, rule.cron, gid)
                TaskType.Heartbeat -> store.api.createHeartbeat(n, p, rule.cron, bot?.ifEmpty { null })
                TaskType.Goal -> store.api.createGoal(n, instruction.trim().ifEmpty { null }, priority, bot?.ifEmpty { null }, target.trim().ifEmpty { null })
            }
            val err = store.create(result)
            saving = false
            if (err == null) onClose() else error = err
        }
    }

    LibrarySheet(
        title = if (k == null) "New automation task" else "New ${k.label.lowercase()}", testTag = "automationWizard", onDismiss = onClose,
        leading = LibrarySheetAction("Cancel", testTag = "automationWizardCancel", onClick = onClose),
        trailing = if (k == null) null else LibrarySheetAction(if (saving) "Saving…" else "Create", enabled = canCreate, bold = true, testTag = "automationWizardCreate") { create() },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (k == null) {
                Text("What do you want to set up?", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp))
                listOf(
                    TaskType.Routine to "A bot does a task on a schedule.",
                    TaskType.Loop to "A bot works on something again and again.",
                    TaskType.Goal to "An outcome to track, with routines that serve it.",
                    TaskType.Heartbeat to "A check your computer runs on a schedule.",
                ).forEach { (t, line) ->
                    Column(Modifier.fillMaxWidth().heightIn(min = 56.dp).testTag("automationKind${t.name}").clickable(role = Role.Button) { picked = t }.padding(horizontal = 20.dp, vertical = 8.dp)) {
                        Text(t.label, color = c.textPrimary, fontSize = 16.sp)
                        Text(line, color = c.textSecondary, fontSize = 12.sp)
                    }
                }
            } else {
                LibraryField(if (k == TaskType.Goal) "Goal" else "Name", name, if (k == TaskType.Goal) "What do you want to get done?" else "Morning summary", "automationName") { name = it }
                LibraryField(if (k == TaskType.Goal) "Details (optional)" else "What should it do?", instruction, if (k == TaskType.Goal) "Anything a bot should know" else "Tell the bot what to do each time", "automationInstruction", singleLine = false, minHeight = 96) { instruction = it }
                if (k == TaskType.Goal) {
                    PillRow(PRIORITIES, priority, { it.replaceFirstChar { c -> c.uppercase() } }, { "automationPriority_$it" }, Modifier.padding(horizontal = 16.dp)) { priority = it }
                    LibraryField("Target date (optional)", target, "2027-01-31", "automationTarget") { target = it }
                } else {
                    if (store.bots.isEmpty() && needsBot) Text("Create a bot first. A routine runs as a turn in a bot's chat.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("automationNoBots"))
                    val choices = if (needsBot) store.bots else listOf(NO_BOT) + store.bots
                    if (choices.isNotEmpty()) {
                        Text("Run by", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
                        PillRow(choices, choices.firstOrNull { it.id == (bot ?: "") } ?: choices.first(), { it.name }, { "automationBot_${it.id}" }, Modifier.padding(horizontal = 16.dp)) { botId = it.id }
                    }
                    val goals = store.of(TaskType.Goal)
                    if (goals.isNotEmpty() && k != TaskType.Heartbeat) {
                        Text("Goal it serves", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp))
                        val gc = listOf(BotLite("", "None")) + goals.map { BotLite(it.id, it.name) }
                        PillRow(gc, gc.firstOrNull { it.id == goalId } ?: gc.first(), { it.name }, { "automationGoal_${it.id}" }, Modifier.padding(horizontal = 16.dp)) { goalId = it.id }
                    }
                    Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).testTag("automationWhen").clickable(role = Role.Button) { editTime = true }.padding(horizontal = 20.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Column {
                            Text("When", color = c.textSecondary, fontSize = 12.sp)
                            Text(rule.label, color = c.textPrimary, fontSize = 16.sp, modifier = Modifier.testTag("automationWhenLabel"))
                        }
                        Text("Change", color = c.accentCode, fontSize = 15.sp)
                    }
                }
                error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("automationWizardError")) }
            }
        }
    }
    if (editTime) RoutineScheduleEditor("When", rule, saving = false, error = null, onCancel = { editTime = false }, onSave = { rule = it; editTime = false })
}

/** iOS ScheduledTaskSheet: the "new scheduled task" sheet, a routine unless told otherwise. */
@Composable
internal fun ScheduledTaskSheet(store: AutomationStore, kind: TaskType = TaskType.Routine, slotCron: String? = null, onClose: () -> Unit) =
    AutomationWizardSheet(store, kind, slotCron, onClose)

/** iOS CreateAutomationTaskSheet: a new agent task on the computer (a heartbeat), with the same time editor. */
@Composable
internal fun CreateAutomationTaskSheet(store: AutomationStore, onClose: () -> Unit) =
    AutomationWizardSheet(store, TaskType.Heartbeat, null, onClose)
