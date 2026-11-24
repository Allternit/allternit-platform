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
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.code.PtySession
import com.allternit.feature.code.PtyState
import com.allternit.feature.code.PtyTerminalView
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

// Computers console step 5.4 (iOS ACIConsoleTerminalFiles.swift, PR #2770): Terminal and Files tabs of the detail sheet.

/** One row of `GET /api/v1/computers/snippets` (`{snippets:[...]}` or a bare array). */
internal data class ConsoleSnippet(val id: String, val name: String, val command: String, val description: String?) {
    companion object {
        fun parse(o: JsonObject): ConsoleSnippet? =
            ConsoleSnippet(o.str("id") ?: return null, o.str("name") ?: return null, o.str("command") ?: return null, o.str("description"))
    }
}

/** One mount of the snapshot's `disk` list. */
internal data class DiskMount(val mount: String, val usedBytes: Double, val totalBytes: Double) {
    val percent: Double get() = if (totalBytes > 0) (usedBytes / totalBytes * 100).coerceIn(0.0, 100.0) else 0.0

    companion object {
        fun parseAll(snapshot: JsonElement): List<DiskMount> =
            (snapshot as? JsonObject)?.list("disk").orEmpty().mapNotNull { d ->
                val total = d.num("totalBytes") ?: return@mapNotNull null
                DiskMount(d.str("mount") ?: "/", d.num("usedBytes") ?: 0.0, total)
            }
    }
}

/** A clean-up is a finding whose suggested action is `space.clean` (ids apt-cache, dnf-cache, journal-vacuum, docker-prune, brew-cleanup). */
internal fun consoleCleanups(findings: List<HostFinding>): List<HostFinding> = findings.filter { it.suggestedAction?.type == "space.clean" }

// ---- Terminal ----

/**
 * The Code shell's PtySession + terminal view over the relay, aimed at `row.runtimeId ?: row.id`. Foreground only:
 * leaving the app or the tab drops the socket (the shell keeps running on the computer). An asleep or offline computer
 * never gets a terminal, because opening one would wake it. Snippets run through the approval flow.
 */
@Composable
internal fun TerminalTab(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val backend = remember { ChatBackends.create(context) }
    var session by remember { mutableStateOf<PtySession?>(null) }
    fun close() { session?.detach(); session = null }
    fun open() { session = PtySession(backend, scope, computer.relayId).also { it.start(null) } }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, e -> if (e == Lifecycle.Event.ON_STOP) close() }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer); close() }
    }
    val live = computer.isOnline && !computer.isAsleep
    CCard {
        CTitle("Terminal")
        val s = session
        when {
            !live -> CText(if (computer.isAsleep) "Asleep, not woken. A terminal would wake it, so it stays closed. Open one after it wakes." else "Offline. A terminal opens when the computer is online.")
            s == null -> {
                CText("A shell on ${computer.name}. Closing it leaves the shell running on the computer.")
                AciPillButton("Open terminal", "aciConsoleTerminalOpen") { open() }
            }
            else -> {
                Box(Modifier.fillMaxWidth().height(380.dp).clip(RoundedCornerShape(8.dp)).testTag("aciConsoleTerminal")) { PtyTerminalView(s, Modifier.fillMaxWidth().height(380.dp)) }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(when (s.state) {
                        PtyState.Starting -> "Connecting…"
                        PtyState.Live -> "Connected"
                        PtyState.Ended -> "The shell ended."
                        PtyState.Offline -> "Terminal disconnected."
                        PtyState.Failed -> "Terminal disconnected."
                    }, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    AciPillButton("Close", "aciConsoleTerminalClose") { close() }
                }
            }
        }
    }
    if (live) SnippetsSection(computer)
}

