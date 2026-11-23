package com.allternit.feature.computer

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.disabled
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.str
import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.doubleOrNull

/** One write's outcome (iOS ACIComputersClient.Failure). Asleep and Unavailable never retry or wake. */
internal sealed interface ConsoleWrite {
    data class Ok(val body: JsonElement?) : ConsoleWrite
    data object Asleep : ConsoleWrite
    data object Unavailable : ConsoleWrite
    data class NeedsElevation(val message: String) : ConsoleWrite
    data class Failed(val message: String) : ConsoleWrite
}

// ---- Models (iOS ACIComputersClient HostAction / HostFinding / SSHKey / CloudBackup) ----

/** Contract `HostAction` (`{ type, target }`); [label] is the plain sentence the node may add. */
internal data class HostAction(val type: String, val target: String, val label: String?) {
    val commandLine: String get() = "$type $target"

    companion object {
        fun parse(e: JsonElement?): HostAction? {
            val o = e as? JsonObject ?: return null
            return HostAction(o.str("type") ?: return null, o.str("target") ?: return null, o.str("label"))
        }
    }
}

internal data class HostFinding(val id: String, val severity: String, val title: String, val detail: String, val suggestedAction: HostAction?) {
    companion object {
        fun parse(o: JsonObject): HostFinding? {
            val title = o.str("title") ?: return null
            return HostFinding(o.str("id") ?: title, o.str("severity") ?: "info", title, o.str("detail").orEmpty(), HostAction.parse(o["suggestedAction"]))
        }
    }
}

internal data class HostActionResult(val changeId: String, val undo: HostAction?) {
    companion object {
        fun parse(e: JsonElement?): HostActionResult? {
            val o = e as? JsonObject ?: return null
            return HostActionResult(o.str("changeId") ?: return null, HostAction.parse(o["undo"]))
        }
    }
}

internal data class SshKey(val id: String, val label: String, val fingerprint: String?, val kind: String?) {
    /** Agent keys are short-lived and rotate on their own, so they have no Remove. */
    val isAgent: Boolean get() = kind == "agent"

    companion object {
        fun parse(o: JsonObject): SshKey? {
            val id = o.str("id") ?: return null
            return SshKey(id, o.str("label") ?: return null, o.str("fingerprint"), o.str("kind"))
        }
    }
}

internal data class CloudBackup(val id: String, val createdAt: String, val sizeBytes: Double?) {
    val instant: Instant? get() = runCatching { Instant.parse(createdAt) }.getOrNull()

    companion object {
        fun parse(o: JsonObject): CloudBackup? {
            val size = (o["sizeBytes"] as? JsonPrimitive)?.doubleOrNull
            return CloudBackup(o.str("id") ?: return null, o.str("createdAt") ?: return null, size)
        }
    }
}

/** `{<key>:[...]}` or a bare array. */
internal fun JsonElement.rows(key: String): List<JsonObject> = when (this) {
    is JsonArray -> mapNotNull { it as? JsonObject }
    is JsonObject -> list(key)
    else -> emptyList()
}

/** Newest backup newer than 24 hours, or null (iOS consoleFreshBackup). */
internal fun consoleFreshBackup(backups: List<CloudBackup>, now: Instant = Instant.now()): CloudBackup? =
    backups.mapNotNull { b -> b.instant?.let { b to it } }
        .filter { Duration.between(it.second, now).seconds < 24 * 3600 }
        .maxByOrNull { it.second }?.first

/** Public-key form check for Add a key: refuses private keys, needs ssh- / ecdsa- / sk-. */
internal fun consoleLooksPrivate(key: String) = key.contains("PRIVATE KEY")

internal fun consoleKeyValid(label: String, key: String): Boolean {
    val k = key.trim()
    return label.isNotBlank() && !consoleLooksPrivate(k) && (k.startsWith("ssh-") || k.startsWith("ecdsa-") || k.startsWith("sk-"))
}

internal fun consoleWhen(raw: String): String {
    val at = runCatching { Instant.parse(raw) }.getOrNull() ?: return raw
    return DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(at)
}

