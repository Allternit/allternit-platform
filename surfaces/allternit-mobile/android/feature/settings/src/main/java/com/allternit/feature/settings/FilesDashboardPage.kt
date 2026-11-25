package com.allternit.feature.settings

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * cloud-api's Files routes that act for the signed-in user (iOS `FilesCloudAPI` in Core/Files/FilesDrive.swift):
 * wake settings, shares, share access (the guest audit) and storage. They carry the Clerk session like the rest of
 * the app (FilesSession: `resolve_user_scoped(…, "compute")`), never the Files key. The computers are the same list
 * :app's FilesDrive shows (`GET /api/v1/runtime-devices`, phones left out, `runtime:files` capability).
 */
internal object FilesCloud {
    data class Computer(val id: String, val name: String)
    data class Share(
        val shareId: String, val computerId: String, val role: String, val linkUrl: String, val createdAt: String,
        val expiresAt: String?, val passcodeRequired: Boolean, val guestWake: Boolean, val revokedAt: String?, val guestCount: Long,
    )
    data class Access(val shareId: String, val guestId: String, val action: String, val result: String, val at: String)
    data class Volume(val id: String, val label: String, val free: Long, val total: Long)

    class Failed(message: String) : Exception(message)

    private suspend fun call(method: String, path: String, body: JsonElement? = null): JsonElement? = when (val r = Net.cloud(method, path, body)) {
        is ApiResult.Ok -> r.value
        is ApiResult.HttpError -> throw Failed(r.message?.takeIf { it.isNotBlank() } ?: r.friendly("load Files"))
        is ApiResult.Network -> throw Failed("You're offline. Check your connection and try again.")
        else -> throw Failed("Allternit sent a reply this app couldn't read.")
    }

    fun seg(s: String): String = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    suspend fun computers(): List<Computer> = call("GET", "api/v1/runtime-devices").obj.list("runtimes").mapNotNull { o ->
        val type = o.str("runtimeType").orEmpty()
        if (type in setOf("ios", "android", "ipados")) return@mapNotNull null
        val caps = o["capabilities"] as? kotlinx.serialization.json.JsonArray
        if (caps != null && caps.none { (it as? JsonPrimitive)?.content == "runtime:files" }) return@mapNotNull null
        Computer(o.str("id") ?: return@mapNotNull null, o.str("name")?.ifBlank { null } ?: "Computer")
    }

    /** `GET /:id/endpoints`: the computer's state (`awake`, `asleep`, `waking`, `offline`). */
    suspend fun state(cid: String): String? = runCatching { call("GET", "api/v1/computer-files/${seg(cid)}/endpoints").obj.str("computer_state") }.getOrNull()

    suspend fun wake(cid: String): String = call("GET", "api/v1/computer-files/${seg(cid)}/wake-settings").obj.str("wake") ?: "ask"

    suspend fun setWake(cid: String, wake: String): String =
        call("PUT", "api/v1/computer-files/${seg(cid)}/wake-settings", buildJsonObject { put("wake", JsonPrimitive(wake)) }).obj.str("wake") ?: wake

    suspend fun storage(cid: String): List<Volume> = call("GET", "api/v1/computer-files/${seg(cid)}/storage").obj.list("volumes").map {
        Volume(it.str("volume_id").orEmpty(), it.str("label").orEmpty(), it.long("free_bytes") ?: 0, it.long("total_bytes") ?: 0)
    }

    suspend fun shares(cid: String): List<Share> = call("GET", "api/v1/computer-files/${seg(cid)}/shares").obj.list("shares").mapNotNull { o ->
        Share(
            o.str("share_id") ?: return@mapNotNull null, o.str("computer_id") ?: cid, o.str("role").orEmpty(), o.str("link_url").orEmpty(),
            o.str("created_at").orEmpty(), o.str("expires_at"), o.bool("passcode_required") == true, o.bool("guest_wake") == true,
            o.str("revoked_at"), o.long("guest_count") ?: 0,
        )
    }

