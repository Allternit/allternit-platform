package com.allternit.feature.computer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonObject

/** Shown when GET /api/v1/computers/console answers 404 or 501 (same words as iOS). */
internal const val CONSOLE_UNAVAILABLE = "Computers console isn't available on your account yet."

/** Last health sample (contract HostSnapshot subset), as percents. */
internal data class ConsoleSnapshot(val cpuPercent: Double, val memoryPercent: Double, val diskPercent: Double) {
    companion object {
        /** Contract HostSnapshot subset to percents; null when cpu or memory is missing (same rule as iOS decoding). */
        fun parse(s: JsonObject?): ConsoleSnapshot? {
            val cpu = s.child("cpu").num("percent") ?: return null
            val mem = s.child("memory")
            val memTotal = mem.num("totalBytes") ?: return null
            val memUsed = mem.num("usedBytes") ?: return null
            val disks = s.list("disk").map { d -> d.num("totalBytes")?.let { t -> if (t > 0) (d.num("usedBytes") ?: 0.0) / t * 100 else 0.0 } ?: 0.0 }
            return ConsoleSnapshot(cpu, if (memTotal > 0) memUsed / memTotal * 100 else 0.0, disks.maxOrNull() ?: 0.0)
        }
    }
}

/** One row of `GET /api/v1/computers/console` (contract ConsoleComputer). Same rules as iOS `ConsoleComputer`. */
internal data class ConsoleComputer(
    val id: String, val name: String, val provider: String, val region: String?, val os: String,
    val status: String, val lastSample: ConsoleSnapshot?, val servicesCount: Int?, val address: String? = null,
) {
    // online, offline, asleep, starting, stopped, suspended, error
    val isOnline: Boolean get() = status.lowercase() !in setOf("offline", "asleep", "stopped")
    val isAsleep: Boolean get() = status.lowercase() == "asleep"

    /** Plain reasons this computer needs a look: offline, or CPU / memory / disk at 90% or more. */
    val attention: List<String>
        get() = buildList {
            if (status.lowercase() == "offline") add("Offline")
            lastSample?.let {
                if (it.cpuPercent >= 90) add("CPU at ${it.cpuPercent.toInt()}%")
                if (it.memoryPercent >= 90) add("Memory at ${it.memoryPercent.toInt()}%")
                if (it.diskPercent >= 90) add("Disk ${it.diskPercent.toInt()}% full")
            }
        }

    companion object {
        /** Null for a malformed row (skipped, like iOS ACILossy). */
        fun parse(o: JsonObject): ConsoleComputer? {
            val id = o.str("id") ?: return null
            val name = o.str("name") ?: return null
            val status = o.str("status") ?: return null
            val sample = ConsoleSnapshot.parse(o.child("lastSample"))
            return ConsoleComputer(id, name, o.str("provider").orEmpty(), o.str("region"), o.str("os").orEmpty(), status, sample, o.num("servicesCount")?.toInt(), o.str("address"))
        }
    }
}

private sealed interface ConsolePhase {
    data object Loading : ConsolePhase
    data object Unavailable : ConsolePhase
    data class Failed(val message: String) : ConsolePhase
    data class Ready(val rows: List<ConsoleComputer>) : ConsolePhase
}

/**
 * Computers console list (iOS ACIConsoleSection), shown under the Add / View buttons when the account flag
 * `computers_console` is on. States: loading, empty, error (Try again), not available. Asleep computers show
 * their last sample and are never woken. Neutral grey meters; amber at 85% or more.
 */
@Composable
internal fun AciConsoleSection() {
    var phase by remember { mutableStateOf<ConsolePhase>(ConsolePhase.Loading) }
    var attempt by remember { mutableIntStateOf(0) }
    var selected by remember { mutableStateOf<ConsoleComputer?>(null) }
    LaunchedEffect(attempt) {
        phase = ConsolePhase.Loading
        phase = when (val r = AciApi.console()) {
            is AciApi.Result.Ok -> ConsolePhase.Ready(r.value)
            is AciApi.Result.Failed -> if (r.message == CONSOLE_UNAVAILABLE) ConsolePhase.Unavailable else ConsolePhase.Failed(r.message)
        }
    }
    selected?.let { AciConsoleDetail(it) { selected = null } }
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(bottom = 8.dp).testTag("aciConsole"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        when (val p = phase) {
            ConsolePhase.Loading -> Text("Loading computers…", color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(vertical = 32.dp))
            ConsolePhase.Unavailable -> Note(CONSOLE_UNAVAILABLE)
            is ConsolePhase.Failed -> Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Note(p.message)
                AciPillButton("Try again", "aciConsoleRetry") { attempt++ }
            }
            is ConsolePhase.Ready -> if (p.rows.isEmpty()) Note("No computers yet. Add one above and it shows its health here.") else {
                Text("${p.rows.size} computers · ${p.rows.count { it.isOnline }} online", color = c.textSecondary, fontSize = 13.sp)
                val flagged = p.rows.filter { it.attention.isNotEmpty() }
                if (flagged.isNotEmpty()) {
                    Text("Needs attention", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    flagged.forEach { row ->
                        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp).testTag("aciConsoleAttention"),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Box(Modifier.size(8.dp).clip(CircleShape).background(AllternitStatic.StatusWarning))
                            Text("${row.name}: ${row.attention.joinToString(", ")}", color = c.textPrimary, fontSize = 14.sp)
                        }
                    }
                }
                p.rows.forEach { ConsoleCard(it) { selected = it } }
            }
        }
    }
}

@Composable
private fun Note(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp))
}

@Composable
private fun ConsoleCard(cp: ConsoleComputer, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape)
        .clickable(onClickLabel = "Opens details") { onOpen() }.padding(16.dp).testTag("aciConsoleRow"),
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(if (cp.isOnline) AllternitStatic.StatusSuccess else c.textTertiary))
            Text(cp.name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            Text(if (cp.isAsleep) "Asleep" else if (cp.isOnline) "Online" else "Offline", color = c.textSecondary, fontSize = 13.sp)
        }
        Text(listOfNotNull(cp.provider, cp.region, cp.os).filter { it.isNotEmpty() }.joinToString(" · "), color = c.textSecondary, fontSize = 13.sp)
        val s = cp.lastSample
        if (s != null) {
            Meter("CPU", s.cpuPercent)
            Meter("Memory", s.memoryPercent)
            Meter("Disk", s.diskPercent)
            if (cp.isAsleep) Text("Last sample while awake. We never wake it to read it.", color = c.textTertiary, fontSize = 12.sp)
        } else Text("No health sample yet", color = c.textTertiary, fontSize = 13.sp)
        cp.servicesCount?.let { Text("$it services", color = c.textSecondary, fontSize = 13.sp) }
    }
}

/** Neutral grey capsule, amber at 85% or more, percent at the right. */
@Composable
internal fun Meter(label: String, percent: Double) {
    val c = AllternitTheme.colors
    val p = percent.coerceIn(0.0, 100.0)
    Row(Modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = "$label ${p.toInt()} percent" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(label, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.width(56.dp))
        Box(Modifier.weight(1f).height(6.dp).clip(CircleShape).background(c.textPrimary.copy(alpha = 0.1f))) {
            Box(Modifier.fillMaxWidth((p / 100).toFloat()).height(6.dp).clip(CircleShape)
                .background(if (p >= 85) AllternitStatic.StatusWarning else c.textPrimary.copy(alpha = 0.5f)))
        }
        Text("${p.toInt()}%", color = c.textPrimary, fontSize = 13.sp, textAlign = TextAlign.End, modifier = Modifier.width(40.dp))
    }
}