internal fun consoleAgo(raw: String, now: Instant = Instant.now()): String {
    val at = runCatching { Instant.parse(raw) }.getOrNull() ?: return raw
    val m = Duration.between(at, now).toMinutes().coerceAtLeast(0)
    return when {
        m < 1 -> "just now"
        m < 60 -> "$m min ago"
        m < 48 * 60 -> "${m / 60} hr ago"
        else -> "${m / (24 * 60)} days ago"
    }
}

/** Plain sentence for a failed change. Never a status code (iOS consoleFailureText). */
internal fun consoleFailureText(w: ConsoleWrite): String = when (w) {
    ConsoleWrite.Unavailable -> "This isn't available for this computer yet."
    ConsoleWrite.Asleep -> "This computer is asleep. It is not woken for this. Nothing was changed."
    is ConsoleWrite.NeedsElevation -> "This change needs the one-time admin helper on that computer."
    is ConsoleWrite.Failed -> w.message
    is ConsoleWrite.Ok -> ""
}

/** A read mapped to the same shapes the tabs draw. */
internal suspend fun <T> readRows(id: String, tail: String, key: String, parse: (JsonObject) -> T?): HostRead<List<T>> =
    when (val r = AciApi.consoleRead(id, tail)) {
        is HostRead.Ok -> HostRead.Ok(r.value.rows(key).mapNotNull(parse))
        is HostRead.Asleep -> r
        HostRead.Unavailable -> HostRead.Unavailable
        is HostRead.Failed -> r
    }

internal suspend fun consoleBackups(id: String) = readRows(id, "backups", "backups", CloudBackup::parse)

/**
 * The canonical two-step (LANES-BRIEF "APPROVAL FLOW"): 1) POST approvals {type,target} gives an approvalId bound to
 * user + computer + type + target; 2) POST host/actions {type,target,approval_id} gives {changeId, undo?}.
 * An undo goes through the same two steps with its own approval.
 */
internal suspend fun consoleRunAction(id: String, action: HostAction): ConsoleWrite {
    val approval = AciApi.consoleWrite("POST", id, "approvals", buildBody("type" to action.type, "target" to action.target))
    if (approval !is ConsoleWrite.Ok) return approval
    val approvalId = (approval.body as? JsonObject)?.let { it.str("approvalId") ?: it.str("approval_id") }?.takeIf { it.isNotEmpty() }
        ?: return ConsoleWrite.Failed("Couldn't get an approval for this change. Nothing was changed.")
    val ran = AciApi.consoleWrite("POST", id, "host/actions", buildBody("type" to action.type, "target" to action.target, "approval_id" to approvalId))
    if (ran !is ConsoleWrite.Ok) return ran
    return if (HostActionResult.parse(ran.body) != null) ran else ConsoleWrite.Failed("The change ran but the answer was unreadable. Check Activity.")
}

internal fun buildBody(vararg pairs: Pair<String, String>): JsonObject = JsonObject(pairs.associate { it.first to JsonPrimitive(it.second) })

// ---- Shared pieces ----

@Composable
internal fun CTitle(text: String) {
    Text(text, color = AllternitTheme.colors.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
}

@Composable
internal fun CText(text: String, secondary: Boolean = true, size: Int = 13, color: androidx.compose.ui.graphics.Color? = null) {
    val c = AllternitTheme.colors
    Text(text, color = color ?: if (secondary) c.textSecondary else c.textTertiary, fontSize = size.sp)
}

@Composable
internal fun CCode(text: String) {
    val c = AllternitTheme.colors
    SelectionContainer {
        Text(text, color = c.textPrimary, fontSize = 13.sp, fontFamily = FontFamily.Monospace,
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.textPrimary.copy(alpha = 0.08f)).padding(horizontal = 12.dp, vertical = 8.dp))
    }
}

@Composable
private fun StatusLine(text: String?, error: Boolean = false) {
    if (text != null) Text(text, color = if (error) AllternitStatic.StatusError else AllternitTheme.colors.textSecondary, fontSize = 13.sp, modifier = Modifier.fillMaxWidth())
}

