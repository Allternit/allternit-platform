package com.allternit.feature.bots

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Text
import androidx.compose.material3.TimeInput
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow

/**
 * A routine's schedule as people see it and the cron the scheduler runs (iOS RoutineRule, the port
 * of allternit-ai routine-rule.ts). Times are minutes after midnight, local time.
 */
data class RoutineRule(
    val repeatKind: Repeat,
    val at: Int,
    val until: Int? = null,
    val every: Int? = null,
    val days: List<Int>,
    val dom: List<Int>? = null,
) {
    enum class Repeat(val label: String) { Daily("Every day"), Weekdays("Weekdays"), Weekly("Weekly"), Monthly("Monthly"), Interval("Every few hours") }

    val cron: String
        get() {
            val hh = at / 60
            val mm = at % 60
            val d = days.toSortedSet().toList()
            val dow = when {
                repeatKind == Repeat.Daily || repeatKind == Repeat.Monthly || d.size == 7 -> "*"
                d == WEEKDAYS -> "1-5"
                else -> d.joinToString(",")
            }
            return when (repeatKind) {
                Repeat.Monthly -> "$mm $hh ${(dom?.takeIf { it.isNotEmpty() } ?: listOf(1)).joinToString(",")} * *"
                Repeat.Interval -> {
                    val e = every ?: 60
                    val last = (until ?: at) / 60
                    if (e % 60 == 0) "$mm $hh-$last/${e / 60} * * $dow" else "*/$e $hh-$last * * $dow"
                }
                else -> "$mm $hh * * $dow"
            }
        }

    val label: String
        get() {
            val t = time(at)
            return when (repeatKind) {
                Repeat.Daily -> "Every day · $t"
                Repeat.Weekdays -> "Weekdays · $t"
                Repeat.Weekly -> "${days.joinToString(", ") { DAY_SHORT[it] }} · $t"
                Repeat.Monthly -> "Monthly on the ${(dom ?: listOf(1)).joinToString(" & ") { ordinal(it) }} · $t"
                Repeat.Interval -> {
                    val e = every ?: 60
                    val span = when {
                        days.size == 7 -> ""
                        days == WEEKDAYS -> " weekdays"
                        else -> " " + days.joinToString(", ") { DAY_SHORT[it] }
                    }
                    "Every ${if (e % 60 == 0) "${e / 60} h" else "$e min"} · ${short(at)}–${short(until ?: at)}$span"
                }
            }
        }

    val isValid: Boolean
        get() = when (repeatKind) {
            Repeat.Weekly -> days.isNotEmpty()
            Repeat.Interval -> days.isNotEmpty() && (until ?: -1) >= at && (every ?: 0) in INTERVALS
            Repeat.Monthly -> !dom.isNullOrEmpty() && dom.all { it in 1..28 }
            else -> true
        }

    companion object {
        val ALL_DAYS = listOf(0, 1, 2, 3, 4, 5, 6)
        val WEEKDAYS = listOf(1, 2, 3, 4, 5)
        val INTERVALS = listOf(30, 60, 120, 180, 240)
        val DAY_SHORT = listOf("Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat")

        fun time(m: Int): String {
            val h = m / 60
            return "${if (h % 12 == 0) 12 else h % 12}:${String.format(java.util.Locale.US, "%02d", m % 60)} ${if (h < 12) "AM" else "PM"}"
        }

        private fun short(m: Int): String {
            val h = m / 60
            val min = m % 60
            return "${if (h % 12 == 0) 12 else h % 12}${if (min > 0) ":" + String.format(java.util.Locale.US, "%02d", min) else ""}${if (h < 12) "a" else "p"}"
        }

        private fun ordinal(n: Int): String {
            if (n % 100 in 11..13) return "${n}th"
            return "$n" + (listOf("th", "st", "nd", "rd").getOrNull(n % 10) ?: "th")
        }

        private fun parseField(field: String, lo: Int, hi: Int): Set<Int>? {
            val out = mutableSetOf<Int>()
            for (part in field.split(",")) {
                val pieces = part.split("/")
                val step = if (pieces.size > 1) pieces[1].toIntOrNull() ?: return null else 1
                if (step < 1) return null
                val range = pieces[0]
                val (s, e) = when {
                    range == "*" -> lo to hi
                    range.contains("-") -> range.split("-").let { ab -> if (ab.size != 2) return null else (ab[0].toIntOrNull() ?: return null) to (ab[1].toIntOrNull() ?: return null) }
                    else -> (range.toIntOrNull() ?: return null).let { it to it }
                }
                if (s < lo || e > hi || s > e) return null
                out.addAll(s..e step step)
            }
            return out.ifEmpty { null }
        }

        /** iOS RoutineRule(cron:): null when the cron is not one the editor can show. */
        fun fromCron(cron: String): RoutineRule? {
            val f = cron.trim().split(Regex("\\s+"))
            if (f.size != 5) return null
            val minutes = parseField(f[0], 0, 59) ?: return null
            val hours = parseField(f[1], 0, 23) ?: return null
            val doms = parseField(f[2], 1, 31) ?: return null
            val months = parseField(f[3], 1, 12) ?: return null
            val dows = (parseField(f[4], 0, 7) ?: return null).toMutableSet()
            if (months.size != 12) return null
            if (7 in dows) dows.add(0)
            val times = hours.sorted().flatMap { h -> minutes.sorted().map { h * 60 + it } }
            val first = times.firstOrNull() ?: return null
            val dowRestricted = f[4] != "*"
            val domRestricted = f[2] != "*"
            val days = if (dowRestricted) dows.filter { it < 7 }.sorted() else ALL_DAYS
            if (domRestricted) {
                if (dowRestricted || times.size != 1) return null
                return RoutineRule(Repeat.Monthly, first, days = ALL_DAYS, dom = doms.sorted())
            }
            if (times.size > 1) {
                val every = times[1] - times[0]
                if (times.indices.drop(1).any { times[it] - times[it - 1] != every } || every !in INTERVALS) return null
                return RoutineRule(Repeat.Interval, first, until = times.last(), every = every, days = days)
            }
            val kind = when {
                days.size == 7 -> Repeat.Daily
                days == WEEKDAYS -> Repeat.Weekdays
                else -> Repeat.Weekly
            }
            return RoutineRule(kind, first, days = days)
        }
    }
}

