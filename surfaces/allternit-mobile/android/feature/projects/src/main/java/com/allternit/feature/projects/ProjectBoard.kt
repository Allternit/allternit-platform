package com.allternit.feature.projects

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Locale
import kotlinx.serialization.json.JsonObject

/*
 * The project board (iOS Board/ProjectDashboard.swift): ProjectBoardData from the status record,
 * the project's threads and files, and the eight Style tiles as the pad draws them one at a time
 * (Bento look): Your decision, Countdown, Progress, Team, Activity, Schedule, Files, Latest changes.
 */

/** `GET cowork/projects/:id/status` → status (iOS ProjectStatusRecord). */
data class ProjectStatus(
    val lifecycle: String = "active",
    val state: String = "on_track",
    val nextAction: String? = null,
    val sections: Map<String, String> = emptyMap(),
    val log: List<StatusLogEntry> = emptyList(),
) {
    val closed get() = lifecycle == "closed"
    val stateLabel get() = when (state) { "at_risk" -> "At risk"; "blocked" -> "Blocked"; "needs_decision" -> "Needs decision"; "done" -> "Done"; else -> "On track" }

    companion object {
        val sectionOrder = listOf("goal", "status", "next_actions", "owners", "deadlines", "decisions", "blockers", "outcome", "lessons")
        fun from(o: JsonObject?): ProjectStatus? {
            o ?: return null
            val secs = o.child("sections")?.mapNotNull { (k, v) -> (v as? JsonObject).str("text")?.let { k to it } }?.toMap().orEmpty()
            return ProjectStatus(
                o.str("lifecycle") ?: "active", o.str("state") ?: "on_track", o.str("next_action"), secs,
                o.list("log").map { StatusLogEntry(parseTime(it.str("at")), it.str("by") ?: "", it.str("note") ?: "") },
            )
        }
    }
}

data class StatusLogEntry(val at: Long, val by: String, val note: String)
data class BoardWork(val id: String, val title: String, val column: String, val at: Long)
data class BoardDeadline(val at: Long, val label: String)

/** iOS ProjectBoardData, built the way the chat project Overview builds it. */
data class ProjectBoardData(
    val status: ProjectStatus?,
    val work: List<BoardWork>,
    val made: List<String>,
    val people: List<Pair<String, String?>> = emptyList(),
) {
    val needsYou get() = work.count { it.column == "needsYou" }
    val done get() = work.count { it.column == "done" }
    val total get() = work.size

    /** Deadlines parsed from the status page's deadlines section (one "date — label" per line). */
    val deadlines: List<BoardDeadline> by lazy {
        status?.sections?.get("deadlines").orEmpty().lines().mapNotNull { line ->
            val m = Regex("(\\d{4}-\\d{2}-\\d{2})").find(line) ?: return@mapNotNull null
            val at = parseTime(m.value + "T00:00:00Z").takeIf { it > 0 } ?: return@mapNotNull null
            BoardDeadline(at, line.replace(m.value, "").trim(' ', '-', '—', ':', '–').ifEmpty { "Deadline" })
        }.sortedBy { it.at }
    }
    val nextDeadline get() = deadlines.firstOrNull { it.at >= startOfToday() }

    /** 14 days of activity: status log and work timestamps per day, oldest first. */
    val activity: List<Int> by lazy {
        val today = startOfToday()
        val counts = IntArray(14)
        (status?.log.orEmpty().map { it.at } + work.map { it.at }).forEach { t ->
            if (t <= 0) return@forEach
            val day = ((today - startOfDay(t)) / 86_400_000L).toInt()
            if (day in 0..13) counts[13 - day]++
        }
        counts.toList()
    }

    /** Latest changes: status log + work, newest first. */
    val stream: List<Triple<Long, String, String>> by lazy {
        (status?.log.orEmpty().map { Triple(it.at, it.by.ifEmpty { "Allternit" }, it.note) } + work.map { Triple(it.at, it.title, columnVerb(it.column)) })
            .filter { it.first > 0 }.sortedByDescending { it.first }
    }

    companion object {
        /** iOS: thread group → column (waiting → needsYou, working → working, queued/idle → queued, resolved → done). */
        fun column(group: String?) = when (group) { "waiting" -> "needsYou"; "working" -> "working"; "resolved" -> "done"; else -> "queued" }
        private fun columnVerb(c: String) = when (c) { "needsYou" -> "needs you"; "working" -> "is working"; "done" -> "is done"; else -> "is queued" }
    }
}

