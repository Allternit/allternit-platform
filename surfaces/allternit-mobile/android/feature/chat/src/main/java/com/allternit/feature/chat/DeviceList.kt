package com.allternit.feature.chat

import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
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
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

/*
 * The shared device list (Start anywhere D1; web components/devices/DeviceList.tsx, iOS shared list):
 * one row per computer, used by the + sheet's Computer page, the chip selector's Computer row and Code's
 * Computer screen. Row copy matches iOS and web exactly.
 */

/** Where a computer is right now, which picks the row's icon and line. */
enum class DeviceState { Online, Cloud, Asleep }

fun RelayDevice.state(): DeviceState = when {
    isCloud -> DeviceState.Cloud
    isOnline -> DeviceState.Online
    else -> DeviceState.Asleep
}

/** "Online now" | "Asleep · last seen 9 hours ago" | "Wakes when you send" (web useDeviceList). */
fun RelayDevice.subtitle(now: Long = System.currentTimeMillis()): String = when (state()) {
    DeviceState.Online -> "Online now"
    DeviceState.Cloud -> "Wakes when you send"
    DeviceState.Asleep -> "Asleep · last seen ${lastSeenPhrase(lastSeenAt, now)}"
}

internal fun lastSeenPhrase(iso: String?, now: Long): String {
    val at = iso?.let { runCatching { java.time.Instant.parse(it).toEpochMilli() }.getOrNull() } ?: return "a while ago"
    if (now - at < 60_000L) return "just now"
    return android.text.format.DateUtils.getRelativeTimeSpanString(at, now, android.text.format.DateUtils.MINUTE_IN_MILLIS).toString().lowercase()
}

/** Order: online, then cloud, then asleep (web order). */
fun List<RelayDevice>.sortedForPicker(): List<RelayDevice> = sortedBy { it.state().ordinal }

private fun DeviceState.icon(): Int = when (this) {
    DeviceState.Cloud -> DesignR.drawable.ph_cloud
    else -> DesignR.drawable.ph_desktop
}

/** One computer row: icon, name, the state line, a check when picked. */
@Composable
fun DeviceRow(device: RelayDevice, selected: Boolean, testTag: String = "deviceRow-${device.id}", onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val state = device.state()
    PickRow(
        icon = state.icon(),
        iconAlpha = if (state == DeviceState.Asleep) 0.5f else 1f,
        title = device.name.ifEmpty { "Computer" },
        sub = device.subtitle(),
        checked = selected,
        testTag = testTag,
        onClick = onClick,
    )
}

/** The list rows' shared look (web SheetRow): white + neutral fill, never tan. */
@Composable
fun PickRow(
    title: String,
    sub: String? = null,
    icon: Int? = null,
    iconAlpha: Float = 1f,
    value: String? = null,
    checked: Boolean = false,
    chevron: Boolean = false,
    testTag: String? = null,
    onClick: () -> Unit,
) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth()
            .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
            .clickable(role = Role.Button, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = listOfNotNull(title, sub, value, if (checked) "selected" else null).joinToString(", ") }
            .heightIn(min = 48.dp).padding(horizontal = 14.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (icon != null) Icon(painterResource(icon), null, tint = c.textPrimary.copy(alpha = iconAlpha), modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (sub != null) Text(sub, color = c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (value != null) Text(value, color = c.textSecondary, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 4.dp))
        if (checked) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.textPrimary, modifier = Modifier.size(14.dp))
        if (chevron) Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textSecondary.copy(alpha = 0.7f), modifier = Modifier.size(12.dp))
    }
}

/** A rounded group of rows on the neutral fill, with hairlines between rows. */
@Composable
fun PickGroup(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(AllternitTheme.colors.fillNeutral)) { content() }
}

@Composable
fun PickDivider() {
    Box(Modifier.fillMaxWidth().padding(start = 46.dp).height(1.dp).background(AllternitTheme.colors.borderSubtle))
}

@Composable
fun PickFoot(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 4.dp))
}

