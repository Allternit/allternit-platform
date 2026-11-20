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
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * Infrastructure and cloud managers (iOS CloudInstancesManagerView, CloudDeployManagerView, VPSServersManagerView,
 * NodesManagerView, CloudComputerBackupsView, EnterpriseBYOCPanelView). Each is a list from the same API the iOS store
 * calls, a detail with that item's actions, and (where iOS has one) a form. One engine, one spec per screen, so the
 * loading, empty, error and no-computer states are the same everywhere.
 */

/** One input in a create form. [number] sends a JSON number; [secret] is a password field (never logged or echoed). */
internal class RField(val label: String, val key: String, val tag: String, val default: String = "", val number: Boolean = false, val optional: Boolean = false)

/** A button on an item's detail. [confirm] puts a confirmation dialog first (every delete, cancel and restore has one). */
internal class RAction(
    val title: String,
    val tag: String,
    val method: String,
    val path: (JsonObject) -> String,
    val destructive: Boolean = false,
    val confirm: String? = null,
    val show: (JsonObject) -> Boolean = { true },
)

internal class ResourceSpec(
    val title: String,
    val tag: String,
    val note: String,
    /** GET path of the list. */
    val list: String,
    /** True when the path is on cloud-api (`api/v1/...`), false for the gateway (`/api/v1/` through the account token). */
    val cloud: Boolean = false,
    val listKeys: List<String> = listOf("items"),
    val idKeys: List<String> = listOf("id"),
    val titleKeys: List<String> = listOf("name", "id"),
    val detailKeys: List<String> = emptyList(),
    val empty: String,
    val actions: List<RAction> = emptyList(),
    val createTitle: String? = null,
    val createPath: String? = null,
    val createFields: List<RField> = emptyList(),
    val createConstants: Map<String, String> = emptyMap(),
    val createNote: String? = null,
    val secretKeys: Set<String> = emptySet(),
)

private fun JsonObject.first(keys: List<String>): String? = keys.firstNotNullOfOrNull { str(it) }

private suspend fun resourceCall(cloud: Boolean, method: String, path: String, body: JsonElement? = null): ApiResult<JsonElement> = when {
    cloud -> Net.cloud(method, path, body)
    method == "GET" -> Net.get(path)
    else -> Net.send(method, path, body)
}

/** Every simple value on an item, as label / value rows. Nested values are skipped. */
internal fun JsonObject.facts(skip: Set<String>): List<Pair<String, String>> = entries.mapNotNull { (k, v) ->
    if (k in skip || v !is JsonPrimitive || v is JsonNull) null else k.replace('_', ' ').replaceFirstChar { it.uppercase() } to v.content
}

@Composable
internal fun ResourcePage(spec: ResourceSpec, back: () -> Unit) {
    val (load, reload) = rememberLoad(spec.tag) { resourceCall(spec.cloud, "GET", spec.list).toLoad("load ${spec.title.lowercase()}") }
    var selected by remember { mutableStateOf<JsonObject?>(null) }
    var creating by remember { mutableStateOf(false) }
    if (creating && spec.createPath != null) {
        ResourceForm(spec, onBack = { creating = false }, onDone = { creating = false; reload() })
        return
    }
    val open = selected
    if (open != null) {
        ResourceDetail(spec, open, onBack = { selected = null }, onChanged = { selected = null; reload() })
        return
    }
    SPage(spec.title, tag = spec.tag, onBack = back) {
        SNote(spec.note)
        LoadView(load, reload, "load ${spec.title.lowercase()}") { body ->
            val rows = body.listUnder(*spec.listKeys.toTypedArray())
            if (rows.isEmpty()) StateBox("Nothing here yet", spec.empty, null, null) else SCard {
                rows.forEachIndexed { i, item ->
                    val id = item.first(spec.idKeys) ?: "$i"
                    if (i > 0) SDivider()
                    SRow(
                        item.first(spec.titleKeys) ?: id, "${spec.tag}_$id",
                        detail = spec.detailKeys.mapNotNull { item.str(it) }.joinToString(" · ").ifBlank { null },
                        onClick = { selected = item },
                    )
                }
            }
        }
        if (spec.createTitle != null && spec.createPath != null) {
            SCard(Modifier.padding(top = 18.dp)) { SRow(spec.createTitle, "${spec.tag}_new", chevron = false, onClick = { creating = true }) }
        }
        SButton("Refresh", "${spec.tag}_refresh") { reload() }
    }
}