private fun startOfDay(t: Long): Long = Calendar.getInstance().apply { timeInMillis = t; set(Calendar.HOUR_OF_DAY, 0); set(Calendar.MINUTE, 0); set(Calendar.SECOND, 0); set(Calendar.MILLISECOND, 0) }.timeInMillis
private fun startOfToday(): Long = startOfDay(System.currentTimeMillis())

// MARK: - Palette (iOS ProjectPalette.slot)

private data class Slot(val fill: Color, val ink: Color)

private val darkInk = Color(0xFF15171C)
private val lightInk = Color(0xFFF2F3F5)

private val brightSets = mapOf(
    "bright" to listOf(Color(0xFF2F54EB) to Color.White, Color(0xFFFF5A36) to darkInk, Color(0xFF0FA876) to darkInk, Color(0xFFFFC31F) to darkInk, Color(0xFFD6338F) to Color.White),
    "sunset" to listOf(Color(0xFFD93A2A) to Color.White, Color(0xFF7B3FF2) to Color.White, Color(0xFFFFA51F) to darkInk, Color(0xFFE62E86) to Color.White, Color(0xFF0F8585) to Color.White),
)

private fun mix(a: Color, b: Color, t: Float) = Color(a.red * t + b.red * (1 - t), a.green * t + b.green * (1 - t), a.blue * t + b.blue * (1 - t))
private fun inkOn(c: Color) = if (c.luminance() > 0.45f) darkInk else Color.White

private fun slot(colors: String, i: Int, night: Boolean): Slot {
    val k = i % 5
    brightSets[colors]?.let { val (f, ink) = it[k]; return Slot(f, ink) }
    val own = colors.split(",").takeIf { it.size == 2 && it.all { h -> h.startsWith("#") } }
    if (own != null) {
        val main = hexColor(own[0]); val accent = hexColor(own[1])
        val page = if (night) Color(0xFF262625) else Color.White
        return when (k) {
            0 -> Slot(main, inkOn(main)); 1 -> Slot(accent, inkOn(accent))
            2 -> mix(main, page, 0.28f).let { Slot(it, inkOn(it)) }; 3 -> mix(main, page, 0.18f).let { Slot(it, inkOn(it)) }
            else -> mix(main, page, 0.10f).let { Slot(it, inkOn(it)) }
        }
    }
    if (k == 1) return Slot(padAccent(colors), Color.White)
    val neutrals = if (night) listOf(Color(0xFFEEF0F3), Color.Unspecified, Color(0xFF3B3E45), Color(0xFF303238), Color(0xFF292B30))
    else listOf(Color(0xFF23252B), Color.Unspecified, Color(0xFFD3D6DB), Color(0xFFE1E3E6), Color(0xFFEAEBED))
    val ink = if (k == 0) (if (night) darkInk else Color.White) else (if (night) lightInk else darkInk)
    return Slot(neutrals[k], ink)
}

// MARK: - Tiles

private val dateShort = SimpleDateFormat("MMM d", Locale.getDefault())
private val dateTime = SimpleDateFormat("MMM d, h:mm a", Locale.getDefault())

