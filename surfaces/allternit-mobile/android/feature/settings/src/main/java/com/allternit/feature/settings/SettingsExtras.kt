package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.artifacts.num
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/* Gap run C settings leftovers: the execution-mode switch (iOS RuntimeOperationsView) and the node join token (iOS NodesManagerView). */

private val MODE_COPY = mapOf(
    "plan" to ("Plan rail" to "Generate and inspect a plan before runtime changes are allowed."),
    "safe" to ("Safe rail" to "Require runtime rails and policy checks before direct execution."),
    "auto" to ("Auto rail" to "Execute directly when the runtime and rails permit the request."),
)

/** `GET/POST /runtime/execution-mode` on the person's computer (iOS RuntimeExecutionModeStatus: mode, supported_modes). */
@Composable
internal fun ExecutionModeCard(host: SettingsHost) {
    val scope = rememberCoroutineScope()
    var mode by remember { mutableStateOf<String?>(null) }
    var supported by remember { mutableStateOf(listOf("plan", "safe", "auto")) }
    var state by remember { mutableStateOf("loading") }
    var error by remember { mutableStateOf<String?>(null) }
    fun apply(r: RuntimeCall) {
        when (r) {
            is RuntimeCall.Ok -> {
                val o: JsonObject = r.body.obj
                mode = o.str("mode") ?: mode
                (o["supported_modes"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }?.takeIf { it.isNotEmpty() }?.let { supported = it }
                state = "ready"
                error = null
            }
            RuntimeCall.NoComputer -> state = "nocomputer"
            is RuntimeCall.Http -> { state = "ready"; error = r.message ?: "Your computer answered ${r.status}." }
            is RuntimeCall.Failed -> { state = "ready"; error = r.message }
        }
    }
    androidx.compose.runtime.LaunchedEffect(Unit) { apply(host.backend.runtimeJson("GET", "/runtime/execution-mode", null)) }
    SLabel("Execution mode")
    when (state) {
        "loading" -> SNote("Waiting for the runtime to report the shared execution default.")
        "nocomputer" -> SNote("Start a computer to choose how your bots run.")
        else -> {
            SCard {
                supported.forEachIndexed { i, m ->
                    if (i > 0) SDivider()
                    val (title, detail) = MODE_COPY[m] ?: (m.replaceFirstChar { it.uppercase() } to "")
                    SRow(title, "runtime_mode_$m", detail = detail.ifBlank { null }, chevron = false, trailing = { if (mode == m) CheckMark() }, onClick = {
                        scope.launch { apply(host.backend.runtimeJson("POST", "/runtime/execution-mode", buildJsonObject { put("mode", m) }.toString())) }
                    })
                }
            }
            SError(error)
        }
    }
}

/** `POST nodes/token` (iOS NodeTokenResponse: node_id, token, install_command): the command to run on the machine that should join. */
@Composable
internal fun NodeJoinToken() {
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    var command by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    SLabel("Join token")
    SNote("Make a token, then run the command on the machine you want to connect. The token works once.")
    Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SButton(if (busy) "Making..." else "Make a join token", "nodes_token", enabled = !busy) {
            busy = true
            error = null
            scope.launch {
                val r = Net.send("POST", "nodes/token", buildJsonObject { })
                if (r is ApiResult.Ok) command = r.value.obj.str("install_command") ?: r.value.obj.str("token") else error = r.friendly("make a join token")
                busy = false
            }
        }
        command?.let { c -> SButton("Copy command", "nodes_token_copy") { clipboard.setText(AnnotatedString(c)) } }
    }
    command?.let { SNote("Run this on the node you want to connect:\n\n$it") }
    SError(error)
}

/** Accounts card: one row per organization the person belongs to; tap a different one to switch (iOS SettingsView.switchAccount). */
@Composable
internal fun OrgSwitcher(activeOrgId: String?) {
    val scope = rememberCoroutineScope()
    var orgs by remember { mutableStateOf<List<com.allternit.core.auth.AllternitAuth.OrgMembership>>(emptyList()) }
    var active by remember(activeOrgId) { mutableStateOf(activeOrgId) }
    var switching by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    androidx.compose.runtime.LaunchedEffect(Unit) { orgs = com.allternit.core.auth.AllternitAuth.organizations() }
    if (orgs.size > 1) {
        orgs.forEach { o ->
            SDivider()
            SRow(o.name, "settingsOrg_${o.id}", detail = if (switching == o.id) "Switching..." else null, chevron = false,
                trailing = { if (active == o.id) CheckMark() },
                onClick = {
                    if (active != o.id && switching == null) {
                        switching = o.id
                        error = null
                        scope.launch {
                            val r = com.allternit.core.auth.AllternitAuth.switchOrganization(o.id)
                            if (r.ok) active = o.id else error = "Couldn't switch accounts. ${r.error ?: ""}".trim()
                            switching = null
                        }
                    }
                })
        }
    }
    error?.let { SError(it) }
}

