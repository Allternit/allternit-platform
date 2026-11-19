package com.allternit.feature.code

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.Calendar

/*
 * Code extras that iOS keeps beside the composer and the launch screen: the usage dashboard
 * (CodeUsageDashboard, computed from the account's Code threads) and the Scheduled screen
 * (CodeScheduledScreen: the routines on the computer Code runs on, with New task).
 */

private const val DAY_MS = 86_400_000L

/** What the usage dashboard shows for a range; pure so it can be checked on its own. */
data class CodeUsageMetrics(
    val sessions: Int, val activeDays: Int, val peakHour: Int?, val currentStreak: Int, val longestStreak: Int, val last7: Int,
    /** Thread count per local day number (epoch days in the local zone). */
    val perDay: Map<Long, Int>,
)

internal fun localDay(millis: Long): Long {
    val cal = Calendar.getInstance().apply { timeInMillis = millis }
    return (millis + cal.get(Calendar.ZONE_OFFSET) + cal.get(Calendar.DST_OFFSET)) / DAY_MS
}

internal fun codeUsageMetrics(rows: List<CodeSessionRow>, rangeDays: Int?, now: Long = System.currentTimeMillis()): CodeUsageMetrics {
    val today = localDay(now)
    val filtered = rows.filter { it.updatedMillis > 0 && (rangeDays == null || localDay(it.updatedMillis) > today - rangeDays) }
    val perDay = HashMap<Long, Int>()
    val hours = IntArray(24)
    for (r in filtered) {
        perDay[localDay(r.updatedMillis)] = (perDay[localDay(r.updatedMillis)] ?: 0) + 1
        hours[Calendar.getInstance().apply { timeInMillis = r.updatedMillis }.get(Calendar.HOUR_OF_DAY)]++
    }
    val days = perDay.keys.sorted()
    var longest = 0
    var run = 0
    var prev = Long.MIN_VALUE
    for (d in days) { run = if (d == prev + 1) run + 1 else 1; longest = maxOf(longest, run); prev = d }
    // The current streak counts back from today, or from yesterday when today is still empty.
    var cur = 0
    var d = if (perDay.containsKey(today)) today else today - 1
    while (perDay.containsKey(d)) { cur++; d-- }
    val peak = if (filtered.isEmpty()) null else hours.indices.maxByOrNull { hours[it] }
    return CodeUsageMetrics(filtered.size, perDay.size, peak, cur, longest, filtered.count { localDay(it.updatedMillis) > today - 7 }, perDay)
}

private fun hourLabel(h: Int?): String = when {
    h == null -> "-"
    h == 0 -> "12 AM"
    h < 12 -> "$h AM"
    h == 12 -> "12 PM"
    else -> "${h - 12} PM"
}

/** iOS CodeUsageDashboard: All / 30d / 7d, six stat tiles and a 12-week activity heatmap. */
@Composable
internal fun CodeUsageScreen(host: CodeHost, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    var range by remember { mutableStateOf<Int?>(null) }
    androidx.compose.runtime.LaunchedEffect(Unit) { host.reloadSessions() }
    val rows = host.sessions()
    val m = codeUsageMetrics(rows, range)
    CodeScaffold("Usage", "Your Code threads", onBack, "codeUsageBack") {
        ScrollBody {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf<Pair<String, Int?>>("All" to null, "30d" to 30, "7d" to 7).forEach { (label, days) ->
                    Text(
                        label, color = if (range == days) c.textPrimary else c.textTertiary, fontSize = 13.sp,
                        modifier = Modifier.testTag("codeUsageRange-$label").clip(RoundedCornerShape(10.dp))
                            .background(if (range == days) c.fillNeutral else androidx.compose.ui.graphics.Color.Transparent)
                            .clickable { range = days }.padding(horizontal = 14.dp, vertical = 10.dp),
                    )
                }
            }
            if (rows.isEmpty()) {
                EmptyState("No usage yet", "Start a Code thread and your activity shows up here.")
            } else {
                val tiles = listOf(
                    "Sessions" to m.sessions.toString(), "Active days" to m.activeDays.toString(), "Last 7 days" to m.last7.toString(),
                    "Current streak" to "${m.currentStreak}d", "Longest streak" to "${m.longestStreak}d", "Peak hour" to hourLabel(m.peakHour),
                )
                tiles.chunked(2).forEach { pair ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        pair.forEach { (label, value) ->
                            Column(Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(14.dp).testTag("codeUsageTile-$label")) {
                                Text(label, color = c.textSecondary, fontSize = 12.sp)
                                Text(value, color = c.textPrimary, fontSize = 22.sp)
                            }
                        }
                    }
                }
                SectionLabel("Activity, last 12 weeks")
                Heatmap(m.perDay)
            }
        }
    }
}