/** One Style tile on the pad, drawn like iOS ProjectDashboardView (Bento) with only that tile. */
@Composable
fun StyleTile(id: PadTileId, board: ProjectBoardData, colors: String, modifier: Modifier = Modifier, onOpenStatus: () -> Unit = {}) {
    val c = AllternitTheme.colors
    val night = c.bgPrimary.luminance() < 0.5f
    val plain = Slot(c.bgPanel, c.textPrimary)
    val accent = slot(colors, 1, night).fill
    val (s, minH) = when (id) {
        PadTileId.Decision -> slot(colors, 0, night) to 170
        PadTileId.Countdown -> slot(colors, 3, night) to 170
        PadTileId.Team -> slot(colors, 2, night) to 110
        PadTileId.Schedule -> slot(colors, 4, night) to 110
        PadTileId.Progress -> plain to 170
        else -> plain to 110
    }
    Column(
        modifier.heightIn(min = minH.dp).clip(RoundedCornerShape(20.dp)).background(s.fill).padding(16.dp).testTag("padTile${id.raw}"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val ink = s.ink
        @Composable fun label(t: String) = Text(t, color = ink, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold)
        @Composable fun body(t: String) = Text(t, color = ink.copy(alpha = 0.8f), fontSize = 13.sp)
        when (id) {
            PadTileId.Decision -> {
                val st = board.status
                val (lab, text, needs) = when {
                    board.needsYou > 0 -> Triple("Needs you", board.work.first { it.column == "needsYou" }.title, true)
                    st != null && (st.state == "needs_decision" || st.state == "blocked") ->
                        Triple(st.stateLabel, st.nextAction ?: st.sections["blockers"] ?: "Check the status page", true)
                    st?.nextAction != null -> Triple("Next", st.nextAction, false)
                    else -> Triple(null, null, false)
                }
                if (text == null) {
                    label("✓ Nothing needs you")
                    Text("You're clear.", color = ink, fontSize = 26.sp, fontWeight = FontWeight.Bold)
                } else {
                    label((if (needs) "⚠ " else "✓ ") + lab)
                    Text(text, color = ink, fontSize = 26.sp, fontWeight = FontWeight.Bold, maxLines = 3, overflow = TextOverflow.Ellipsis)
                    Pill(if (needs) "Open" else "Status page", ink, s.fill, onOpenStatus)
                }
            }
            PadTileId.Countdown -> {
                val d = board.nextDeadline
                if (d == null) { label("Countdown"); body("No dated deadline yet. Add one on the status page.") }
                else {
                    val days = ((startOfDay(d.at) - startOfToday()) / 86_400_000L).toInt()
                    label(d.label)
                    Text("$days", color = ink, fontSize = 64.sp, fontWeight = FontWeight.Black, fontFamily = FontFamily.Monospace, maxLines = 1)
                    Text("$days day${if (days == 1) "" else "s"} to ${dateShort.format(d.at)}", color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                }
            }
            PadTileId.Progress -> {
                label("Progress")
                if (board.total == 0) body("Progress shows here once this project has work.")
                else Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { ProgressRing(board.done, board.total, accent, ink) }
            }
            PadTileId.Team -> {
                label("Team")
                if (board.people.isEmpty()) body("No bots on this project yet.")
                board.people.take(5).forEach { (name, doing) ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(Modifier.size(28.dp).clip(CircleShape).background(ink.copy(alpha = 0.15f)), contentAlignment = Alignment.Center) { Text(name.take(1).uppercase(), color = ink, fontSize = 12.sp, fontWeight = FontWeight.Bold) }
                        Column { Text(name, color = ink, fontSize = 13.5.sp, fontWeight = FontWeight.Bold); Text(doing ?: "Idle", color = ink.copy(alpha = 0.75f), fontSize = 13.sp) }
                    }
                }
            }
            PadTileId.Chart -> ActivityChart(board.activity, accent, ink)
            PadTileId.Schedule -> {
                label("Coming up")
                val upcoming = board.deadlines.filter { it.at >= startOfToday() }.take(5)
                if (upcoming.isEmpty()) body("No deadlines on the status page yet.")
                upcoming.forEach { d ->
                    Row { Text(dateShort.format(d.at), color = ink, fontSize = 13.sp, fontWeight = FontWeight.Bold, modifier = Modifier.width(52.dp)); Text(d.label, color = ink, fontSize = 14.sp) }
                }
            }
            PadTileId.Files -> {
                label("Made so far")
                if (board.made.isEmpty()) body("Files and things made in this project show here.")
                board.made.take(4).chunked(2).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        row.forEach { name ->
                            Column(Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(10.dp)) {
                                Text(name, color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text("File", color = ink.copy(alpha = 0.7f), fontSize = 12.sp)
                            }
                        }
                        if (row.size == 1) Box(Modifier.weight(1f))
                    }
                }
            }
            PadTileId.Activity -> {
                label("Latest changes")
                if (board.stream.isEmpty()) body("Changes show here as work happens.")
                board.stream.take(3).forEach { (at, by, text) ->
                    Column {
                        Text(androidx.compose.ui.text.buildAnnotatedString {
                            pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Bold)); append(by); pop(); append(" "); append(text)
                        }, color = ink, fontSize = 13.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        Text(dateTime.format(at), color = ink.copy(alpha = 0.6f), fontSize = 12.sp)
                    }
                }
            }
            else -> Unit
        }
    }
}