@Composable
private fun ResourceDetail(spec: ResourceSpec, item: JsonObject, onBack: () -> Unit, onChanged: () -> Unit) {
    val scope = rememberCoroutineScope()
    var confirm by remember { mutableStateOf<RAction?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val title = item.first(spec.titleKeys) ?: spec.title
    SPage(title, tag = "${spec.tag}_detail", onBack = onBack) {
        val facts = item.facts(spec.secretKeys)
        if (facts.isNotEmpty()) SCard {
            facts.forEachIndexed { i, (k, v) ->
                if (i > 0) SDivider()
                SRow(k, "${spec.tag}_fact_$i", value = v, chevron = false)
            }
        }
        val shown = spec.actions.filter { it.show(item) }
        if (shown.isNotEmpty()) {
            SLabel("Actions")
            SCard {
                shown.forEachIndexed { i, a ->
                    if (i > 0) SDivider()
                    SRow(a.title, a.tag, destructive = a.destructive, chevron = false, enabled = !busy, onClick = {
                        if (a.confirm != null) confirm = a else scope.launch {
                            busy = true; error = null
                            val r = resourceCall(spec.cloud, a.method, a.path(item))
                            if (r is ApiResult.Ok) onChanged() else error = r.friendly(a.title.lowercase())
                            busy = false
                        }
                    })
                }
            }
        }
        SError(error)
    }
    confirm?.let { a ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirm = null },
            title = { androidx.compose.material3.Text(a.title + "?") },
            text = { androidx.compose.material3.Text(a.confirm ?: "") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    confirm = null
                    scope.launch {
                        busy = true; error = null
                        val r = resourceCall(spec.cloud, a.method, a.path(item))
                        if (r is ApiResult.Ok) onChanged() else error = r.friendly(a.title.lowercase())
                        busy = false
                    }
                }) { androidx.compose.material3.Text(a.title, color = if (a.destructive) Danger else AllternitTheme.colors.textPrimary) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { confirm = null }) { androidx.compose.material3.Text("Cancel") } },
        )
    }
}

@Composable
private fun ResourceForm(spec: ResourceSpec, onBack: () -> Unit, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val values = remember { mutableStateOf(spec.createFields.associate { it.key to it.default }) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage(spec.createTitle ?: "New", tag = "${spec.tag}_form", onBack = onBack) {
        spec.createNote?.let { SNote(it) }
        for (f in spec.createFields) {
            SField(f.label + if (f.optional) " (optional)" else "", values.value[f.key] ?: "", f.tag, { v -> values.value = values.value + (f.key to v) }, singleLine = true)
        }
        val ready = spec.createFields.all { it.optional || !values.value[it.key].isNullOrBlank() }
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SButton(if (busy) "Working..." else "Create", "${spec.tag}_create", enabled = ready && !busy) {
                busy = true; error = null
                scope.launch {
                    val body = buildJsonObject {
                        for ((k, v) in spec.createConstants) put(k, v)
                        for (f in spec.createFields) {
                            val v = values.value[f.key]?.trim().orEmpty()
                            if (v.isEmpty()) continue
                            val n = v.toLongOrNull()
                            if (f.number && n != null) put(f.key, n) else put(f.key, v)
                        }
                    }
                    val r = resourceCall(spec.cloud, "POST", spec.createPath ?: "", body)
                    if (r is ApiResult.Ok) onDone() else error = r.friendly("create this")
                    busy = false
                }
            }
            SButton("Cancel", "${spec.tag}_cancel") { onBack() }
        }
        SError(error)
    }
}

private fun esc(s: String) = LibraryNet.escape(s)
private fun JsonObject.id(): String = first(listOf("id", "name")) ?: ""

private const val WIZ = "api/v1/cloud/wizard/deployments"