/**
 * The routine time editor (iOS RoutineScheduleEditor): repeat kind, the time, the days (weekly,
 * every few hours), the days of the month (monthly, 1–28), and the label of what it will do.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RoutineScheduleEditor(title: String, initial: RoutineRule, saving: Boolean, error: String?, onSave: (RoutineRule) -> Unit, onCancel: () -> Unit) {
    val c = AllternitTheme.colors
    var rule by remember { mutableStateOf(initial) }
    val time = rememberTimePickerState(initialHour = initial.at / 60, initialMinute = initial.at % 60, is24Hour = false)
    LaunchedEffect(time.hour, time.minute) {
        val at = time.hour * 60 + time.minute
        rule = rule.copy(at = at, until = rule.until?.let { maxOf(it, at) })
    }
    LibrarySheet(
        title = title, testTag = "routineScheduleEditor", onDismiss = onCancel,
        leading = LibrarySheetAction("Cancel", testTag = "routineScheduleCancel", onClick = onCancel),
        trailing = LibrarySheetAction(if (saving) "Saving…" else "Save", enabled = rule.isValid && !saving, bold = true, testTag = "routineScheduleSave") { onSave(rule) },
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(rule.label, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp).testTag("routineScheduleLabel"))
            PillRow(RoutineRule.Repeat.entries, rule.repeatKind, { it.label }, { "routineRepeat${it.name}" }, Modifier.padding(horizontal = 16.dp)) { k ->
                rule = when (k) {
                    RoutineRule.Repeat.Daily -> RoutineRule(k, rule.at, days = RoutineRule.ALL_DAYS)
                    RoutineRule.Repeat.Weekdays -> RoutineRule(k, rule.at, days = RoutineRule.WEEKDAYS)
                    RoutineRule.Repeat.Weekly -> RoutineRule(k, rule.at, days = rule.days.ifEmpty { listOf(1) })
                    RoutineRule.Repeat.Monthly -> RoutineRule(k, rule.at, days = RoutineRule.ALL_DAYS, dom = rule.dom ?: listOf(1))
                    RoutineRule.Repeat.Interval -> RoutineRule(k, rule.at, until = maxOf(rule.at + 8 * 60, rule.at).coerceAtMost(23 * 60), every = 60, days = RoutineRule.WEEKDAYS)
                }
            }
            SectionHeader(if (rule.repeatKind == RoutineRule.Repeat.Interval) "From" else "Time")
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { TimeInput(state = time, modifier = Modifier.testTag("routineScheduleTime")) }
            if (rule.repeatKind == RoutineRule.Repeat.Interval) {
                SectionHeader("Every")
                PillRow(RoutineRule.INTERVALS, rule.every ?: 60, { if (it % 60 == 0) "${it / 60} h" else "$it min" }, { "routineEvery$it" }, Modifier.padding(horizontal = 16.dp)) { rule = rule.copy(every = it) }
                SectionHeader("Until")
                val untilHours = (rule.at / 60..23).toList()
                PillRow(untilHours, (rule.until ?: rule.at) / 60, { RoutineRule.time(it * 60) }, { "routineUntil$it" }, Modifier.padding(horizontal = 16.dp)) { rule = rule.copy(until = it * 60 + rule.at % 60) }
            }
            if (rule.repeatKind == RoutineRule.Repeat.Weekly || rule.repeatKind == RoutineRule.Repeat.Interval) {
                SectionHeader("Days")
                Row(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    RoutineRule.DAY_SHORT.forEachIndexed { i, d ->
                        val on = i in rule.days
                        Box(
                            Modifier.size(40.dp).clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral).testTag("routineDay$i")
                                .clickable(role = Role.Checkbox) { rule = rule.copy(days = if (on) rule.days - i else (rule.days + i).sorted()) }
                                .semantics { contentDescription = d + if (on) ", on" else ", off" },
                            contentAlignment = Alignment.Center,
                        ) { Text(d.take(1), color = if (on) c.bgPrimary else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
                    }
                }
            }
            if (rule.repeatKind == RoutineRule.Repeat.Monthly) {
                SectionHeader("Days of the month")
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    (1..28).chunked(7).forEach { week ->
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            week.forEach { d ->
                                val on = d in rule.dom.orEmpty()
                                Box(
                                    Modifier.size(38.dp).clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral).testTag("routineDom$d")
                                        .clickable(role = Role.Checkbox) { val cur = rule.dom.orEmpty(); rule = rule.copy(dom = if (on) cur - d else (cur + d).sorted()) },
                                    contentAlignment = Alignment.Center,
                                ) { Text("$d", color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.sp) }
                            }
                        }
                    }
                }
            }
            error?.let { ErrorLine(it) }
        }
    }
}