/** iOS McpServerEditorView (new server): saves `~/.allternit/mcps/<slug>.json` on the person's computer through the files API. */
@Composable
internal fun AddMcpServer(host: SettingsHost, onCancel: () -> Unit, onSaved: () -> Unit) {
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf("") }
    var description by remember { mutableStateOf("") }
    var command by remember { mutableStateOf("") }
    var argsText by remember { mutableStateOf("") }
    var tagsText by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val slug = name.lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-').ifEmpty { "item" }.take(48)
    val path = "~/.allternit/mcps/$slug.json"
    SPage("Add MCP server", tag = "settingsMcpAdd", onBack = onCancel) {
        SField("Name", name, "mcp_name", { name = it })
        SField("What it does (optional)", description, "mcp_description", { description = it }, singleLine = false)
        if (name.isNotBlank()) SNote("Saved as $path on your computer.")
        SField("Command (for example npx)", command, "mcp_command", { command = it })
        SField("Arguments, one per line", argsText, "mcp_args", { argsText = it }, singleLine = false)
        SField("Tags, separated by commas", tagsText, "mcp_tags", { tagsText = it })
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SButton(if (busy) "Saving..." else "Save", "mcp_save", enabled = name.isNotBlank() && command.isNotBlank() && !busy) {
                busy = true
                error = null
                scope.launch {
                    val exists = host.backend.runtimeJson("GET", "/api/v1/files/read?path=${com.allternit.feature.artifacts.LibraryNet.escape(path)}")
                    if (exists is RuntimeCall.NoComputer) { error = "Connect a computer first."; busy = false; return@launch }
                    if (exists is RuntimeCall.Ok && exists.body.obj.str("content") != null) {
                        error = "A server file named $slug.json already exists. Pick another name."; busy = false; return@launch
                    }
                    val now = java.time.Instant.now().toString()
                    val config = buildJsonObject {
                        put("name", name.trim()); put("description", description.trim()); put("command", command.trim())
                        put("args", JsonArray(argsText.lines().map { it.trim() }.filter { it.isNotEmpty() }.map { JsonPrimitive(it) }))
                        put("tags", JsonArray(tagsText.split(",").map { it.trim() }.filter { it.isNotEmpty() }.map { JsonPrimitive(it) }))
                        put("enabled", true); put("version", "1.0.0"); put("author", "User"); put("createdAt", now); put("updatedAt", now)
                    }
                    val body = buildJsonObject { put("path", path); put("content", config.toString()) }.toString()
                    when (val r = host.backend.runtimeJson("POST", "/api/v1/files/write", body)) {
                        is RuntimeCall.Ok -> onSaved()
                        RuntimeCall.NoComputer -> error = "Connect a computer first."
                        is RuntimeCall.Http -> error = r.message ?: "Your computer answered ${r.status}."
                        is RuntimeCall.Failed -> error = r.message
                    }
                    busy = false
                }
            }
            SButton("Cancel", "mcp_cancel") { onCancel() }
        }
        SError(error)
    }
}

/** Memory entry lines `- text [source: ...; added: ...; id: ...]` (iOS MemoryDriveView.entries(in:)). */
internal data class MemoryEntry(val index: Int, val text: String, val source: String, val added: String, val entryId: String?, val conflictOf: String?) {
    val resolvedId: String
        get() = entryId ?: run {
            val d = java.security.MessageDigest.getInstance("SHA-256").digest("$text\u0000$source\u0000$added".toByteArray())
            "entry-" + d.joinToString("") { "%02x".format(it) }
        }
}

internal object MemoryEntries {
    fun parse(content: String): List<MemoryEntry> {
        val out = mutableListOf<MemoryEntry>()
        for (line in content.split("\n")) {
            if (!line.startsWith("- ") || line.startsWith("- [[") || !line.endsWith("]")) continue
            val at = line.lastIndexOf(" [")
            if (at < 2) continue
            val text = line.substring(2, at)
            val meta = mutableMapOf<String, String>()
            for (part in line.substring(at + 2, line.length - 1).split("; ")) {
                val sep = part.indexOf(": ")
                if (sep > 0) meta[part.substring(0, sep)] = part.substring(sep + 2)
            }
            val source = meta["source"] ?: continue
            val added = meta["added"] ?: continue
            out += MemoryEntry(out.size, text, source, added, meta["id"], meta["conflict_of"])
        }
        return out
    }
}