/** One loadable read: state + a reload. */
internal class HostLoad<T>(val state: HostRead<T>?, val reload: () -> Unit)

@Composable
internal fun <T> rememberHostLoad(key: Any?, load: suspend () -> HostRead<T>): HostLoad<T> {
    var state by remember(key) { mutableStateOf<HostRead<T>?>(null) }
    var attempt by remember(key) { mutableIntStateOf(0) }
    LaunchedEffect(key, attempt) { state = null; state = load() }
    return HostLoad(state) { attempt++ }
}

/** The non-ready states every card shares (loading, asleep, not available, error + Try again). */
@Composable
internal fun <T> HostNotReady(s: HostLoad<T>, unavailable: String, asleep: String, loading: String = "Loading…") {
    when (val st = s.state) {
        null -> CNote(loading)
        HostRead.Unavailable -> CNote(unavailable)
        is HostRead.Asleep -> CNote(asleep)
        is HostRead.Failed -> Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            CNote(st.message)
            AciPillButton("Try again", "aciConsoleManageRetry") { s.reload() }
        }
        is HostRead.Ok -> Unit
    }
}

/**
 * Hold to confirm: a 1 second press-and-hold capsule with a fill (iOS ACIHoldButton). Releasing early cancels.
 * A screen reader gets a plain activate action, since holding is not possible there.
 */