    suspend fun revoke(shareId: String) { call("DELETE", "api/v1/computer-files/shares/${seg(shareId)}") }

    /** `GET /shares/:id/access`: a bare `ShareAccess[]` (SPEC); an `{access: […]}` wrapper is read too. */
    suspend fun access(shareId: String): List<Access> = call("GET", "api/v1/computer-files/shares/${seg(shareId)}/access").listUnder("access").map {
        Access(it.str("share_id") ?: shareId, it.str("guest_id").orEmpty(), it.str("action").orEmpty(), it.str("result").orEmpty(), it.str("at").orEmpty())
    }

    // iOS FilesFormat / FilesDashboardView labels.
    fun wakeLabel(w: String) = when (w) { "auto" -> "Automatically"; "never" -> "Never"; else -> "Ask first" }
    fun roleLabel(r: String) = when (r) { "upload_only" -> "Upload only"; "edit" -> "Can edit"; "comment" -> "Can comment"; else -> "View and download" }
    fun stateLabel(s: String?) = when (s) { "awake" -> "Awake"; "asleep" -> "Asleep"; "waking" -> "Waking up"; "offline" -> "Offline"; else -> "Unknown" }
    fun count(n: Long, noun: String) = if (n == 1L) "1 $noun" else "$n ${noun}s"
    /** iOS ByteCountFormatter(.file): decimal units. */
    fun bytes(n: Long): String {
        if (n < 1000) return "$n bytes"
        val units = listOf("KB", "MB", "GB", "TB", "PB")
        var v = n.toDouble()
        var i = -1
        while (v >= 1000 && i < units.lastIndex) { v /= 1000; i++ }
        return if (v >= 100) "${v.toLong()} ${units[i]}" else String.format(Locale.getDefault(), "%.1f %s", v, units[i])
    }

    fun day(raw: String?): String = parseTime(raw).takeIf { it > 0 }?.let { SimpleDateFormat("MMM d", Locale.getDefault()).format(Date(it)) }.orEmpty()

    fun shareTitle(s: Share, computerName: String) = "${roleLabel(s.role)} · $computerName"

    fun shareLine(s: Share): String {
        val parts = ArrayList<String>()
        parts += day(s.expiresAt).takeIf { it.isNotEmpty() }?.let { "Ends $it" } ?: "Until turned off"
        if (s.passcodeRequired) parts += "passcode"
        parts += count(s.guestCount, "guest")
        if (s.guestWake) parts += "may wake the computer"
        return parts.joinToString(" · ")
    }

    fun actionLabel(a: Access): String {
        val verb = when (a.action) {
            "redeem" -> "Opened the link"; "list" -> "Listed files"; "download" -> "Downloaded a file"
            "upload" -> "Uploaded a file"; "comment" -> "Commented"; "wake" -> "Asked to wake the computer"; else -> "Used the link"
        }
        return if (a.result == "denied") "$verb (blocked)" else verb
    }
}

private enum class FilesTab(val title: String) { Sharing("Sharing"), Devices("Devices"), Audit("Audit") }

private class FilesDeviceRow(val computer: FilesCloud.Computer, val state: String?, val wake: String?, val volumes: List<FilesCloud.Volume>, val error: String?)
private class FilesAuditRow(val access: FilesCloud.Access, val share: FilesCloud.Share, val computerName: String)

/**
 * Settings > Files, the port of iOS `FilesDashboardView` (design 4, Dashboard A): the Files switch, then tabs
 * Sharing (every live share on every computer, Copy link, Turn off after a confirm), Devices (each computer's state,
 * its wake setting for Files as three taps, its disks; this phone; the Files key) and Audit (what guests did with
 * those shares, newest first).
 */