internal val CLOUD_INSTANCES = ResourceSpec(
    title = "Cloud instances", tag = "cloudInstances", cloud = true, list = WIZ, listKeys = listOf("deployments", "sessions", "items"),
    titleKeys = listOf("instance_name", "name", "id"), detailKeys = listOf("status", "provider", "step"),
    note = "Servers you registered with Allternit. Advance a setup, bootstrap it, or cancel it.",
    empty = "No registered instances. Register one and it shows up here.",
    actions = listOf(
        RAction("Advance", "cloudInstances_advance", "POST", { "$WIZ/${esc(it.id())}/advance" }),
        RAction("Bootstrap", "cloudInstances_bootstrap", "POST", { "$WIZ/${esc(it.id())}/bootstrap" }, confirm = "This installs Allternit on the server. It can take a few minutes."),
        RAction("Cancel setup", "cloudInstances_cancel", "POST", { "$WIZ/${esc(it.id())}/cancel" }, destructive = true, confirm = "Setup stops. The server isn't touched."),
        RAction("Delete", "cloudInstances_delete", "DELETE", { "$WIZ/${esc(it.id())}" }, destructive = true, confirm = "The registration is removed from Allternit."),
    ),
    createTitle = "Register an instance", createPath = WIZ, createConstants = mapOf("provider" to "manual"),
    createNote = "Your own server, reached over SSH. The key or password is sent to Allternit once and never shown again.",
    createFields = listOf(
        RField("Instance name", "instance_name", "cloudInstances_name", optional = true),
        RField("SSH host", "ssh_host", "cloudInstances_host"),
        RField("SSH port", "ssh_port", "cloudInstances_port", default = "22", number = true),
        RField("SSH username", "ssh_username", "cloudInstances_user", default = "root"),
        RField("SSH private key", "ssh_private_key", "cloudInstances_key", optional = true),
        RField("SSH password", "ssh_password", "cloudInstances_password", optional = true),
    ),
    secretKeys = setOf("ssh_private_key", "ssh_password"),
)

internal val CLOUD_DEPLOY = ResourceSpec(
    title = "Cloud deployments", tag = "cloudDeploy", list = "deployments", listKeys = listOf("deployments", "items"),
    titleKeys = listOf("instance_name", "name", "id"), detailKeys = listOf("status", "provider_id", "region_id"),
    note = "Allternit servers being built at a cloud provider, and the ones already running.",
    empty = "No deployments yet.",
    actions = listOf(
        RAction("Cancel deployment", "cloudDeploy_cancel", "DELETE", { "deployments/${esc(it.id())}/cancel" }, destructive = true,
            confirm = "The build stops. Anything already created at the provider stays there.",
            show = { it.str("status") == "deploying" || it.str("status") == "pending" }),
    ),
    createTitle = "New deployment", createPath = "deployments", createConstants = mapOf("mode" to "manual"),
    createNote = "This asks the provider for a server and can cost money there. Nothing starts until you tap Create.",
    createFields = listOf(
        RField("Instance name", "instance_name", "cloudDeploy_name"),
        RField("Provider", "provider_id", "cloudDeploy_provider", default = "hetzner"),
        RField("Region", "region_id", "cloudDeploy_region"),
        RField("Instance type", "instance_type_id", "cloudDeploy_type"),
        RField("Storage (GB)", "storage_gb", "cloudDeploy_storage", default = "40", number = true),
        RField("SSH host", "ssh_host", "cloudDeploy_host", optional = true),
        RField("SSH username", "ssh_username", "cloudDeploy_user", default = "root", optional = true),
    ),
    secretKeys = setOf("api_token", "ssh_private_key"),
)

internal val VPS_SERVERS = ResourceSpec(
    title = "VPS servers", tag = "vpsServers", list = "ssh-connections", listKeys = listOf("connections", "items"),
    titleKeys = listOf("name", "host", "id"), detailKeys = listOf("host", "status"),
    note = "Servers Allternit reaches over SSH. Connect to start using one, disconnect to let it go.",
    empty = "No servers yet. Add one with its address and a key or password.",
    actions = listOf(
        RAction("Connect", "vpsServers_connect", "POST", { "ssh-connections/${esc(it.id())}/connect" }, show = { it.str("status") != "connected" }),
        RAction("Disconnect", "vpsServers_disconnect", "POST", { "ssh-connections/${esc(it.id())}/disconnect" }, show = { it.str("status") == "connected" }),
        RAction("Delete", "vpsServers_delete", "DELETE", { "ssh-connections/${esc(it.id())}" }, destructive = true, confirm = "Allternit forgets this server and its key."),
    ),
    createTitle = "Add a server", createPath = "ssh-connections", createConstants = mapOf("auth_type" to "key"),
    createNote = "Paste a private key to sign in with a key. The key is sent once and never shown again.",
    createFields = listOf(
        RField("Name", "name", "vpsServers_name"),
        RField("Host", "host", "vpsServers_host"),
        RField("Port", "port", "vpsServers_port", default = "22", number = true),
        RField("Username", "username", "vpsServers_user", default = "root"),
        RField("Private key", "private_key", "vpsServers_key"),
    ),
    secretKeys = setOf("private_key", "password"),
)