@Composable
internal fun AciHoldButton(title: String, testTag: String, danger: Boolean = false, enabled: Boolean = true, onHeld: () -> Unit) {
    val c = AllternitTheme.colors
    val progress = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val tint = if (danger) AllternitStatic.StatusError else c.textPrimary
    Box(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(CircleShape).background(c.fillNeutral).border(1.dp, if (danger) tint else c.borderSubtle, CircleShape)
            .alpha(if (enabled) 1f else 0.45f)
            .semantics(mergeDescendants = true) {
                role = Role.Button
                if (enabled) onClick(label = title.replaceFirst("Hold to ", "").replaceFirstChar { it.uppercase() }) { onHeld(); true } else disabled()
            }
            .pointerInput(enabled) {
                if (!enabled) return@pointerInput
                detectTapGestures(onPress = {
                    val hold = scope.launch {
                        progress.animateTo(1f, tween(1000, easing = LinearEasing))
                        onHeld()
                        progress.snapTo(0f)
                    }
                    tryAwaitRelease()
                    hold.cancel()
                    scope.launch { progress.animateTo(0f, tween(150)) }
                })
            }.testTag(testTag),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.align(Alignment.CenterStart).fillMaxHeight().fillMaxWidth(progress.value).background(tint.copy(alpha = 0.18f)))
        Text(title, color = tint, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}

// ---- Access ----

@Composable
internal fun AccessTab(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val keys = rememberHostLoad(computer.id) { readRows(computer.id, "ssh-keys", "keys", SshKey::parse) }
    var adding by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf<SshKey?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    CCard {
        CTitle("Who can use it")
        if (computer.group == "Shared with you") {
            CRow("You", "Member")
            CText("It was shared with you. The owner decides who else can use it.", false, 12)
        } else {
            CRow("You", "Owner")
            CText("Members of your organization can use it when it is shared with them. Agents ask before they change anything.", false, 12)
        }
    }
    if (computer.kind == "cloud") {
        CCard {
            CTitle("Hosted by Allternit")
            CText("Part of your plan. You reach it through the Allternit relay. Nothing is opened to the internet.")
        }
    } else {
        CCard {
            CTitle("Pairing")
            CText("To pair another computer, install Gizzi there and run:")
            CCode("gizzi pair")
            CText("Install: npm install -g @allternit/gizzi-code", false, 12)
        }
    }
    CCard {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.weight(1f)) { CTitle("SSH keys") }
            if (keys.state is HostRead.Ok) AciPillButton("Add a key", "aciConsoleAddKey") { adding = true }
        }
        HostNotReady(keys, "SSH keys aren't available on your account yet.", "Asleep, not woken. Keys show when it is awake.")
        (keys.state as? HostRead.Ok)?.value?.let { list ->
            if (list.isEmpty()) CNote("No keys yet. Add one to sign in over SSH.") else list.forEach { key ->
                Row(Modifier.fillMaxWidth().testTag("aciConsoleKey"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(key.label, color = c.textPrimary, fontSize = 14.sp)
                        Text(if (key.isAgent) "Short-lived, rotates on its own" else key.fingerprint.orEmpty(), color = c.textTertiary, fontSize = 11.sp,
                            fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    if (!key.isAgent) Box(Modifier.heightIn(min = 44.dp).clickable(onClickLabel = "Remove ${key.label}") { removing = key }.padding(horizontal = 8.dp).testTag("aciConsoleKeyRemove"), contentAlignment = Alignment.Center) {
                        Text("Remove", color = AllternitStatic.StatusError, fontSize = 13.sp)
                    }
                }
            }
        }
        StatusLine(message)
    }
    if (adding) AddKeySheet(computer, onDone = { adding = false }) { adding = false; keys.reload() }
    removing?.let { key ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Remove this key?") },
            text = { Text("It can no longer sign in to this computer.") },
            confirmButton = {
                TextButton(onClick = {
                    removing = null
                    scope.launch {
                        val r = AciApi.consoleWrite("DELETE", computer.id, "ssh-keys/${java.net.URLEncoder.encode(key.id, "UTF-8").replace("+", "%20")}", null)
                        if (r is ConsoleWrite.Ok) { message = "Removed ${key.label}."; keys.reload() } else message = consoleFailureText(r)
                    }
                }) { Text("Remove ${key.label}") }
            },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AddKeySheet(computer: ConsoleComputer, onDone: () -> Unit, onAdded: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var label by remember { mutableStateOf("") }
    var key by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val valid = consoleKeyValid(label, key)
    ModalBottomSheet(onDismissRequest = onDone, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp).testTag("aciConsoleAddKeySheet"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Add a key", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            OutlinedTextField(label, { label = it }, label = { Text("Name") }, placeholder = { Text("Studio Mac") }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("aciConsoleKeyName"))
            OutlinedTextField(key, { key = it }, label = { Text("Public key") }, placeholder = { Text("ssh-ed25519 AAAA…") }, minLines = 3, maxLines = 6,
                textStyle = androidx.compose.ui.text.TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp), modifier = Modifier.fillMaxWidth().testTag("aciConsoleKeyValue"))
            if (consoleLooksPrivate(key)) Text("That is a private key. Paste the public key (the .pub file) and keep the private one to yourself.", color = AllternitStatic.StatusError, fontSize = 12.sp)
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                AciPillButton("Cancel", "aciConsoleKeyCancel") { onDone() }
                Box(Modifier.alpha(if (valid && !busy) 1f else 0.45f)) {
                    AciPillButton(if (busy) "Adding…" else "Add", "aciConsoleKeyAdd") {
                        if (!valid || busy) return@AciPillButton
                        busy = true
                        scope.launch {
                            val r = AciApi.consoleWrite("POST", computer.id, "ssh-keys", buildBody("label" to label.trim(), "publicKey" to key.trim()))
                            busy = false
                            if (r is ConsoleWrite.Ok) onAdded() else error = consoleFailureText(r)
                        }
                    }
                }
            }
        }
    }
}

// ---- Backups ----