/**
 * The Computer page body: every computer, "No computer", and the asleep offer. Picking an asleep computer
 * offers "Use a cloud computer" (same git folder) or "Pick <name> anyway". With no cloud computer on the
 * account, "Use a cloud computer" opens the compute gate (subscribe / link) instead.
 * [onPick] gets the device (null = No computer), whether it was asleep, and the folder to use there.
 */
@Composable
fun DeviceList(
    backend: ChatBackend,
    selectedId: String?,
    folder: StartFolderRef?,
    onPick: (device: RelayDevice?, asleep: Boolean, folder: StartFolderRef?) -> Unit,
) {
    val c = AllternitTheme.colors
    var tick by remember { mutableIntStateOf(0) }
    var devices by remember { mutableStateOf<List<RelayDevice>?>(null) }
    var failed by remember { mutableStateOf(false) }
    var snapshots by remember { mutableStateOf<List<StartSnapshot>>(emptyList()) }
    var asleepId by remember { mutableStateOf<String?>(null) }
    var gate by remember { mutableStateOf(false) }
    LaunchedEffect(tick) {
        val list = backend.relay.listDevices()
        failed = list == null
        devices = list?.sortedForPicker()
        snapshots = SessionStartClient.snapshots(backend)
    }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.testTag("deviceList")) {
        val list = devices
        PickGroup {
            when {
                list == null && !failed -> Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.Center) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                }
                failed -> PickRow("Couldn't load your computers", sub = "Tap to try again", testTag = "deviceListRetry") { devices = null; failed = false; tick++ }
                else -> list.orEmpty().forEach { d ->
                    DeviceRow(d, selected = selectedId == d.id) {
                        if (d.state() == DeviceState.Asleep) asleepId = d.id else { asleepId = null; onPick(d, false, folder) }
                    }
                    PickDivider()
                }
            }
            PickRow("No computer", sub = "Plan chats work without one", checked = selectedId == null, testTag = "deviceRow-none") {
                asleepId = null; onPick(null, false, null)
            }
        }
        val asleep = list?.firstOrNull { it.id == asleepId }
        if (asleep != null) {
            val name = asleep.name.ifEmpty { "Computer" }
            val snap = snapshots.firstOrNull { it.runtimeId == asleep.id }
            val path = (folder as? StartFolderRef.Path)?.path
            val snapFolder = snap?.folders?.let { fs -> fs.firstOrNull { it.path == path } ?: fs.firstOrNull() }
            val folderName = snapFolder?.name ?: folder?.label ?: "this folder"
            val git = snapFolder?.git
            val cloud = list.orEmpty().firstOrNull { it.isCloud }
            PickGroup {
                val line = when {
                    snap == null -> "$name is asleep. $name has not shared its folders, so a cloud computer cannot open $folderName."
                    git == null -> "$name is asleep. $folderName has no git remote, so a cloud computer cannot open it."
                    else -> "$name is asleep. Run in $folderName on a cloud computer instead?"
                }
                Text(line, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 12.dp).testTag("deviceAsleepOffer"))
                if (git != null) {
                    PickDivider()
                    PickRow("Use a cloud computer", chevron = true, testTag = "deviceUseCloud") {
                        if (cloud == null) gate = true
                        else { asleepId = null; onPick(cloud, false, StartFolderRef.Git(git.remote, git.branch, git.head)) }
                    }
                }
                PickDivider()
                PickRow("Pick $name anyway", sub = "Runs when it wakes", testTag = "devicePickAnyway") {
                    asleepId = null; onPick(asleep, true, folder)
                }
            }
        } else {
            PickFoot("Your next message runs on the computer you pick: a real session with your files and tools.")
        }
        // No cloud computer on the account: the compute gate (ComputeGate.kt) is the way to get one.
        if (gate) ComputeGateCard(ComputeGateReason.NoComputer, backend, onConnected = { gate = false; tick++ }, onDismiss = { gate = false })
    }
}

/**
 * Loads the picked computer's start options once (folders + per-runtime mode defaults) into [PendingStart],
 * so the Folder page lists its recent folders and the Permission row shows "<mode> · default".
 */