internal val NODES = ResourceSpec(
    title = "Nodes", tag = "nodes", list = "nodes", listKeys = listOf("nodes", "items"),
    titleKeys = listOf("name", "hostname", "id"), detailKeys = listOf("status", "hostname"),
    note = "Machines that joined your mesh. Remove one and it has to join again with a new token.",
    empty = "No nodes have joined yet. Make a token below and run the join command on the machine.",
    actions = listOf(RAction("Remove node", "nodes_delete", "DELETE", { "nodes/${esc(it.id())}" }, destructive = true, confirm = "The node leaves your mesh.")),
)

internal val CLOUD_BACKUPS = ResourceSpec(
    title = "Computer backups", tag = "cloudBackups", list = "provisioned-instances", listKeys = listOf("instances", "computers", "items"),
    titleKeys = listOf("name", "id"), detailKeys = listOf("status", "region"),
    note = "Pick a cloud computer to see its restore points.",
    empty = "No cloud computers yet.",
)

internal val BYOC = ResourceSpec(
    title = "Enterprise BYOC", tag = "byoc", list = "cloud-credentials", listKeys = listOf("credentials", "items"),
    titleKeys = listOf("label", "provider", "id"), detailKeys = listOf("provider", "region", "status"),
    note = "Cloud accounts your organization connected, so Allternit can build environments in them.",
    empty = "No provider connections yet.",
    actions = listOf(RAction("Revoke", "byoc_revoke", "DELETE", { "cloud-credentials/${esc(it.id())}" }, destructive = true, confirm = "Allternit can't use this connection any more.")),
    createTitle = "Connect a provider", createPath = "cloud-credentials",
    createNote = "The secret is sent once to Allternit's vault and never shown again.",
    createFields = listOf(
        RField("Provider", "provider", "byoc_provider", default = "aws"),
        RField("Label", "label", "byoc_label"),
        RField("Region", "region", "byoc_region", optional = true),
        RField("External ID", "external_id", "byoc_external", optional = true),
    ),
)

/** Restore points for one cloud computer: list, make one now, restore, delete. Each of the last three asks first. */
@Composable
internal fun CloudBackupsPage(back: () -> Unit) {
    var computer by remember { mutableStateOf<JsonObject?>(null) }
    val c = computer
    if (c == null) { ResourcePicker(CLOUD_BACKUPS, back) { computer = it }; return }
    val base = "provisioned-instances/${esc(c.id())}"
    ResourcePage(
        ResourceSpec(
            title = c.first(listOf("name", "id")) ?: "Backups", tag = "cloudBackups_points", list = "$base/snapshots", listKeys = listOf("snapshots", "restore_points", "items"),
            titleKeys = listOf("name", "id"), detailKeys = listOf("created_at", "status"),
            note = "Restore points for this computer. Restoring replaces what is on it now.",
            empty = "No restore points yet.",
            actions = listOf(
                RAction("Restore", "cloudBackups_restore", "POST", { "$base/snapshots/${esc(it.id())}/restore" }, destructive = true, confirm = "This computer goes back to this restore point. Anything newer is lost."),
                RAction("Delete restore point", "cloudBackups_delete", "DELETE", { "$base/snapshots/${esc(it.id())}" }, destructive = true, confirm = "This restore point is gone for good."),
            ),
            createTitle = "Make a restore point now", createPath = "$base/snapshots",
        ),
        back = { computer = null },
    )
}

@Composable
private fun ResourcePicker(spec: ResourceSpec, back: () -> Unit, onPick: (JsonObject) -> Unit) {
    val (load, reload) = rememberLoad(spec.tag) { resourceCall(spec.cloud, "GET", spec.list).toLoad("load your computers") }
    SPage(spec.title, tag = spec.tag, onBack = back) {
        SNote(spec.note)
        LoadView(load, reload, "load your computers") { body ->
            val rows = body.listUnder(*spec.listKeys.toTypedArray())
            if (rows.isEmpty()) StateBox("Nothing here yet", spec.empty, null, null) else SCard {
                rows.forEachIndexed { i, item ->
                    if (i > 0) SDivider()
                    SRow(item.first(spec.titleKeys) ?: "Computer", "${spec.tag}_${item.id()}", detail = spec.detailKeys.mapNotNull { item.str(it) }.joinToString(" · ").ifBlank { null }, onClick = { onPick(item) })
                }
            }
        }
    }
}
