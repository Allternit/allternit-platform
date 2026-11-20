package com.allternit.feature.computer

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonObject

/** Shown when GET /api/v1/computers/console answers 404 or 501. */
internal const val CONSOLE_UNAVAILABLE = "The Computers console is not available on your account yet."

/** The `computers_console` flag: device-local, default off, so today's screen stays as it is. */
internal object ConsoleFlag {
    private const val KEY = "computers_console"
    fun enabled(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY, false)
    fun set(context: Context, on: Boolean) =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY, on).apply()
}

/** The snapshot subset the list needs (contract HostSnapshot). */
internal data class ConsoleSnapshot(
    val sampledAt: String?, val cpuPercent: Double?, val memUsedBytes: Long?, val memTotalBytes: Long?, val diskPercent: Double?,
)

/** One row of `GET /api/v1/computers/console` (contract ConsoleComputer). */
internal data class ConsoleComputer(
    val id: String, val kind: String, val group: String, val name: String, val provider: String, val region: String?,
    val os: String, val osFamily: String, val address: String?, val reach: String, val status: String,
    val sample: ConsoleSnapshot?, val servicesCount: Int?, val lastBackupAt: String?,
) {
    val isOnline: Boolean get() = status.lowercase() in setOf("online", "running", "ready", "active")
    val isAsleep: Boolean get() = status.lowercase() in setOf("asleep", "sleeping", "stopped", "paused")

    companion object {
        fun parse(o: JsonObject): ConsoleComputer? {
            val id = o.str("id") ?: return null
            val s = o.child("lastSample")
            val sample = s?.let {
                val disks = it.list("disk")
                val used = disks.sumOf { d -> d.long("usedBytes") ?: 0L }
                val total = disks.sumOf { d -> d.long("totalBytes") ?: 0L }
                ConsoleSnapshot(
                    it.str("sampledAt"), it.child("cpu").num("percent"),
                    it.child("memory").long("usedBytes"), it.child("memory").long("totalBytes"),
                    if (total > 0) used * 100.0 / total else null,
                )
            }
            val kind = o.str("kind") ?: "paired"
            return ConsoleComputer(
                id, kind, o.str("group") ?: when (kind) { "cloud" -> "Allternit Cloud"; "desktop" -> "On your desk"; else -> "Your servers" },
                o.str("name") ?: id, o.str("provider").orEmpty(), o.str("region"), o.str("os").orEmpty(), o.str("osFamily").orEmpty(),
                o.str("address"), o.str("reach").orEmpty(), o.str("status").orEmpty(), sample,
                o.num("servicesCount")?.toInt(), o.str("lastBackupAt"),
            )
        }
    }
}

/** What the console list shows, one state at a time (loading / error / unavailable / empty / rows). */
internal sealed interface ConsoleState {
    data object Loading : ConsoleState
    data object Unavailable : ConsoleState
    data class Failed(val message: String) : ConsoleState
    data class Loaded(val computers: List<ConsoleComputer>) : ConsoleState
}

private const val HOT = 85.0

private data class Attention(val title: String, val detail: String, val warn: Boolean, val computer: ConsoleComputer)

private fun attention(list: List<ConsoleComputer>): List<Attention> = list.flatMap { cp ->
    buildList {
        if (!cp.isOnline && !cp.isAsleep) add(Attention("${cp.name} is offline", "Last sample ${cp.sample?.sampledAt?.let { lastSeen(it).removePrefix("Last seen ") } ?: "unavailable"}.", false, cp))
        cp.sample?.diskPercent?.takeIf { it >= HOT }?.let { add(Attention("${cp.name} disk is ${it.toInt()}% full", "Free some space before it fills.", true, cp)) }
        cp.sample?.cpuPercent?.takeIf { it >= 95.0 }?.let { add(Attention("${cp.name} CPU is at ${it.toInt()}%", "It has been busy in the last sample.", true, cp)) }
    }
}

/** DotMeter: eight dots, filled to [percent]; hot (amber) at 85% and over when [hot] is set. */
@Composable
internal fun DotMeter(percent: Double?, hot: Boolean = false, dots: Int = 8) {
    val c = AllternitTheme.colors
    val filled = if (percent == null) 0 else ((percent.coerceIn(0.0, 100.0) / 100.0) * dots).let { Math.ceil(it).toInt() }
    val on = if (hot && percent != null && percent >= HOT) AllternitStatic.StatusWarning else c.textPrimary
    Row(horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
        for (i in 0 until dots) Box(Modifier.size(5.dp).clip(CircleShape).background(if (i < filled) on else c.fillNeutral))
    }
}