@Composable
internal fun BackupsTab(computer: ConsoleComputer, restorePoints: (@Composable (onDone: () -> Unit) -> Unit)?) {
    val scope = rememberCoroutineScope()
    val backups = rememberHostLoad(computer.id) { if (computer.kind == "cloud") consoleBackups(computer.id) else HostRead.Ok(emptyList()) }
    var confirming by remember { mutableStateOf<CloudBackup?>(null) }
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var showPoints by remember { mutableStateOf(false) }
    if (computer.kind == "cloud") {
        CCard {
            CTitle("Daily backups")
            HostNotReady(backups, "Backups aren't available for this computer yet.", "Asleep, not woken. Backups show when it is awake.")
            (backups.state as? HostRead.Ok)?.value?.let { list ->
                if (list.isEmpty()) CNote("No backups yet. The first one is made within a day.")
                list.forEach { b ->
                    Row(Modifier.fillMaxWidth().testTag("aciConsoleBackup"), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            CText(consoleWhen(b.createdAt), true, 14, AllternitTheme.colors.textPrimary)
                            CText(listOfNotNull(consoleAgo(b.createdAt), b.sizeBytes?.let { consoleBytes(it) }).joinToString(" · "), false, 12)
                        }
                        Box(Modifier.alpha(if (busy) 0.45f else 1f)) {
                            AciPillButton("Restore", "aciConsoleRestore", label = "Restore backup from ${consoleWhen(b.createdAt)}") { if (!busy) confirming = b }
                        }
                    }
                }
            }
            StatusLine(message)
        }
        CCard {
            CTitle("Restore points and copies")
            CText("Your existing restore points and \"Download a copy\" stay here.")
            if (restorePoints != null) AciPillButton("Open", "aciConsoleOpenRestorePoints") { showPoints = true }
        }
    } else {
        CCard {
            CTitle("Change records only")
            CText("Allternit does not copy the disk of a paired computer. Every fix writes a change record before it runs, so you can see what changed and undo it from Health.")
        }
    }
    CCard {
        CTitle("Snapshots on demand")
        CText("Taking a snapshot yourself isn't supported yet. Daily backups and change records are what protect a fix.")
    }
    if (showPoints && restorePoints != null) Dialog(onDismissRequest = { showPoints = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary).testTag("aciConsoleRestorePoints")) { restorePoints { showPoints = false } }
    }
    confirming?.let { b ->
        AlertDialog(
            onDismissRequest = { confirming = null },
            title = { Text("Restore this backup?") },
            text = { Text("The computer goes back to this point and restarts. Changes made after it are lost.") },
            confirmButton = {
                TextButton(onClick = {
                    confirming = null
                    scope.launch {
                        busy = true
                        val r = AciApi.consoleWrite("POST", computer.id, "backups/restore", buildBody("backupId" to b.id))
                        busy = false
                        message = if (r is ConsoleWrite.Ok) "Restoring from ${consoleWhen(b.createdAt)}. The computer restarts." else consoleFailureText(r)
                    }
                }) { Text("Restore ${consoleWhen(b.createdAt)}") }
            },
            dismissButton = { TextButton(onClick = { confirming = null }) { Text("Cancel") } },
        )
    }
}

// ---- Health ----

@Composable
internal fun HealthTab(computer: ConsoleComputer) {
    val c = AllternitTheme.colors
    val findings = rememberHostLoad(computer.id) { readRows(computer.id, "host/findings", "findings", HostFinding::parse) }
    var fixing by remember { mutableStateOf<HostFinding?>(null) }
    HostNotReady(findings, "Health checks aren't available for this computer yet.", "Asleep, not woken. Health shows when it is awake.")
    (findings.state as? HostRead.Ok)?.value?.let { list ->
        if (list.isEmpty()) CNote("Nothing needs a look. Disk, services, updates and open ports are all fine.") else list.forEach { f ->
            CCard {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(when (f.severity) {
                        "critical" -> AllternitStatic.StatusError
                        "warning" -> AllternitStatic.StatusWarning
                        else -> c.textTertiary
                    }))
                    Text(f.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                }
                CText(f.detail)
                if (f.suggestedAction != null) AciPillButton("Fix", "aciConsoleFix", label = "Fix: ${f.title}") { fixing = f }
            }
        }
    }
    fixing?.let { FixSheet(computer, it, onDone = { fixing = null }, onChanged = { findings.reload() }) }
}

private sealed interface FixPhase {
    data object Review : FixPhase
    data object Running : FixPhase
    data class Done(val changeId: String, val undo: HostAction?) : FixPhase
    data class Undone(val changeId: String) : FixPhase
    data object NeedsElevation : FixPhase
    data class Failed(val message: String) : FixPhase
}