@Composable
private fun SnippetsSection(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    val snippets = rememberHostLoad("snippets") { AciApi.consoleSnippets() }
    var running by remember { mutableStateOf<ConsoleSnippet?>(null) }
    Text("Snippets", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
    HostNotReady(snippets, "Snippets aren't available on your account yet.", "", "Loading snippets…")
    (snippets.state as? HostRead.Ok)?.value?.let { list ->
        if (list.isEmpty()) CNote("No snippets yet. Saved commands show here.") else list.forEach { sn ->
            CCard {
                Text(sn.name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                sn.description?.takeIf { it.isNotBlank() }?.let { CText(it) }
                Text(sn.command, color = c.textPrimary, fontSize = 13.sp, fontFamily = FontFamily.Monospace,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.textPrimary.copy(alpha = 0.08f)).padding(horizontal = 12.dp, vertical = 8.dp))
                AciPillButton("Run on ${computer.name}", "aciConsoleSnippetRun", label = "Run ${sn.name} on ${computer.name}") { running = sn }
            }
        }
    }
    running?.let { sn ->
        // The same Hold-to-approve sheet as a fix: POST :id/approvals {type:"snippet.run", target:<snippetId>}, then POST host/actions.
        FixSheet(computer, HostFinding(sn.id, "info", sn.name, sn.description.orEmpty(), HostAction("snippet.run", sn.id, sn.command)),
            onDone = { running = null }, onChanged = {}, heading = "Snippet", navTitle = "Approve a snippet")
    }
}

// ---- Files ----

private enum class FilesSegment(val title: String) { Browse("Browse"), Space("Space") }

/** Browse (the Files drive of this computer) | Space (disk bars + clean-ups). No Trash: the Files drive has no Trash client. */
@Composable
internal fun FilesTab(computer: ConsoleComputer, onClose: () -> Unit, onOpenSettings: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    var segment by remember { mutableStateOf(FilesSegment.Browse) }
    Row(Modifier.fillMaxWidth().clip(CircleShape).background(c.fillNeutral).border(1.dp, c.borderSubtle, CircleShape).padding(3.dp), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        FilesSegment.entries.forEach { seg ->
            val on = seg == segment
            Box(Modifier.weight(1f).heightIn(min = 44.dp).clip(CircleShape).background(if (on) c.textPrimary.copy(alpha = 0.12f) else androidx.compose.ui.graphics.Color.Transparent)
                .selectable(selected = on, role = Role.Tab, onClick = { segment = seg }).testTag("aciConsoleFiles${seg.title}"), contentAlignment = Alignment.Center) {
                Text(seg.title, color = if (on) c.textPrimary else c.textSecondary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
    }
    when (segment) {
        FilesSegment.Browse -> {
            // The Files drive client (:feature:computer AciFiles) keyed on the runtime device behind this row.
            val device = remember(computer.relayId, computer.name, computer.status) { AciDevice(computer.relayId, computer.name, if (computer.isOnline) "online" else "offline", null, null) }
            Box(Modifier.fillMaxWidth().height(520.dp).testTag("aciConsoleFilesBrowse")) { AciComputerFilesScreen(device, onClose = onClose, embedded = true,
                diskRoots = computer.wholeDisk?.takeIf { it.showsRootsSwitcher }?.roots.orEmpty(), onOpenSettings = onOpenSettings) }
        }
        FilesSegment.Space -> SpaceView(computer)
    }
}

@Composable
private fun SpaceView(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    var attempt by remember { mutableIntStateOf(0) }
    var snap by remember { mutableStateOf<HostRead<JsonElement>?>(null) }
    LaunchedEffect(attempt) { snap = null; snap = AciApi.consoleRead(computer.id, "host/snapshot") }
    val findings = rememberHostLoad(computer.id) { readRows(computer.id, "host/findings", "findings", HostFinding::parse) }
    var cleaning by remember { mutableStateOf<HostFinding?>(null) }
    when (val s = snap) {
        null -> CNote("Loading…")
        HostRead.Unavailable -> CNote("Disk use isn't available for this computer yet.")
        is HostRead.Failed -> Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            CNote(s.message)
            AciPillButton("Try again", "aciConsoleSpaceRetry") { attempt++ }
        }
        is HostRead.Asleep -> {
            CNote("Asleep, not woken. This is the last sample from while it was awake.")
            (s.last ?: computer.lastSample)?.let { last -> CCard { CTitle("Disk use"); DiskBar("All disks", last.diskPercent, null) } }
        }
        is HostRead.Ok -> {
            val mounts = DiskMount.parseAll(s.value)
            CCard {
                CTitle("Disk use")
                if (mounts.isEmpty()) CText("This computer didn't report any disks.") else mounts.forEach { m ->
                    DiskBar(m.mount, m.percent, "${consoleBytes(m.usedBytes)} used of ${consoleBytes(m.totalBytes)}")
                }
            }
            Text("Clean up", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            HostNotReady(findings, "Clean-ups aren't available for this computer yet.", "Asleep, not woken. Clean-ups show when it is awake.")
            (findings.state as? HostRead.Ok)?.value?.let { all ->
                val items = consoleCleanups(all)
                if (items.isEmpty()) CNote("Nothing to clean up right now.") else items.forEach { f ->
                    CCard {
                        Text(f.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                        CText(f.detail)
                        AciPillButton("Clean", "aciConsoleClean", label = "Clean: ${f.title}") { cleaning = f }
                    }
                }
            }
        }
    }
    cleaning?.let { FixSheet(computer, it, onDone = { cleaning = null }, onChanged = { findings.reload(); attempt++ }, heading = "What this frees", navTitle = "Approve a clean-up") }
}

/** Grey capsule (neutral scale, no tan, no colour): the used share of one disk. */
@Composable
private fun DiskBar(label: String, percent: Double, detail: String?) {
    val c = AllternitTheme.colors
    val p = percent.coerceIn(0.0, 100.0)
    Column(Modifier.fillMaxWidth().clearAndSetSemantics { contentDescription = "$label ${p.toInt()} percent used" + (detail?.let { ", $it" } ?: "") }, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(label, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.weight(1f))
            Text("${p.toInt()}%", color = c.textPrimary, fontSize = 13.sp)
        }
        Box(Modifier.fillMaxWidth().height(8.dp).clip(CircleShape).background(c.textPrimary.copy(alpha = 0.1f))) {
            Box(Modifier.fillMaxWidth((p / 100).toFloat()).height(8.dp).clip(CircleShape).background(c.textPrimary.copy(alpha = 0.5f)))
        }
        detail?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
    }
}