@Composable
fun LoadPendingOptions(backend: ChatBackend, runtime: String) {
    val id = PendingStart.runtimeId
    LaunchedEffect(id, runtime) {
        if (id == null) return@LaunchedEffect
        val opts = PendingStart.options ?: (SessionStartClient.options(backend, id) as? StartResult.Ok)?.value
        if (PendingStart.runtimeId != id) return@LaunchedEffect
        PendingStart.options = opts
        PendingStart.defaultMode = opts?.modes?.get(contractRuntime(runtime))?.default
    }
}

/** "<default label> · default" with no explicit pick, else the picked mode's label (web ComposerPlusSheet). */
fun permissionValue(): String {
    val def = PendingStart.defaultMode
    val shown = PendingStart.mode ?: def ?: StartMode.Default
    return if (shown == def || (PendingStart.mode == null && def == null)) "${shown.label} · default" else shown.label
}

/** The Folder page: the picked computer's recent folders, then "Other folder…" (a typed path). */
@Composable
fun FolderPage(backend: ChatBackend, runtime: String, onDone: () -> Unit) {
    val c = AllternitTheme.colors
    LoadPendingOptions(backend, runtime)
    var other by remember { mutableStateOf(false) }
    var typed by remember { mutableStateOf("") }
    val opts = PendingStart.options
    val current = (PendingStart.folder as? StartFolderRef.Path)?.path
    Column(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.testTag("folderPage")) {
        PickGroup {
            if (PendingStart.runtimeId != null && opts == null) {
                Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.Center) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                }
                PickDivider()
            }
            opts?.folders.orEmpty().forEach { f ->
                PickRow(f.name, sub = f.path.substringBeforeLast('/').replaceFirst(Regex("^/Users/[^/]+"), "~"), icon = DesignR.drawable.ph_folder,
                    checked = current == f.path, testTag = "folderRow-${f.name}") {
                    PendingStart.folder = StartFolderRef.Path(f.path); onDone()
                }
                PickDivider()
            }
            PickRow("Other folder…", chevron = !other, testTag = "folderOther") { other = !other }
            if (other) {
                Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(c.bgPrimary).padding(10.dp)) {
                        if (typed.isEmpty()) Text("/Users/you/code/project", color = c.textTertiary, fontSize = 14.sp)
                        androidx.compose.foundation.text.BasicTextField(
                            value = typed, onValueChange = { typed = it }, singleLine = true,
                            textStyle = androidx.compose.ui.text.TextStyle(color = c.textPrimary, fontSize = 14.sp),
                            cursorBrush = androidx.compose.ui.graphics.SolidColor(c.textPrimary),
                            modifier = Modifier.fillMaxWidth().testTag("folderOtherField"),
                        )
                    }
                    val ok = typed.trim().startsWith("/") || typed.trim().startsWith("~")
                    Text("Use", color = if (ok) c.onAccent else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(if (ok) c.accentPrimary else c.borderSubtle)
                            .clickable(enabled = ok, role = Role.Button) { PendingStart.folder = StartFolderRef.Path(typed.trim()); onDone() }
                            .padding(horizontal = 14.dp, vertical = 10.dp))
                }
            }
        }
        PickFoot("Recent folders on ${PendingStart.deviceName ?: "this computer"}.")
    }
}

/**
 * The Permission page: the four modes (Code's CodePermissionMode semantics: Ask = Manual, Auto = Auto-accept
 * edits, Read-only = Plan, plus Bypass), the computer's default marked. A pick is for this chat only.
 */
@Composable
fun PermissionPage(supported: List<StartMode>? = null, onDone: () -> Unit) {
    val def = PendingStart.defaultMode
    val modes = supported ?: PendingStart.options?.modes?.values?.firstOrNull()?.supported?.takeIf { it.isNotEmpty() } ?: StartMode.entries
    Column(verticalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.testTag("permissionPage")) {
        PickGroup {
            modes.forEachIndexed { i, m ->
                if (i > 0) PickDivider()
                val shown = PendingStart.mode ?: def ?: StartMode.Default
                PickRow(m.label + if (m == (def ?: StartMode.Default)) " · default" else "", checked = shown == m, testTag = "permission-${m.id}") {
                    // Any tap is an explicit pick for this chat: the start then sends `mode`.
                    PendingStart.mode = m; onDone()
                }
            }
        }
        PickFoot("A change here is for this chat only.")
    }
}