@Composable
internal fun FilesPage(back: () -> Unit, push: (String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val c = AllternitTheme.colors
    var enabled by remember { mutableStateOf(SettingsPrefs.flag(context, "files.enabled", false)) }
    var tab by remember { mutableStateOf(FilesTab.Sharing) }
    var rows by remember { mutableStateOf<List<FilesDeviceRow>?>(null) }
    var shares by remember { mutableStateOf<List<Pair<FilesCloud.Share, String>>>(emptyList()) }
    var audit by remember { mutableStateOf<List<FilesAuditRow>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var revoking by remember { mutableStateOf<FilesCloud.Share?>(null) }
    var nonce by remember { mutableIntStateOf(0) }

    LaunchedEffect(enabled, nonce) {
        if (!enabled) return@LaunchedEffect
        val computers = try { FilesCloud.computers() } catch (e: Exception) { error = e.message; rows = emptyList(); return@LaunchedEffect }
        val newRows = ArrayList<FilesDeviceRow>()
        val newShares = ArrayList<Pair<FilesCloud.Share, String>>()
        for (computer in computers) {
            val state = FilesCloud.state(computer.id)
            val wake = runCatching { FilesCloud.wake(computer.id) }.getOrNull()
            val volumes = runCatching { FilesCloud.storage(computer.id) }.getOrDefault(emptyList())
            var rowError: String? = null
            try {
                newShares += FilesCloud.shares(computer.id).filter { it.revokedAt == null }.map { it to computer.name }
            } catch (e: Exception) {
                rowError = e.message
            }
            newRows += FilesDeviceRow(computer, state, wake, volumes, rowError)
        }
        rows = newRows
        val sorted = newShares.sortedByDescending { parseTime(it.first.createdAt) }
        shares = sorted
        val events = ArrayList<FilesAuditRow>()
        for ((share, name) in sorted.take(20)) {
            runCatching { FilesCloud.access(share.shareId) }.getOrDefault(emptyList()).forEach { events += FilesAuditRow(it, share, name) }
        }
        audit = events.sortedByDescending { parseTime(it.access.at) }.take(200)
        error = null
    }

    SPage("Files", tag = "settingsFiles", onBack = back) {
        SNote("Files keeps your files in sync between your devices and computers, encrypted with a key only your devices hold.")
        SCard {
            SToggleRow("Files on this phone", "files_enabled", enabled, onChange = { enabled = it; SettingsPrefs.setFlag(context, "files.enabled", it) })
        }
        if (!enabled) {
            SNote("Files is off. Turn it on to see your key, shares and guests.")
            return@SPage
        }
        // The segmented control (iOS Picker .segmented): three taps, never a menu.
        Row(
            Modifier.fillMaxWidth().padding(top = 18.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(2.dp),
            horizontalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            FilesTab.entries.forEach { t ->
                val on = t == tab
                Box(
                    Modifier.weight(1f).heightIn(min = 36.dp).clip(RoundedCornerShape(8.dp)).background(if (on) c.bgPrimary else c.fillNeutral)
                        .clickable { tab = t }.testTag("files_tab_${t.name.lowercase()}"),
                    contentAlignment = Alignment.Center,
                ) { Text(t.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal) }
            }
        }
        error?.let { SError(it) }
        notice?.let { SNote(it) }
        val list = rows
        if (list == null) {
            SNote("Loading…")
            return@SPage
        }
        when (tab) {
            FilesTab.Sharing -> if (shares.isEmpty()) {
                com.allternit.core.design.InlineEmpty("No shares. Folders you share from the web, Desktop or a computer's Files screen show here, and you can turn them off.", art = com.allternit.core.design.EmptyArtKind.ConnectScene)
            } else {
                SCard(Modifier.padding(top = 12.dp)) {
                    shares.forEachIndexed { i, (share, name) ->
                        if (i > 0) SDivider()
                        Column(Modifier.fillMaxWidth().padding(16.dp).testTag("files_share_$i"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(FilesCloud.shareTitle(share, name), color = c.textPrimary, fontSize = 16.sp)
                            Text(FilesCloud.shareLine(share), color = c.textSecondary, fontSize = 12.sp)
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                TextButton(onClick = {
                                    (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Share link", share.linkUrl))
                                    notice = "Link copied"
                                }, modifier = Modifier.testTag("files_share_copy_$i")) { Text("Copy link", color = c.textPrimary) }
                                Box(Modifier.weight(1f))
                                TextButton(onClick = { revoking = share }, modifier = Modifier.testTag("files_share_off_$i")) { Text("Turn off", color = Danger) }
                            }
                        }
                    }
                }
            }
            FilesTab.Devices -> {
                list.forEach { row ->
                    SLabel(row.computer.name)
                    SCard {
                        SRow("State", "files_state_${row.computer.id}", value = FilesCloud.stateLabel(row.state), chevron = false)
                        SDivider()
                        Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text("Wake for Files", color = c.textPrimary, fontSize = 15.sp)
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                listOf("ask", "auto", "never").forEach { w ->
                                    val on = (row.wake ?: "ask") == w
                                    Box(
                                        Modifier.weight(1f).heightIn(min = 36.dp).clip(RoundedCornerShape(8.dp))
                                            .background(if (on) c.textPrimary else c.fillNeutral)
                                            .clickable(enabled = !on) {
                                                scope.launch {
                                                    try {
                                                        val saved = FilesCloud.setWake(row.computer.id, w)
                                                        rows = rows?.map { if (it.computer.id == row.computer.id) FilesDeviceRow(it.computer, it.state, saved, it.volumes, it.error) else it }
                                                    } catch (e: Exception) {
                                                        error = "Couldn't change the wake setting. ${e.message.orEmpty()}"
                                                    }
                                                }
                                            }.testTag("files_wake_${w}_${row.computer.id}"),
                                        contentAlignment = Alignment.Center,
                                    ) { Text(FilesCloud.wakeLabel(w), color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.sp) }
                                }
                            }
                        }
                        row.volumes.forEach { v ->
                            SDivider()
                            SRow(v.label, "files_volume_${v.id}", value = "${FilesCloud.bytes(v.free)} free of ${FilesCloud.bytes(v.total)}", chevron = false)
                        }
                        row.error?.let { SDivider(); SNote(it, Modifier.padding(16.dp)) }
                    }
                }
                if (list.isNotEmpty()) SNote("Browsing the saved list never wakes a computer. Opening or saving a file wakes it only as set here, and an awake computer bills credits at your plan's awake rate.")
                SLabel("This phone")
                SCard { SRow("Name", "files_device", value = android.os.Build.MODEL ?: "Android", chevron = false) }
                SCard(Modifier.padding(top = 18.dp)) { SRow("Files key", "files_keysRow", detail = "This phone's key, waiting devices, and giving a computer the key.", onClick = { push("files-keys") }) }
            }
            FilesTab.Audit -> if (audit.isEmpty()) {
                SNote("No guest activity. When someone opens, downloads or uploads through a share, it shows here.")
            } else {
                SCard(Modifier.padding(top = 12.dp)) {
                    audit.forEachIndexed { i, row ->
                        if (i > 0) SDivider()
                        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp).testTag("files_audit_$i")) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(FilesCloud.actionLabel(row.access), color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                                Text(FilesCloud.day(row.access.at), color = c.textTertiary, fontSize = 12.sp)
                            }
                            Text("Guest ${row.access.guestId} · ${FilesCloud.shareTitle(row.share, row.computerName)}", color = c.textSecondary, fontSize = 12.sp)
                        }
                    }
                }
            }
        }
    }

    revoking?.let { share ->
        AlertDialog(
            onDismissRequest = { revoking = null },
            title = { Text("Turn off this share?") },
            text = { Text("People with the link lose access on their next request.") },
            confirmButton = {
                TextButton(onClick = {
                    revoking = null
                    scope.launch {
                        try {
                            FilesCloud.revoke(share.shareId)
                            shares = shares.filterNot { it.first.shareId == share.shareId }
                            notice = "Share turned off"
                        } catch (e: Exception) {
                            error = "Couldn't turn off the share. ${e.message.orEmpty()}"
                        }
                    }
                }, modifier = Modifier.testTag("files_share_off_confirm")) { Text("Turn off", color = Danger) }
            },
            dismissButton = { TextButton(onClick = { revoking = null }) { Text("Cancel") } },
        )
    }
}