/** The approval sheet: what is wrong, the exact action, restore point status, then Hold to approve; after it ran, the change id and Hold to undo (its own approval). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun FixSheet(computer: ConsoleComputer, finding: HostFinding, onDone: () -> Unit, onChanged: () -> Unit, heading: String = "What is wrong", navTitle: String = "Approve a fix") {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val action = finding.suggestedAction
    var phase by remember { mutableStateOf<FixPhase>(FixPhase.Review) }
    val restore = rememberHostLoad(computer.id) { if (computer.kind == "cloud") consoleBackups(computer.id) else HostRead.Ok(emptyList()) }

    fun run(a: HostAction, undoing: Boolean) {
        phase = FixPhase.Running
        scope.launch {
            phase = run {
                when (val r = consoleRunAction(computer.id, a)) {
                    is ConsoleWrite.Ok -> {
                        val res = HostActionResult.parse(r.body)!!
                        onChanged()
                        if (undoing) FixPhase.Undone(res.changeId) else FixPhase.Done(res.changeId, res.undo)
                    }
                    is ConsoleWrite.NeedsElevation -> FixPhase.NeedsElevation
                    else -> FixPhase.Failed(consoleFailureText(r))
                }
            }
        }
    }

    ModalBottomSheet(onDismissRequest = onDone, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.92f).testTag("aciConsoleFixSheet")) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(navTitle, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Box(Modifier.heightIn(min = 44.dp).clickable { onDone() }.padding(start = 12.dp).testTag("aciConsoleFixClose"), contentAlignment = Alignment.Center) { Text("Close", color = c.textPrimary, fontSize = 15.sp) }
            }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                CCard {
                    CTitle(heading)
                    Text(finding.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    CText(finding.detail)
                }
                if (action == null) CNote("There is no one-tap fix for this one.") else {
                    CCard {
                        CTitle("This will run on ${computer.name}")
                        action.label?.takeIf { it.isNotEmpty() }?.let { CText(it, true, 14, c.textPrimary) }
                        CCode(action.commandLine)
                    }
                    CCard {
                        CTitle("Restore point")
                        RestoreStatus(computer, restore)
                    }
                    FixFooter(computer, phase, action, ::run, onDone) { phase = FixPhase.Review }
                }
            }
        }
    }
}

@Composable
private fun RestoreStatus(computer: ConsoleComputer, restore: HostLoad<List<CloudBackup>>) {
    if (computer.kind != "cloud") {
        CText("A change record is written first, so you can see exactly what changed and undo it.")
        return
    }
    when (val s = restore.state) {
        null -> CText("Checking backups…")
        is HostRead.Ok -> {
            val fresh = consoleFreshBackup(s.value)
            if (fresh != null) CText("Backup from ${consoleAgo(fresh.createdAt)}. A change record is written too.")
            else CText("No backup newer than 24 hours. You can still approve, but this change could not be rolled back from a backup.", color = AllternitStatic.StatusWarning)
        }
        is HostRead.Asleep -> CText("Asleep, not woken. Backups can't be checked right now.")
        HostRead.Unavailable, is HostRead.Failed -> CText("Couldn't check backups. A change record is still written first.", color = AllternitStatic.StatusWarning)
    }
}

@Composable
private fun FixFooter(computer: ConsoleComputer, phase: FixPhase, action: HostAction, run: (HostAction, Boolean) -> Unit, onDone: () -> Unit, retry: () -> Unit) {
    val c = AllternitTheme.colors
    when (phase) {
        FixPhase.Review -> Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            AciHoldButton("Hold to approve", "aciConsoleHoldApprove") { run(action, false) }
            Box(Modifier.heightIn(min = 44.dp).clickable { onDone() }.padding(horizontal = 16.dp).testTag("aciConsoleNotNow"), contentAlignment = Alignment.Center) { CText("Not now", true, 14) }
        }
        FixPhase.Running -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CircularProgressIndicator(Modifier.size(18.dp), color = c.textSecondary, strokeWidth = 2.dp)
            CText("Running…", true, 14)
        }
        is FixPhase.Done -> CCard {
            Text("Approved by you. It ran.", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            CRow("Change", phase.changeId)
            phase.undo?.let { undo ->
                CText("You can undo this. Undo needs its own approval.", false, 12)
                AciHoldButton("Hold to undo", "aciConsoleHoldUndo") { run(undo, true) }
            }
        }
        is FixPhase.Undone -> CCard {
            Text("Undone.", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            CRow("Change", phase.changeId)
        }
        FixPhase.NeedsElevation -> CCard {
            Text("One-time setup needed", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            CText("This change needs administrator rights, and the helper that grants them isn't set up on ${computer.name}. You set it up once, on that computer. It asks for the admin password one time and never stores it. Nothing was changed. Come back and approve again after the setup.")
        }
        is FixPhase.Failed -> CCard {
            Text(phase.message, color = AllternitStatic.StatusError, fontSize = 13.sp)
            AciPillButton("Try again", "aciConsoleFixRetry") { retry() }
        }
    }
}

// ---- Settings ----

private val LOCATIONS = listOf("nyc" to "New York", "chi" to "Chicago", "lon" to "London", "syd" to "Sydney")

internal fun consoleLocationKey(region: String?): String? {
    val r = region.orEmpty().lowercase()
    return LOCATIONS.firstOrNull { it.first == r || it.second.lowercase() == r }?.first
}

@Composable
internal fun SettingsTab(computer: ConsoleComputer, onChanged: () -> Unit, onGone: () -> Unit) {
    val scope = rememberCoroutineScope()
    var message by remember { mutableStateOf<String?>(null) }
    var isError by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }

    /** Runs one write; success text or a plain failure sentence. Returns true on success. */
    fun perform(success: String, call: suspend () -> ConsoleWrite, after: () -> Unit = {}) {
        busy = true
        scope.launch {
            val r = call()
            busy = false
            isError = r !is ConsoleWrite.Ok
            message = if (r is ConsoleWrite.Ok) success else consoleFailureText(r)
            if (r is ConsoleWrite.Ok) after()
        }
    }

    when (computer.kind) {
        "cloud" -> CloudSettings(computer, busy) { text, call -> perform(text, call) }
        "paired" -> PairedSettings(computer, busy, { text, call, after -> perform(text, call, after) }, onChanged, onGone)
        else -> CNote("This is the computer you are holding. Manage it from Allternit Desktop.")
    }
    StatusLine(message, isError)
}