@Composable
private fun Pill(text: String, bg: Color, ink: Color, onClick: () -> Unit) {
    Text(
        text, color = ink, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).background(bg).clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 13.dp),
    )
}

@Composable
private fun ProgressRing(done: Int, total: Int, accent: Color, ink: Color) {
    val size = 112.dp
    Box(Modifier.size(size), contentAlignment = Alignment.Center) {
        Canvas(Modifier.size(size).semantics { contentDescription = "$done of $total done" }) {
            val stroke = this.size.width / 9
            val inset = stroke / 2
            val arcSize = androidx.compose.ui.geometry.Size(this.size.width - stroke, this.size.height - stroke)
            drawArc(ink.copy(alpha = 0.12f), 0f, 360f, false, Offset(inset, inset), arcSize, style = Stroke(stroke))
            drawArc(accent, -90f, 360f * done / total.coerceAtLeast(1), false, Offset(inset, inset), arcSize, style = Stroke(stroke, cap = StrokeCap.Round))
        }
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Text("$done/$total", color = ink, fontSize = (112 / 4.2).sp, fontWeight = FontWeight.Black)
            Text("done", color = ink.copy(alpha = 0.7f), fontSize = 12.sp)
        }
    }
}

/** iOS Activity tile: 14-day line, a big count for the selected day (drag to scrub), first date … Today. */
@Composable
private fun ActivityChart(days: List<Int>, accent: Color, ink: Color) {
    var selected by remember(days) { mutableIntStateOf(days.lastIndex) }
    val total = days.sum()
    val n = days.getOrElse(selected) { 0 }
    Column(Modifier.semantics(mergeDescendants = true) { contentDescription = "$total changes in the last 14 days, ${days.lastOrNull() ?: 0} today" }) {
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("$n", color = accent, fontSize = 38.sp, fontWeight = FontWeight.Black, fontFamily = FontFamily.Monospace)
            Text(if (n == 1) "change" else "changes", color = ink.copy(alpha = 0.7f), fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp))
        }
        val max = (days.maxOrNull() ?: 0).coerceAtLeast(1)
        Canvas(
            Modifier.fillMaxWidth().height(130.dp)
                .pointerInput(days) {
                    fun at(x: Float) { selected = ((x / size.width) * (days.size - 1)).toInt().coerceIn(0, days.lastIndex) }
                    detectTapGestures { at(it.x) }
                }
                .pointerInput(days) { detectDragGestures { ch, _ -> selected = ((ch.position.x / size.width) * (days.size - 1)).toInt().coerceIn(0, days.lastIndex) } },
        ) {
            if (days.isEmpty()) return@Canvas
            val step = size.width / (days.size - 1).coerceAtLeast(1)
            fun y(v: Int) = size.height - 8f - (size.height - 16f) * v / max
            val path = Path().apply { days.forEachIndexed { i, v -> if (i == 0) moveTo(0f, y(v)) else lineTo(i * step, y(v)) } }
            drawPath(path, accent, style = Stroke(3.dp.toPx(), cap = StrokeCap.Round))
            drawLine(ink.copy(alpha = 0.2f), Offset(selected * step, 0f), Offset(selected * step, size.height))
            drawCircle(accent, 5.dp.toPx(), Offset(selected * step, y(days[selected])))
        }
        Row {
            val first = startOfToday() - 13 * 86_400_000L
            Text(dateShort.format(first), color = ink.copy(alpha = 0.6f), fontSize = 12.sp, modifier = Modifier.weight(1f))
            Text("Today", color = ink.copy(alpha = 0.6f), fontSize = 12.sp)
        }
    }
}
