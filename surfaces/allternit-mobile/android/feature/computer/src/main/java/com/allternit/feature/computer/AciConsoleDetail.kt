package com.allternit.feature.computer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import java.util.Locale
import kotlinx.coroutines.CancellationException
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.doubleOrNull

// ---- Client models (contract HostSnapshot / HostService / HostPort / LogLine / MetricSeries) ----

/** One read of a host or metrics route. Same five outcomes as iOS `ACIConsoleLoad` (loading is the UI's own state). */
internal sealed interface HostRead<out T> {
    data class Ok<T>(val value: T) : HostRead<T>
    /** 409: asleep, not woken. [last] is the last sample the cloud returned, when it returned one. */
    data class Asleep(val last: ConsoleSnapshot?) : HostRead<Nothing>
    /** 404 / 501: the route is not live for this computer yet. */
    data object Unavailable : HostRead<Nothing>
    data class Failed(val message: String) : HostRead<Nothing>
}

internal class ConsoleStreamException(val kind: Kind, message: String) : Exception(message) {
    enum class Kind { Network, Asleep, Unavailable }
}

/** The live read behind the Overview. [sample] carries the three percents. */
internal data class HostSnapshot(
    val sample: ConsoleSnapshot, val cores: Int?, val memoryUsed: Double, val memoryTotal: Double,
    val uptimeSec: Double?, val osName: String?, val osVersion: String?, val arch: String?,
) {
    companion object {
        fun parse(e: JsonElement): HostSnapshot? {
            val o = e as? JsonObject ?: return null
            val sample = ConsoleSnapshot.parse(o) ?: return null
            val os = o.child("os")
            val mem = o.child("memory")
            return HostSnapshot(sample, o.child("cpu").num("cores")?.toInt(), mem.num("usedBytes") ?: 0.0, mem.num("totalBytes") ?: 0.0,
                o.num("uptimeSec"), os.str("name"), os.str("version"), os.str("arch"))
        }
    }
}

internal data class HostService(val name: String, val unit: String, val state: String, val description: String?) {
    companion object {
        fun parse(o: JsonObject): HostService? {
            val unit = o.str("unit") ?: return null
            return HostService(o.str("name") ?: unit, unit, o.str("state") ?: "unknown", o.str("description"))
        }
    }
}

internal data class HostPort(val port: Int, val protocol: String, val process: String?, val exposure: String) {
    companion object {
        fun parse(o: JsonObject): HostPort? {
            val port = o.num("port")?.toInt() ?: return null
            return HostPort(port, o.str("protocol") ?: "tcp", o.str("process"), o.str("exposure") ?: "loopback")
        }
    }
}

internal data class ConsoleLogLine(val level: String, val message: String) {
    val isProblem: Boolean get() = level == "error" || level == "warn"

    companion object {
        /** One `data:` payload (a LogLine JSON object); null when it is not one. */
        fun parse(data: String): ConsoleLogLine? {
            val o = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(data) as? JsonObject }.getOrNull() ?: return null
            return ConsoleLogLine(o.str("level") ?: "info", o.str("message") ?: return null)
        }
    }
}

/** `points` are `[ISO timestamp, value]` pairs, oldest first; only the values are drawn. */
internal data class MetricSeries(val metric: String, val values: List<Double>) {
    companion object {
        fun parse(o: JsonObject): MetricSeries? {
            val metric = o.str("metric") ?: return null
            val values = (o["points"] as? JsonArray).orEmpty().mapNotNull { p -> ((p as? JsonArray)?.getOrNull(1) as? JsonPrimitive)?.doubleOrNull }
            return MetricSeries(metric, values)
        }
    }
}

/** At most [n] bars: samples averaged into equal buckets (iOS `bucket(_:into:)`). */
internal fun consoleBucket(v: List<Double>, n: Int): List<Double> {
    if (v.size <= n) return v
    val size = v.size.toDouble() / n
    return (0 until n).map { i ->
        val from = (i * size).toInt()
        val to = minOf(v.size, maxOf(((i + 1) * size).toInt(), from + 1))
        v.subList(from, to).average()
    }
}

