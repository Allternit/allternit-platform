package com.allternit.feature.automation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
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
import com.allternit.feature.artifacts.PillRow
import androidx.compose.foundation.gestures.detectDragGesturesAfterLongPress
import androidx.compose.foundation.layout.offset
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.zIndex
import com.allternit.core.design.Haptics
import kotlinx.coroutines.launch
import kotlin.math.roundToInt
import com.allternit.feature.bots.RoutineRule
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.format.TextStyle
import java.util.Locale

/*
 * The scheduler (iOS AutomationScheduler / AutomationTasksCalendar / AutomationRoutinesCalendar): Day, Week and
 * Month over the next runs of every cron task, and the Graph (iOS AutomationTasksGraph / RoutinesGraph).
 * Moving a task: press and hold a run, then drag it up or down (15-minute steps, as iOS SchedulerChip);
 * the same move is on the row's accessibility actions, and "Change time" on the task still works.
 */

private enum class CalMode(val label: String) { Day("Day"), Week("Week"), Month("Month") }
private data class Occurrence(val at: LocalDateTime, val task: AutoTask)

@Composable
internal fun AutomationCalendar(store: AutomationStore, tasks: List<AutoTask>, onOpen: (AutoTask) -> Unit, onNewAt: (String) -> Unit) {
    val c = AllternitTheme.colors
    var mode by remember { mutableStateOf(CalMode.Week) }
    var picked by remember { mutableStateOf(LocalDate.now()) }
    val today = LocalDate.now()
    val occ = remember(tasks) { tasks.filter { it.isActive }.flatMap { t -> CronWords.upcoming(t, 62).map { Occurrence(it, t) } }.sortedBy { it.at } }
    val byDay = remember(occ) { occ.groupBy { it.at.toLocalDate() } }

    Column(Modifier.fillMaxSize().testTag("automationCalendar")) {
        PillRow(CalMode.entries, mode, { it.label }, { "automationCal${it.name}" }, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { mode = it }
        if (tasks.isEmpty()) {
            EmptyState("Nothing scheduled", "Schedule a task and its runs show up here.", Modifier.padding(horizontal = 20.dp), "New task", onAction = { onNewAt(CronWords.DEFAULT_CRON) }, art = com.allternit.core.design.EmptyArtKind.Bell)
            return
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            when (mode) {
                CalMode.Day -> DayList(store, today, byDay[today].orEmpty(), onOpen, onNewAt, "Nothing runs today.")
                CalMode.Week -> (0 until 7).forEach { i ->
                    val d = today.plusDays(i.toLong())
                    DayList(store, d, byDay[d].orEmpty(), onOpen, onNewAt, "Nothing scheduled.")
                }
                CalMode.Month -> {
                    val first = picked.withDayOfMonth(1)
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                        Text("‹", color = c.textPrimary, fontSize = 26.sp, modifier = Modifier.testTag("automationMonthPrev").heightIn(min = 44.dp).clickable(role = Role.Button) { picked = first.minusMonths(1) }.padding(horizontal = 14.dp))
                        Text(first.month.getDisplayName(TextStyle.FULL, Locale.getDefault()) + " " + first.year, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Text("›", color = c.textPrimary, fontSize = 26.sp, modifier = Modifier.testTag("automationMonthNext").heightIn(min = 44.dp).clickable(role = Role.Button) { picked = first.plusMonths(1) }.padding(horizontal = 14.dp))
                    }
                    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
                        listOf("S", "M", "T", "W", "T", "F", "S").forEach { Text(it, color = c.textTertiary, fontSize = 11.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center) }
                    }
                    val lead = first.dayOfWeek.value % 7
                    val cells = lead + first.lengthOfMonth()
                    Column(Modifier.padding(horizontal = 12.dp, vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        for (row in 0 until (cells + 6) / 7) {
                            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                for (col in 0 until 7) {
                                    val n = row * 7 + col - lead + 1
                                    if (n < 1 || n > first.lengthOfMonth()) { Box(Modifier.weight(1f).aspectRatio(1f)); continue }
                                    val d = first.withDayOfMonth(n)
                                    val count = byDay[d]?.size ?: 0
                                    val on = d == picked
                                    Column(
                                        Modifier.weight(1f).aspectRatio(1f).clip(RoundedCornerShape(10.dp)).testTag("automationDay_$d")
                                            .background(if (on) c.accentCode.copy(alpha = 0.22f) else c.fillNeutral)
                                            .clickable(role = Role.Button) { picked = d }.padding(4.dp),
                                        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center,
                                    ) {
                                        Text("$n", color = if (d == today) c.accentCode else c.textPrimary, fontSize = 14.sp, fontWeight = if (d == today) FontWeight.Bold else FontWeight.Normal)
                                        if (count > 0) Box(Modifier.size(6.dp).clip(CircleShape).background(c.accentCode))
                                    }
                                }
                            }
                        }
                    }
                    DayList(store, picked, byDay[picked].orEmpty(), onOpen, onNewAt, "Nothing scheduled.")
                }
            }
        }
    }
}