/** iOS memory purge: pick entries, then `POST memory/drive/purge` removes them from the drive and its history. */
@Composable
internal fun PurgeMemories(drive: String?, content: String, revision: String, onDone: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    val entries = remember(content) { MemoryEntries.parse(content).filter { it.conflictOf == null } }
    var chosen by remember { mutableStateOf(setOf<Int>()) }
    var confirm by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    SNote("Pick the memories to delete. They are removed from this file and from its history. This can't be undone.")
    SCard {
        entries.forEachIndexed { i, e ->
            if (i > 0) SDivider()
            SToggleRow(e.text, "purge_entry_${e.index}", e.index in chosen, detail = "${e.source} · ${e.added}",
                onChange = { on -> chosen = if (on) chosen + e.index else chosen - e.index })
        }
    }
    SButton(if (busy) "Deleting..." else "Delete ${chosen.size} forever", "purge_confirm", destructive = true, enabled = chosen.isNotEmpty() && !busy) { confirm = true }
    SError(error)
    if (confirm) androidx.compose.material3.AlertDialog(
        onDismissRequest = { confirm = false },
        title = { androidx.compose.material3.Text("Delete ${chosen.size} forever?") },
        text = { androidx.compose.material3.Text("They are removed from the drive and its history.") },
        confirmButton = {
            androidx.compose.material3.TextButton(onClick = {
                confirm = false
                busy = true
                scope.launch {
                    val ids = entries.filter { it.index in chosen }.map { it.resolvedId }
                    val body = buildJsonObject {
                        if (!drive.isNullOrBlank() && drive != "personal") put("drive", drive)
                        put("expected_revision", revision)
                        put("entry_ids", JsonArray(ids.map { JsonPrimitive(it) }))
                        put("confirm", "delete forever")
                    }
                    val r = Net.send("POST", "memory/drive/purge", body)
                    if (r is ApiResult.Ok) {
                        val n = r.value.obj.num("purged")?.toInt() ?: ids.size
                        onDone(if (n == 1) "Deleted 1 memory from the drive and its history." else "Deleted $n memories from the drive and its history.")
                    } else {
                        error = if (r is ApiResult.HttpError && r.status == 409) "This file changed since you opened it. Go back and try again." else r.friendly("delete those memories")
                    }
                    busy = false
                }
            }) { androidx.compose.material3.Text("Delete", color = Danger) }
        },
        dismissButton = { androidx.compose.material3.TextButton(onClick = { confirm = false }) { androidx.compose.material3.Text("Cancel") } },
    )
}

/** iOS CloudComputerBackupsView "Download a copy": `POST <computer>/exports`, poll every 5 s until ready, then open the download link. */
@Composable
internal fun ComputerExportRow(base: String) {
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    var status by remember { mutableStateOf<String?>(null) }
    var path by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    SLabel("Copy")
    SNote("Download everything on this computer as one file. Nothing is changed on the computer.")
    when {
        status == "ready" && path != null -> SButton("Download the copy", "cloudBackups_downloadCopy") {
            openLink(com.allternit.feature.artifacts.LibraryNet.config.apiBaseUrl.resolve(path ?: "")?.toString() ?: (path ?: ""))
        }
        busy || status == "preparing" -> SNote("Preparing a copy... this can take a few minutes.")
        else -> SButton("Download a copy", "cloudBackups_export") {
            busy = true
            error = null
            scope.launch {
                var r = Net.send("POST", "$base/exports", buildJsonObject { })
                var export = (r as? ApiResult.Ok)?.value?.obj?.get("export") as? JsonObject
                if (export == null) { error = r.friendly("start the copy"); busy = false; return@launch }
                while (export?.str("status") == "preparing") {
                    kotlinx.coroutines.delay(5000)
                    r = Net.get("$base/exports/${com.allternit.feature.artifacts.LibraryNet.escape(export?.str("id") ?: "")}")
                    export = (r as? ApiResult.Ok)?.value?.obj?.get("export") as? JsonObject ?: break
                }
                status = export?.str("status")
                path = export?.str("download_path")
                if (status == "failed") error = export?.str("error") ?: "The last copy failed."
                busy = false
            }
        }
    }
    SError(error)
}