@Composable
private fun CloudSettings(computer: ConsoleComputer, busy: Boolean, perform: (String, suspend () -> ConsoleWrite) -> Unit) {
    val c = AllternitTheme.colors
    val current = consoleLocationKey(computer.region)
    var location by remember { mutableStateOf(current.orEmpty()) }
    var confirmRestart by remember { mutableStateOf(false) }
    val backups = rememberHostLoad(computer.id) { consoleBackups(computer.id) }
    CCard {
        CTitle("Location")
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            LOCATIONS.forEach { (key, name) ->
                val on = key == location
                Box(Modifier.heightIn(min = 44.dp).clip(CircleShape).background(if (on) c.textPrimary.copy(alpha = 0.12f) else c.fillNeutral)
                    .border(1.dp, if (on) c.textPrimary else c.borderSubtle, CircleShape)
                    .selectable(selected = on, role = Role.RadioButton, onClick = { location = key }).padding(horizontal = 16.dp).testTag("aciConsoleLocation$key"), contentAlignment = Alignment.Center) {
                    Text(name, color = c.textPrimary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
                }
            }
        }
        CText("A cloud computer stays in the city it was made in. Moving makes a fresh computer in the city you pick and erases this one's files. Files you pinned on a device stay there. Save anything you need first.", false, 12)
        val city = LOCATIONS.firstOrNull { it.first == location }?.second ?: location
        AciHoldButton("Hold to move to $city", "aciConsoleMove", danger = true, enabled = !busy && location.isNotEmpty() && location != current) {
            perform("Moving. A fresh computer is being set up in $city. This takes a few minutes.") {
                AciApi.consoleWrite("POST", computer.id, "move", JsonObject(mapOf("location" to JsonPrimitive(location), "confirm" to JsonPrimitive(true))))
            }
        }
    }
    CCard {
        CTitle("Restart")
        CText("Running work stops. Saved files stay.", false, 12)
        Box(Modifier.alpha(if (busy) 0.45f else 1f)) { AciPillButton("Restart", "aciConsoleRestart") { if (!busy) confirmRestart = true } }
    }
    CCard {
        CTitle("Rebuild")
        CText("Rebuild erases the disk and sets the computer up again from the Ubuntu image, in the same location. Files you pinned on a device stay there.")
        RebuildGate(computer, backups, busy, perform)
    }
    CCard {
        CTitle("Coming soon")
        listOf("Resize", "Grow disk", "Clone").forEach { item ->
            Row(Modifier.fillMaxWidth().semantics(mergeDescendants = true) { disabled() }, verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f)) { CText(item, false, 14) }
                CText("Coming soon", false, 12)
            }
        }
        CText("Prices show before you confirm, once they are set.", false, 12)
    }
    if (confirmRestart) AlertDialog(
        onDismissRequest = { confirmRestart = false },
        title = { Text("Restart this computer?") },
        confirmButton = {
            TextButton(onClick = {
                confirmRestart = false
                perform("Restarting.") { AciApi.consoleWrite("POST", computer.id, "restart", null) }
            }) { Text("Restart") }
        },
        dismissButton = { TextButton(onClick = { confirmRestart = false }) { Text("Cancel") } },
    )
}