/** 12 weeks of days, one column per week (oldest left), shaded by thread count. */
@Composable
private fun Heatmap(perDay: Map<Long, Int>) {
    val c = AllternitTheme.colors
    val today = localDay(System.currentTimeMillis())
    val dow = Calendar.getInstance().get(Calendar.DAY_OF_WEEK) - 1 // 0 = Sunday
    val maxCount = (perDay.values.maxOrNull() ?: 1).coerceAtLeast(1)
    Row(Modifier.padding(horizontal = 16.dp).testTag("codeUsageHeatmap"), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        for (w in 11 downTo 0) {
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                for (d in 0..6) {
                    val day = today - dow - w * 7 + d
                    val n = if (day > today) -1 else perDay[day] ?: 0
                    Box(
                        Modifier.size(20.dp).clip(RoundedCornerShape(4.dp))
                            .background(when {
                                n < 0 -> androidx.compose.ui.graphics.Color.Transparent
                                n == 0 -> c.fillNeutral
                                else -> c.accentCode.copy(alpha = 0.25f + 0.75f * n / maxCount)
                            }),
                    )
                }
            }
        }
    }
}

// MARK: - Scheduled

private data class ScheduledRoutine(val id: String, val name: String, val schedule: String?, val trigger: String?, val state: String)

private fun parseRoutines(json: JsonElement): List<ScheduledRoutine> {
    val arr: List<JsonObject> = when (json) {
        is JsonArray -> json.filterIsInstance<JsonObject>()
        is JsonObject -> json.list("routines")
        else -> emptyList()
    }
    return arr.mapNotNull { o -> o.str("id")?.let { ScheduledRoutine(it, o.str("name").orEmpty(), o.str("schedule"), o.str("trigger"), o.str("state") ?: "idle") } }
}

private fun routineLine(r: ScheduledRoutine): String {
    val when_ = r.schedule?.takeIf { it.isNotBlank() }?.let { "Every $it" } ?: r.trigger?.takeIf { it.isNotBlank() } ?: "Manual"
    val state = when (r.state) { "running" -> "Running"; "done", "completed" -> "Done"; "failed" -> "Failed"; else -> "Idle" }
    return "$when_ · $state"
}

/** iOS CodeScheduledScreen: the routines on the computer Code runs on, with New task. */
@Composable
internal fun CodeScheduledScreen(host: CodeHost, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    var creating by remember { mutableStateOf(false) }
    var createError by remember { mutableStateOf<String?>(null) }
    CodeScaffold("Scheduled", null, onBack, "codeScheduledBack") {
        CodeLoader(Unit, "Couldn't load scheduled tasks", load = { host.backend.codeCall("GET", "v1/automations/routines", null, "load scheduled tasks").map { parseRoutines(it) } }) { routines, reload ->
            ScrollBody {
                if (routines.isEmpty()) EmptyState("No scheduled tasks", "Scheduled tasks on your computer show here.")
                routines.forEach { r -> CodeRow(r.name, routineLine(r), "codeScheduled-${r.id}") { } }
                createError?.let { Text(it, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }
                Box(Modifier.padding(16.dp)) { CodePill("New task", testTag = "codeScheduledNew", filled = true) { creating = true } }
            }
            if (creating) NewTaskSheet(onDismiss = { creating = false }) { name, command ->
                creating = false
                scope.launch {
                    val body = buildJsonObject {
                        put("name", name)
                        put("steps", buildJsonArray { add(buildJsonObject { put("command", command); put("status", "pending") }) })
                    }.toString()
                    when (val r = host.backend.codeCall("POST", "v1/automations/routines", body, "create the task")) {
                        is CodeResult.Failed -> createError = r.message
                        CodeResult.NoComputer -> createError = "Connect a computer to schedule tasks."
                        is CodeResult.Ok -> { createError = null; reload() }
                    }
                }
            }
        }
    }
}

@Composable
private fun NewTaskSheet(onDismiss: () -> Unit, onCreate: (name: String, command: String) -> Unit) {
    var name by remember { mutableStateOf("") }
    var command by remember { mutableStateOf("") }
    val c = AllternitTheme.colors
    com.allternit.feature.artifacts.LibrarySheet(
        title = "New task", testTag = "codeNewTaskSheet", onDismiss = onDismiss,
        leading = com.allternit.feature.artifacts.LibrarySheetAction("Cancel", onClick = onDismiss),
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            for ((value, hint, tag, set) in listOf(
                Quad(name, "Task name", "codeNewTaskName") { v: String -> name = v },
                Quad(command, "Command to run", "codeNewTaskCommand") { v: String -> command = v },
            )) {
                BasicTextField(
                    value = value, onValueChange = set, singleLine = true,
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp), cursorBrush = SolidColor(c.textPrimary),
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(14.dp).testTag(tag),
                    decorationBox = { inner -> Box { if (value.isEmpty()) Text(hint, color = c.textTertiary, fontSize = 16.sp); inner() } },
                )
            }
            CodePill("Create", testTag = "codeNewTaskCreate", filled = true, enabled = name.isNotBlank() && command.isNotBlank()) { onCreate(name.trim(), command.trim()) }
        }
    }
}

private data class Quad(val a: String, val b: String, val c: String, val d: (String) -> Unit)