/** Computers console list (mockup Overview, phone branch). Replaces the device rows when the flag is on. */
@Composable
internal fun AciConsoleOverview(
    state: ConsoleState, onRetry: () -> Unit, onPair: () -> Unit, onHosted: () -> Unit, onOpen: (ConsoleComputer) -> Unit,
) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().testTag("aciConsole"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Column {
            AciWideButton(DesignR.drawable.ph_link, "Pair a remote computer", "aciConsolePair", onPair)
            AciWideButton(DesignR.drawable.ph_plus, "Get a hosted computer", "aciConsoleHosted", onHosted)
        }
        when (state) {
            ConsoleState.Loading -> Box(Modifier.fillMaxWidth().padding(vertical = 40.dp), contentAlignment = Alignment.Center) {
                CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            }
            ConsoleState.Unavailable -> FriendlyState(FriendlyStyle.Empty, "Not available yet", CONSOLE_UNAVAILABLE, Modifier.padding(16.dp)) {
                Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary)
            }
            is ConsoleState.Failed -> FriendlyState(FriendlyStyle.Error, "Couldn't load computers", state.message, Modifier.padding(16.dp),
                actionTitle = "Retry", onAction = onRetry) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError) }
            is ConsoleState.Loaded -> if (state.computers.isEmpty()) {
                FriendlyState(FriendlyStyle.Empty, "No computers yet", "Pair a computer you run, or get a hosted one, and it shows up here.", Modifier.padding(16.dp)) {
                    Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary)
                }
            } else ConsoleLoaded(state.computers, onOpen)
        }
    }
}

@Composable
private fun ConsoleCard(content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp))) { content() }
}

@Composable
private fun ConsoleLoaded(list: List<ConsoleComputer>, onOpen: (ConsoleComputer) -> Unit) {
    val c = AllternitTheme.colors
    val online = list.count { it.isOnline }
    val services = list.sumOf { it.servicesCount ?: 0 }
    val cpus = list.mapNotNull { if (it.isOnline) it.sample?.cpuPercent else null }
    val backedUp = list.count { it.lastBackupAt != null }
    val cells = listOf(
        Triple("Computers", "${list.size}", "$online online"),
        Triple("Services", "$services", ""),
        Triple("Average CPU", if (cpus.isEmpty()) "—" else "${cpus.average().toInt()}%", ""),
        Triple("Backups", "$backedUp/${list.size}", ""),
    )
    ConsoleCard {
        for (row in 0..1) {
            Row(Modifier.fillMaxWidth()) {
                for (col in 0..1) {
                    val (l, v, s) = cells[row * 2 + col]
                    Column(Modifier.weight(1f).padding(horizontal = 16.dp, vertical = 12.dp)) {
                        Text(l.uppercase(), color = c.textTertiary, fontSize = 11.sp, letterSpacing = 0.6.sp)
                        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(v, color = c.textPrimary, fontSize = 24.sp, fontWeight = FontWeight.Medium)
                            if (s.isNotEmpty()) Text(s, color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(bottom = 4.dp))
                        }
                    }
                    if (col == 0) Box(Modifier.width(1.dp).heightIn(min = 60.dp).background(c.borderSubtle))
                }
            }
            if (row == 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
        }
    }
    val needs = attention(list)
    if (needs.isNotEmpty()) ConsoleCard {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Needs attention", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            Text("${needs.size}", color = c.textTertiary, fontSize = 12.sp)
        }
        needs.forEach { a ->
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
            Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(onClickLabel = "Review ${a.computer.name}") { onOpen(a.computer) }
                .padding(horizontal = 16.dp, vertical = 10.dp).testTag("aciConsoleAttention"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Box(Modifier.size(8.dp).clip(CircleShape).background(if (a.warn) AllternitStatic.StatusWarning else AllternitStatic.StatusError))
                Column(Modifier.weight(1f)) {
                    Text(a.title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                    Text(a.detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                }
                Text("Review", color = c.textPrimary, fontSize = 13.sp)
            }
        }
    }
    list.groupBy { it.group }.forEach { (group, rows) ->
        Column {
            Text("$group · ${rows.size}".uppercase(), color = c.textTertiary, fontSize = 11.sp, letterSpacing = 0.6.sp, modifier = Modifier.padding(bottom = 8.dp))
            ConsoleCard {
                rows.forEachIndexed { i, cp ->
                    if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                    Column(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(onClickLabel = "Open ${cp.name}") { onOpen(cp) }
                        .padding(horizontal = 16.dp, vertical = 12.dp).testTag("aciConsoleRow"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            val dot = when { cp.isOnline -> AllternitStatic.StatusSuccess; cp.isAsleep -> c.textTertiary; else -> AllternitStatic.StatusError }
                            Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
                            Text(cp.name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f), maxLines = 1)
                            Text(cp.provider, color = c.textSecondary, fontSize = 12.sp)
                        }
                        Text(listOfNotNull(cp.os.ifEmpty { null }, cp.region).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                        Row(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
                            MeterLabel("CPU", cp.sample?.cpuPercent, false)
                            MeterLabel("Disk", cp.sample?.diskPercent, true)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun MeterLabel(label: String, percent: Double?, hot: Boolean) {
    val c = AllternitTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, color = c.textTertiary, fontSize = 11.sp)
        DotMeter(percent, hot)
        Text(percent?.let { "${it.toInt()}%" } ?: "—", color = c.textTertiary, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
    }
}