@Composable
private fun RebuildGate(computer: ConsoleComputer, backups: HostLoad<List<CloudBackup>>, busy: Boolean, perform: (String, suspend () -> ConsoleWrite) -> Unit) {
    val warn = AllternitStatic.StatusWarning
    when (val s = backups.state) {
        null -> CText("Checking backups…")
        is HostRead.Ok -> {
            val fresh = consoleFreshBackup(s.value)
            if (fresh != null) {
                CText("Backup from ${consoleAgo(fresh.createdAt)}.")
                AciHoldButton("Hold to rebuild", "aciConsoleHoldRebuild", danger = true, enabled = !busy) {
                    perform("Rebuilding. This takes a few minutes.") { AciApi.consoleWrite("POST", computer.id, "rebuild", JsonObject(mapOf("confirm" to JsonPrimitive(true)))) }
                }
            } else {
                CText("Rebuild is held. There is no backup newer than 24 hours, and a rebuild erases the only copy of the disk.", color = warn)
                AciHoldButton("Hold to rebuild", "aciConsoleHoldRebuild", danger = true, enabled = false) {}
            }
        }
        is HostRead.Asleep -> {
            CText("Asleep, not woken. Rebuild is held until the backup check can run.")
            AciHoldButton("Hold to rebuild", "aciConsoleHoldRebuild", danger = true, enabled = false) {}
        }
        HostRead.Unavailable -> CText("Rebuild isn't available for this computer yet.")
        is HostRead.Failed -> CText("${s.message} Rebuild is held until the backup check works.", color = warn)
    }
}

@Composable
private fun PairedSettings(
    computer: ConsoleComputer, busy: Boolean,
    perform: (String, suspend () -> ConsoleWrite, () -> Unit) -> Unit, onChanged: () -> Unit, onGone: () -> Unit,
) {
    var name by remember { mutableStateOf(computer.name) }
    val trimmed = name.trim()
    val canSave = !busy && trimmed.isNotEmpty() && trimmed != computer.name
    CCard {
        CTitle("Name")
        OutlinedTextField(name, { name = it }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("aciConsoleRenameField"))
        Box(Modifier.alpha(if (canSave) 1f else 0.45f)) {
            AciPillButton("Save", "aciConsoleRename") {
                if (canSave) perform("Renamed.", { AciApi.consolePairedWrite("PATCH", computer.id, buildBody("name" to trimmed)) }, onChanged)
            }
        }
    }
    CCard {
        CTitle("Remove")
        CText("Unpair to take it off your account and away from every agent. The computer itself keeps running at ${computer.provider}.")
        AciHoldButton("Hold to unpair", "aciConsoleHoldUnpair", danger = true, enabled = !busy) {
            perform("Unpaired.", { AciApi.consoleUnpair(computer.id) }) { onChanged(); onGone() }
        }
    }
}