internal fun consoleDuration(sec: Double): String {
    val s = sec.toLong()
    val d = s / 86_400
    val h = (s % 86_400) / 3_600
    val m = (s % 3_600) / 60
    return when {
        d > 0 -> "${d}d ${h}h"
        h > 0 -> "${h}h ${m}m"
        else -> "${maxOf(m, 1)}m"
    }
}

/** Binary units like iOS `ByteCountFormatter(.binary)`. */
internal fun consoleBytes(bytes: Double): String {
    val units = listOf("bytes", "KB", "MB", "GB", "TB")
    var v = bytes
    var i = 0
    while (v >= 1024 && i < units.lastIndex) { v /= 1024; i++ }
    return if (i == 0 || v >= 10) String.format(Locale.getDefault(), "%.0f %s", v, units[i]) else String.format(Locale.getDefault(), "%.1f %s", v, units[i])
}

// ---- Detail sheet ----

private enum class DetailTab(val title: String) { Overview("Overview"), Services("Services"), Usage("Usage"), Ports("Ports"), Terminal("Terminal"), Files("Files"), Access("Access"), Backups("Backups"), Health("Health"), Settings("Settings") }

/**
 * One computer's detail (iOS ACIConsoleDetailView): Overview, Services (+ live logs), Usage, Ports, Terminal and Files (AciConsoleTerminalFiles.kt), then Access,
 * Backups, Health, Settings (AciConsoleManage.kt). A 409 means asleep: last sample + "Asleep, not woken", nothing retried, never woken. 404/501 = not
 * available per tab.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciConsoleDetail(computer: ConsoleComputer, restorePoints: (@Composable (onDone: () -> Unit) -> Unit)? = null, onChanged: () -> Unit = {}, onDone: () -> Unit) {
    val c = AllternitTheme.colors
    var tab by remember { mutableStateOf(DetailTab.Overview) }
    ModalBottomSheet(onDismissRequest = onDone, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.92f).testTag("aciConsoleDetail")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(computer.name, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Box(Modifier.heightIn(min = 44.dp).clickable { onDone() }.padding(start = 12.dp).testTag("aciConsoleDone"), contentAlignment = Alignment.Center) { Text("Done", color = c.textPrimary, fontSize = 15.sp) }
            }
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                DetailTab.entries.forEach { t ->
                    val on = t == tab
                    Box(Modifier.heightIn(min = 44.dp).clip(CircleShape).background(if (on) c.textPrimary.copy(alpha = 0.12f) else c.fillNeutral)
                        .border(1.dp, if (on) c.textPrimary else c.borderSubtle, CircleShape)
                        .selectable(selected = on, role = Role.Tab, onClick = { tab = t }).padding(horizontal = 16.dp).testTag("aciConsoleTab${t.title}"), contentAlignment = Alignment.Center) {
                        Text(t.title, color = if (on) c.textPrimary else c.textSecondary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
                    }
                }
            }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                when (tab) {
                    DetailTab.Overview -> OverviewTab(computer)
                    DetailTab.Services -> ServicesTab(computer)
                    DetailTab.Usage -> UsageTab(computer)
                    DetailTab.Ports -> PortsTab(computer)
                    DetailTab.Terminal -> TerminalTab(computer)
                    DetailTab.Files -> FilesTab(computer, onDone)
                    DetailTab.Access -> AccessTab(computer)
                    DetailTab.Backups -> BackupsTab(computer, restorePoints)
                    DetailTab.Health -> HealthTab(computer)
                    DetailTab.Settings -> SettingsTab(computer, onChanged, onDone)
                }
            }
        }
    }
}

@Composable
internal fun CNote(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp))
}

@Composable
internal fun CCard(content: @Composable ColumnScope.() -> Unit) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
}

@Composable
internal fun CRow(k: String, v: String) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(k, color = c.textSecondary, fontSize = 13.sp)
        Text(v, color = c.textPrimary, fontSize = 14.sp, textAlign = TextAlign.End, modifier = Modifier.weight(1f))
    }
}

/** Loads one host read and draws the five states every tab shares: loading, ready, asleep, not available, error + Try again. */
@Composable
private fun <T> HostTab(
    loading: String, unavailable: String, asleep: String,
    load: suspend () -> HostRead<T>,
    asleepContent: (@Composable (ConsoleSnapshot?) -> Unit)? = null,
    ready: @Composable (T) -> Unit,
) {
    var state by remember { mutableStateOf<HostRead<T>?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    LaunchedEffect(attempt) { state = null; state = load() }
    when (val s = state) {
        null -> CNote(loading)
        HostRead.Unavailable -> CNote(unavailable)
        is HostRead.Failed -> Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            CNote(s.message)
            AciPillButton("Try again", "aciConsoleDetailRetry") { attempt++ }
        }
        is HostRead.Asleep -> if (asleepContent != null) asleepContent(s.last) else CNote(asleep)
        is HostRead.Ok -> ready(s.value)
    }
}

