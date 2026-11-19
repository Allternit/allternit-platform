package com.allternit.feature.automation

import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.bots.RoutineRule
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId

/*
 * Cron in plain words, the next run times for the calendar, and the cron the Frequency pills write
 * (iOS CronWords + AutomationScheduleLogic). The rule editor is wave E's RoutineRule (:feature:bots).
 */
object CronWords {
    enum class Frequency(val label: String) { Hourly("Hourly"), Daily("Daily"), Weekdays("Weekdays"), Weekly("Weekly") }

    /** The cron the sheet saves (the PWA's routine-rule ruleToCron). */
    fun cron(f: Frequency, hour: Int, minute: Int, weekday: Int): String = when (f) {
        Frequency.Hourly -> "$minute * * * *"
        Frequency.Daily -> "$minute $hour * * *"
        Frequency.Weekdays -> "$minute $hour * * 1-5"
        Frequency.Weekly -> "$minute $hour * * $weekday"
    }

    fun describe(type: String, expression: String): String = when (type) {
        "manual" -> "Runs when you start it"
        "once" -> "Runs once"
        "interval" -> "Every $expression"
        else -> RoutineRule.fromCron(expression)?.label ?: expression.ifBlank { "No schedule" }
    }

    /** A task's schedule in words; goals have none. */
    fun of(t: AutoTask): String = if (t.type == TaskType.Goal) "" else describe(t.scheduleType, t.scheduleExpression)

    /** Cron of the default slot: 9:00 every day. */
    const val DEFAULT_CRON = "0 9 * * *"

    private fun field(f: String, lo: Int, hi: Int): Set<Int>? {
        val out = mutableSetOf<Int>()
        for (part in f.split(",")) {
            val pieces = part.split("/")
            val step = if (pieces.size == 2) pieces[1].toIntOrNull() ?: return null else 1
            if (step < 1) return null
            val base = pieces[0]
            val (s, e) = when {
                base == "*" -> lo to hi
                base.contains("-") -> base.split("-").let { ab -> if (ab.size != 2) return null else (ab[0].toIntOrNull() ?: return null) to (ab[1].toIntOrNull() ?: return null) }
                else -> (base.toIntOrNull() ?: return null).let { it to (if (pieces.size == 2) hi else it) }
            }
            if (s < lo || e > hi || s > e) return null
            out.addAll(s..e step step)
        }
        return out.ifEmpty { null }
    }

    /** Next runs of a cron routine within [days] days, soonest first; a non-cron task gives just its next-run time. */
    fun upcoming(t: AutoTask, days: Int, now: LocalDateTime = LocalDateTime.now()): List<LocalDateTime> {
        val f = t.scheduleExpression.trim().split(Regex("\\s+"))
        val minutes = if (f.size == 5 && t.isCron) field(f[0], 0, 59) else null
        val hours = if (minutes != null) field(f[1], 0, 23) else null
        val doms = if (hours != null) field(f[2], 1, 31) else null
        val months = if (doms != null) field(f[3], 1, 12) else null
        val dows = if (months != null) field(f[4], 0, 7) else null
        if (minutes == null || hours == null || doms == null || months == null || dows == null) {
            val ms = parseTime(t.nextRunAt)
            return if (ms > 0) listOf(LocalDateTime.ofInstant(Instant.ofEpochMilli(ms), ZoneId.systemDefault())) else emptyList()
        }
        val out = ArrayList<LocalDateTime>()
        var day = now.toLocalDate()
        repeat(days) {
            val dow = day.dayOfWeek.value % 7
            if (day.dayOfMonth in doms && day.monthValue in months && (dow in dows || (dow == 0 && 7 in dows))) {
                for (h in hours.sorted()) for (m in minutes.sorted()) {
                    val at = day.atTime(h, m)
                    if (at.isAfter(now)) { out.add(at); if (out.size > 400) return out }
                }
            }
            day = day.plusDays(1)
        }
        return out
    }

    fun time(at: LocalDateTime): String = RoutineRule.time(at.hour * 60 + at.minute)

    /** "in 5 min", "tomorrow 9:00 AM": the next-run line on a card. */
    fun nextLabel(raw: String?): String? {
        val ms = parseTime(raw)
        if (ms <= 0) return null
        val at = LocalDateTime.ofInstant(Instant.ofEpochMilli(ms), ZoneId.systemDefault())
        val today = LocalDate.now()
        val day = when (at.toLocalDate()) {
            today -> "Today"
            today.plusDays(1) -> "Tomorrow"
            else -> at.dayOfWeek.name.take(3).lowercase().replaceFirstChar { it.uppercase() } + " " + at.monthValue + "/" + at.dayOfMonth
        }
        return "$day ${time(at)}"
    }
}