@Composable
private fun DayList(store: AutomationStore, day: LocalDate, items: List<Occurrence>, onOpen: (AutoTask) -> Unit, onNewAt: (String) -> Unit, emptyText: String) {
    val c = AllternitTheme.colors
    val today = LocalDate.now()
    val label = when (day) { today -> "Today"; today.plusDays(1) -> "Tomorrow"; else -> day.dayOfWeek.getDisplayName(TextStyle.FULL, Locale.getDefault()) + " " + day.monthValue + "/" + day.dayOfMonth }
    Column(Modifier.padding(horizontal = 16.dp, vertical = 6.dp).testTag("automationDayList_$day"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(label.uppercase(), color = c.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.Medium, letterSpacing = 0.6.sp)
        if (items.isEmpty()) Text(emptyText, color = c.textTertiary, fontSize = 13.sp)
        items.take(30).forEach { o -> OccurrenceRow(store, o, onOpen) }
        if (items.isEmpty()) Text("New task at 9:00 AM", color = c.accentCode, fontSize = 13.sp, modifier = Modifier.heightIn(min = 44.dp).testTag("automationNewAt_$day").clickable(role = Role.Button) { onNewAt(CronWords.DEFAULT_CRON) }.padding(vertical = 10.dp))
    }
}

/** The graph (iOS AutomationTasksGraph): goals with the routines and loops that serve them, then everything else; each with its run strip and next run. */
@Composable
internal fun AutomationGraph(store: AutomationStore, tasks: List<AutoTask>, all: Boolean, onOpen: (AutoTask) -> Unit) {
    val c = AllternitTheme.colors
    if (tasks.isEmpty()) { EmptyState("Nothing to draw yet", "Scheduled routines and loops appear here with their recent runs.", Modifier.padding(horizontal = 20.dp)); return }
    val goals = if (all) store.of(TaskType.Goal) else emptyList()
    val linked = tasks.filter { t -> t.goalId != null && goals.any { it.id == t.goalId } }
    val free = tasks - linked.toSet()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp).testTag("automationGraph"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        goals.forEach { g ->
            val kids = linked.filter { it.goalId == g.id }
            if (kids.isNotEmpty()) {
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).clickable(role = Role.Button) { onOpen(g) }.padding(12.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(g.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                    Text("${(g.progress ?: 0.0).toInt()}%", color = c.textSecondary, fontSize = 13.sp)
                }
                kids.forEach { GraphNode(store, it, indent = true, onOpen = onOpen) }
            }
        }
        free.forEach { GraphNode(store, it, indent = false, onOpen = onOpen) }
    }
}

@Composable
private fun GraphNode(store: AutomationStore, t: AutoTask, indent: Boolean, onOpen: (AutoTask) -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().padding(start = if (indent) 20.dp else 0.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)
            .testTag("automationNode_${t.id}").clickable(role = Role.Button) { onOpen(t) }.padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            TypeTag(t.type)
            Text(displayName(t.name), color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        }
        Text(listOfNotNull(CronWords.of(t), CronWords.nextLabel(t.nextRunAt)?.let { "next $it" }, store.botName(t.agentId)).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
        if (t.runs.isNotEmpty()) RunStrip(t.runs)
    }
}

/** One run in a day list: a tap opens it, press-and-hold then drag moves it in 15-minute steps (iOS SchedulerChip). */
@Composable
private fun OccurrenceRow(store: AutomationStore, o: Occurrence, onOpen: (AutoTask) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val view = LocalView.current
    val rule = remember(o.task.scheduleExpression) { if (o.task.isCron && o.task.canRun) RoutineRule.fromCron(o.task.scheduleExpression) else null }
    var dy by remember(o.task.id, o.at) { mutableFloatStateOf(0f) }
    var dragging by remember(o.task.id, o.at) { mutableStateOf(false) }
    val stepPx = with(density) { 24.dp.toPx() }
    fun moveBy(minutes: Int) {
        val r = rule ?: return
        val at = (r.at + minutes).coerceIn(0, 24 * 60 - 15)
        if (at != r.at) scope.launch { store.setCron(o.task, r.copy(at = at).cron) }
    }
    val locked = rule == null
    Row(
        Modifier.fillMaxWidth().heightIn(min = 44.dp)
            .offset { IntOffset(0, dy.roundToInt()) }
            .zIndex(if (dragging) 1f else 0f)
            .graphicsLayer { scaleX = if (dragging) 1.03f else 1f; scaleY = if (dragging) 1.03f else 1f; shadowElevation = if (dragging) 24f else 0f }
            .clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).testTag("automationOcc_${o.task.id}")
            .then(if (locked) Modifier else Modifier.pointerInput(o.task.id, o.at) {
                detectDragGesturesAfterLongPress(
                    onDragStart = { dragging = true; Haptics.perform(view, Haptics.Kind.Tap) },
                    onDrag = { change, amount -> change.consume(); dy += amount.y },
                    onDragEnd = { val steps = (dy / stepPx).roundToInt(); dragging = false; dy = 0f; moveBy(steps * 15) },
                    onDragCancel = { dragging = false; dy = 0f },
                )
            })
            .semantics {
                if (!locked) customActions = listOf(
                    CustomAccessibilityAction("Move 15 minutes earlier") { moveBy(-15); true },
                    CustomAccessibilityAction("Move 15 minutes later") { moveBy(15); true },
                )
            }
            .clickable(role = Role.Button) { onOpen(o.task) }.padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(CronWords.time(o.at), color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(end = 2.dp))
        Text(displayName(o.task.name), color = c.textPrimary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        TypeTag(o.task.type)
    }
}