// Overview

@Composable
private fun OverviewTab(computer: ConsoleComputer) {
    HostTab<HostSnapshot>(
        loading = "Loading…", unavailable = "Live details aren't available for this computer yet.", asleep = "",
        load = {
            when (val r = AciApi.consoleRead(computer.id, "host/snapshot")) {
                is HostRead.Ok -> HostSnapshot.parse(r.value)?.let { HostRead.Ok(it) } ?: HostRead.Failed("Couldn't read this. Try again.")
                is HostRead.Asleep -> r
                HostRead.Unavailable -> HostRead.Unavailable
                is HostRead.Failed -> r
            }
        },
        asleepContent = { last ->
            CNote("Asleep, not woken. This is the last sample from while it was awake.")
            (last ?: computer.lastSample)?.let { Meters(it) }
            About(computer, null)
        },
    ) { s ->
        Meters(s.sample)
        About(computer, s)
    }
}

@Composable
private fun Meters(s: ConsoleSnapshot) {
    val c = AllternitTheme.colors
    CCard {
        Text("Right now", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        Meter("CPU", s.cpuPercent); Meter("Memory", s.memoryPercent); Meter("Disk", s.diskPercent)
    }
}

@Composable
private fun About(computer: ConsoleComputer, s: HostSnapshot?) {
    val c = AllternitTheme.colors
    CCard {
        Text("About", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        CRow("System", listOfNotNull(s?.osName, s?.osVersion).joinToString(" ").ifEmpty { computer.os })
        s?.arch?.let { CRow("Architecture", it) }
        s?.cores?.let { CRow("Processor", "$it cores") }
        if (s != null) CRow("Memory", "${consoleBytes(s.memoryUsed)} of ${consoleBytes(s.memoryTotal)}")
        CRow("Provider", listOfNotNull(computer.provider, computer.region).filter { it.isNotEmpty() }.joinToString(" · "))
        computer.address?.takeIf { it.isNotEmpty() }?.let { CRow("Address", it) }
        s?.uptimeSec?.let { CRow("Uptime", consoleDuration(it)) }
    }
}

// Services + logs

@Composable
private fun ServicesTab(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    var logsFor by remember { mutableStateOf<HostService?>(null) }
    HostTab<List<HostService>>(
        loading = "Loading services…", unavailable = "Services aren't available for this computer yet.",
        asleep = "Asleep, not woken. Services show when it is awake.",
        load = {
            when (val r = AciApi.consoleRead(computer.id, "host/services")) {
                is HostRead.Ok -> HostRead.Ok(r.value.under("services").mapNotNull { HostService.parse(it) })
                is HostRead.Asleep -> r
                HostRead.Unavailable -> HostRead.Unavailable
                is HostRead.Failed -> r
            }
        },
    ) { rows ->
        if (rows.isEmpty()) CNote("No services reported.") else rows.forEach { svc ->
            val shape = RoundedCornerShape(14.dp)
            Row(Modifier.fillMaxWidth().clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape)
                .clickable(onClickLabel = "Shows live logs") { logsFor = svc }.padding(14.dp).testTag("aciConsoleService"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(8.dp).clip(CircleShape).background(when (svc.state) {
                    "running" -> AllternitStatic.StatusSuccess
                    "failed" -> AllternitStatic.StatusWarning
                    else -> c.textTertiary
                }))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(svc.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                    Text(svc.description ?: svc.unit, color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Text(svc.state.replaceFirstChar { it.uppercase() }, color = c.textSecondary, fontSize = 13.sp)
                Text("›", color = c.textTertiary, fontSize = 16.sp)
            }
        }
    }
    logsFor?.let { LogsSheet(computer, it) { logsFor = null } }
}

/** `{<key>:[...]}` or a bare array. */
private fun JsonElement.under(key: String): List<JsonObject> = when (this) {
    is JsonArray -> mapNotNull { it as? JsonObject }
    is JsonObject -> list(key)
    else -> emptyList()
}

/**
 * Live log stream for one unit (iOS ACIConsoleLogsView). Streams only while the sheet is open AND the app is in
 * the foreground: leaving the foreground cancels the call, returning restarts it. Last 300 lines in memory,
 * nothing stored, no actions (no restart / stop).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LogsSheet(computer: ConsoleComputer, service: HostService, onDone: () -> Unit) {
    val c = AllternitTheme.colors
    var lines by remember { mutableStateOf(emptyList<ConsoleLogLine>()) }
    var note by remember { mutableStateOf<String?>(null) }
    var errorsOnly by remember { mutableStateOf(false) }
    val owner = LocalLifecycleOwner.current
    var active by remember { mutableStateOf(owner.lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED)) }
    DisposableEffect(owner) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) active = true else if (event == Lifecycle.Event.ON_PAUSE) active = false
        }
        owner.lifecycle.addObserver(observer)
        onDispose { owner.lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(active) {
        if (!active) return@LaunchedEffect
        note = null
        try {
            AciApi.consoleLogStream(computer.id, service.unit).collect { line -> lines = (lines + line).takeLast(300) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: ConsoleStreamException) {
            note = when (e.kind) {
                ConsoleStreamException.Kind.Unavailable -> "Logs aren't available for this computer yet."
                ConsoleStreamException.Kind.Asleep -> "Asleep, not woken."
                ConsoleStreamException.Kind.Network -> e.message ?: "The log stream stopped."
            }
        }
    }
    val shown = if (errorsOnly) lines.filter { it.isProblem } else lines
    val listState = rememberLazyListState()
    LaunchedEffect(shown.size) { if (shown.isNotEmpty()) listState.scrollToItem(shown.lastIndex) }
    ModalBottomSheet(onDismissRequest = onDone, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.92f).testTag("aciConsoleLogs")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Logs · ${service.name}", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Box(Modifier.heightIn(min = 44.dp).clickable { onDone() }.padding(start = 12.dp).testTag("aciConsoleLogsDone"), contentAlignment = Alignment.Center) { Text("Done", color = c.textPrimary, fontSize = 15.sp) }
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Errors only", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                Switch(checked = errorsOnly, onCheckedChange = { errorsOnly = it }, modifier = Modifier.testTag("aciConsoleErrorsOnly"))
            }
            Box(Modifier.weight(1f).fillMaxWidth().background(c.fillNeutral)) {
                when {
                    note != null -> CNote(note!!)
                    lines.isEmpty() -> CNote("Waiting for log lines…")
                    else -> LazyColumn(state = listState, contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        itemsIndexed(shown) { _, l ->
                            Text(l.message, fontFamily = FontFamily.Monospace, fontSize = 12.sp, color = when (l.level) {
                                "error" -> AllternitStatic.StatusWarning
                                "warn" -> AllternitStatic.StatusWarning.copy(alpha = 0.8f)
                                else -> c.textPrimary
                            })
                        }
                    }
                }
            }
        }
    }
}

// Usage

@Composable
private fun UsageTab(computer: ConsoleComputer) {
    HostTab<List<MetricSeries>>(
        loading = "Loading usage…", unavailable = "Usage history isn't available for this computer yet.",
        asleep = "Asleep, not woken. History shows what it recorded while awake.",
        load = {
            when (val r = AciApi.consoleRead(computer.id, "metrics", mapOf("window" to "24h"))) {
                is HostRead.Ok -> HostRead.Ok(r.value.under("series").mapNotNull { MetricSeries.parse(it) })
                is HostRead.Asleep -> r
                HostRead.Unavailable -> HostRead.Unavailable
                is HostRead.Failed -> r
            }
        },
    ) { series ->
        if (series.all { it.values.isEmpty() }) CNote("No usage recorded in the last 24 hours.") else {
            UsageChart("CPU", "cpu", series, true)
            UsageChart("Memory", "memory", series, true)
            UsageChart("Disk", "disk", series, true)
            UsageChart("Network in", "net_rx", series, false)
            UsageChart("Network out", "net_tx", series, false)
        }
    }
}

/** 48 neutral grey bars (samples averaged into buckets), amber at 85% or more for percent charts, latest value at the right of the title. */
@Composable
private fun UsageChart(title: String, metric: String, all: List<MetricSeries>, percent: Boolean) {
    val values = all.firstOrNull { it.metric == metric }?.values?.takeIf { it.isNotEmpty() } ?: return
    val c = AllternitTheme.colors
    val top = if (percent) 100.0 else maxOf(values.max(), 1.0)
    val latest = values.last()
    CCard {
        Row(Modifier.fillMaxWidth()) {
            Text("$title · 24 hours", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            Text(if (percent) "${latest.toInt()}%" else "${consoleBytes(latest)}/s", color = c.textSecondary, fontSize = 13.sp)
        }
        Row(Modifier.fillMaxWidth().height(64.dp).clearAndSetSemantics {
            contentDescription = "$title, last 24 hours, now ${latest.toInt()}${if (percent) " percent" else ""}"
        }.testTag("aciConsoleChart$metric"), horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.Bottom) {
            consoleBucket(values, 48).forEach { v ->
                Box(Modifier.weight(1f).height((64 * (v / top).coerceIn(0.0, 1.0)).toFloat().coerceAtLeast(2f).dp).clip(RoundedCornerShape(1.5.dp))
                    .background(if (percent && v >= 85) AllternitStatic.StatusWarning else c.textPrimary.copy(alpha = 0.45f)))
            }
        }
    }
}

// Ports

@Composable
private fun PortsTab(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    HostTab<List<HostPort>>(
        loading = "Loading ports…", unavailable = "Ports aren't available for this computer yet.",
        asleep = "Asleep, not woken. Ports show when it is awake.",
        load = {
            when (val r = AciApi.consoleRead(computer.id, "host/ports")) {
                is HostRead.Ok -> HostRead.Ok(r.value.under("ports").mapNotNull { HostPort.parse(it) })
                is HostRead.Asleep -> r
                HostRead.Unavailable -> HostRead.Unavailable
                is HostRead.Failed -> r
            }
        },
    ) { rows ->
        if (rows.isEmpty()) CNote("No listening ports.") else rows.sortedBy { it.port }.forEach { p ->
            val shape = RoundedCornerShape(14.dp)
            Row(Modifier.fillMaxWidth().clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape).padding(14.dp).testTag("aciConsolePort"),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("${p.port}", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, fontFamily = FontFamily.Monospace)
                Text(p.protocol.uppercase(), color = c.textTertiary, fontSize = 11.sp)
                Text(p.process ?: "Unknown program", color = c.textSecondary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text(when (p.exposure) {
                    "public" -> "Open to the internet"
                    "lan" -> "Local network"
                    else -> "This computer only"
                }, color = if (p.exposure == "public") AllternitStatic.StatusWarning else c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}
