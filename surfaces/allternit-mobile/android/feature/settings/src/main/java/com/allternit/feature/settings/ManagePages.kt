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
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/* Manage pages: Models (ModelManagementView), Subagents, MCP servers, Webhooks. */

/** The runtime relay as a Load (MCP lives on the person's computer). */
private suspend fun relay(host: SettingsHost, method: String, path: String, body: String? = null): Load<JsonElement> =
    when (val r = host.backend.runtimeJson(method, path, body)) {
        is RuntimeCall.Ok -> Load.Ready(r.body)
        RuntimeCall.NoComputer -> Load.NoComputer
        is RuntimeCall.Http -> Load.Failed(r.message ?: "Your computer answered ${r.status}.")
        is RuntimeCall.Failed -> Load.Failed(r.message)
    }

/** iOS ModelManagementView: the models the runtime lists (`GET models`), grouped by provider, with search. */
@Composable
internal fun ModelsPage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    val (load, reload) = rememberLoad("models") { Net.get("models").toLoad("load your models") }
    var query by remember { mutableStateOf("") }
    SPage("Models", tag = "settingsModels", onBack = back) {
        SNote("The models your bots and chats can use. Connect a subscription or key on Connections to add more.")
        SCard {
            SRow("Browse open models", "models_browse", onClick = host.openModels)
            SDivider()
            SRow("Connect a provider", "models_connect", detail = "Subscriptions, API keys and local brains.", onClick = { push("subscriptions") })
        }
        SField("Search models", query, "models_search", { query = it })
        LoadView(load, reload, "load your models") { body ->
            val rows = body.listUnder("models", "items").filter {
                query.isBlank() || (it.str("name") ?: it.str("id") ?: "").contains(query, ignoreCase = true)
            }
            if (rows.isEmpty()) SNote(if (query.isBlank()) "No models yet. Connect a provider to add some." else "No models match \"$query\".") else {
                val groups = rows.groupBy { it.str("provider") ?: it.str("providerId") ?: (it.str("id") ?: "").substringBefore('/', "Models") }
                for ((provider, models) in groups.toSortedMap()) {
                    SLabel(provider)
                    SCard {
                        models.forEachIndexed { i, m ->
                            if (i > 0) SDivider()
                            SRow(m.str("name") ?: m.str("displayName") ?: m.str("id") ?: "Model", "model_${m.str("id")}", detail = m.str("description"), chevron = false)
                        }
                    }
                }
            }
        }
    }
}

/** iOS SubagentsSettingsView: `GET subagent-types`, make one, edit one, delete a custom one. Built-in types are read only. */
@Composable
internal fun SubagentsPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("subagents") { Net.get("subagent-types").toLoad("load subagents") }
    var editing by remember { mutableStateOf<JsonObject?>(null) }
    var creating by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    if (creating || editing != null) {
        SubagentEditor(editing, onBack = { creating = false; editing = null }, onSaved = { creating = false; editing = null; reload() })
        return
    }
    SPage("Subagents", tag = "settingsSubagents", onBack = back) {
        SNote("Helpers a bot can hand work to. Each has its own instructions.")
        LoadView(load, reload, "load subagents") { body ->
            val rows = body.listUnder("types", "subagent_types", "items")
            if (rows.isEmpty()) SNote("No subagents yet.") else SCard {
                rows.forEachIndexed { i, t ->
                    val slug = t.str("slug") ?: t.str("id") ?: return@forEachIndexed
                    val builtin = t.bool("is_custom") == false
                    if (i > 0) SDivider()
                    SRow(t.str("name") ?: slug, "subagent_$slug", detail = t.str("description"), value = if (builtin) "Built in" else null, onClick = { editing = t })
                }
            }
        }
        SError(error)
        SCard(Modifier.padding(top = 18.dp)) { SRow("New subagent", "subagent_new", chevron = false, onClick = { creating = true }) }
    }
}

@Composable
private fun SubagentEditor(existing: JsonObject?, onBack: () -> Unit, onSaved: () -> Unit) {
    val scope = rememberCoroutineScope()
    val builtin = existing?.bool("is_custom") == false
    var name by remember { mutableStateOf(existing.str("name") ?: "") }
    var description by remember { mutableStateOf(existing.str("description") ?: "") }
    var prompt by remember { mutableStateOf(existing.str("prompt") ?: existing.str("instructions") ?: "") }
    var busy by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val slug = existing.str("slug") ?: existing.str("id") ?: name.trim().lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-')
    SPage(if (existing == null) "New subagent" else (existing.str("name") ?: "Subagent"), tag = "settingsSubagentEditor", onBack = onBack) {
        SField("Name", name, "subagent_name", { name = it }, enabled = !builtin && existing == null)
        SField("What it does", description, "subagent_description", { description = it }, enabled = !builtin)
        SField("Instructions", prompt, "subagent_prompt", { prompt = it }, singleLine = false, enabled = !builtin)
        if (builtin) SNote("Built-in subagents can't be changed.") else {
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton(if (busy) "Saving..." else "Save", "subagent_save", enabled = name.isNotBlank() && slug.isNotBlank() && !busy) {
                    busy = true
                    error = null
                    scope.launch {
                        // iOS SubagentTypeFields: slug (create only), name, description, model_class, tools, max_steps, look, prompt.
                        val body = buildJsonObject {
                            if (existing == null) put("slug", slug)
                            put("name", name.trim()); put("description", description.trim())
                            put("model_class", existing.str("model_class") ?: "small")
                            put("tools", existing?.get("tools") ?: kotlinx.serialization.json.JsonArray(emptyList()))
                            put("max_steps", existing?.get("max_steps") ?: kotlinx.serialization.json.JsonPrimitive(40))
                            put("look", existing?.get("look") ?: buildJsonObject { put("pack", "default"); put("face", "default") })
                            put("prompt", prompt)
                        }
                        val r = if (existing == null) Net.send("POST", "subagent-types", body) else Net.send("PATCH", "subagent-types/${LibraryNet.escape(slug)}", body)
                        if (r is ApiResult.Ok) onSaved() else error = r.friendly("save the subagent")
                        busy = false
                    }
                }
                if (existing != null) SButton("Delete", "subagent_delete", destructive = true, enabled = !busy) { confirmDelete = true }
            }
        }
        SError(error)
        if (confirmDelete) androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { androidx.compose.material3.Text("Delete this subagent?") },
            text = { androidx.compose.material3.Text("Bots can't hand work to it any more.") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    confirmDelete = false
                    scope.launch {
                        val r = Net.send("DELETE", "subagent-types/${LibraryNet.escape(slug)}")
                        if (r is ApiResult.Ok) onSaved() else error = r.friendly("delete the subagent")
                    }
                }) { androidx.compose.material3.Text("Delete", color = Danger) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { confirmDelete = false }) { androidx.compose.material3.Text("Cancel") } },
        )
    }
}

/** iOS McpServersSettingsView: `GET v1/mcp` on the person's computer, Connect and Disconnect per server. */
@Composable
internal fun McpPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("mcp") { relay(host, "GET", "/api/v1/mcp") }
    var busyName by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var adding by remember { mutableStateOf(false) }
    if (adding) {
        AddMcpServer(host, onCancel = { adding = false }, onSaved = { adding = false; reload() })
        return
    }
    SPage("MCP", tag = "settingsMcp", onBack = back) {
        SNote("Servers that give your bots extra tools. They run on your computer.")
        LoadView(load, reload, "load MCP servers") { body ->
            val rows = body.listUnder("servers", "mcp", "items")
            if (rows.isEmpty()) StateBox("No MCP servers", "Add a server to your computer's MCP config and it shows up here.", null, null) else SCard {
                rows.forEachIndexed { i, s ->
                    val name = s.str("name") ?: s.str("id") ?: return@forEachIndexed
                    val status = s.str("status") ?: if (s.bool("connected") == true) "connected" else "disconnected"
                    val connected = status == "connected"
                    if (i > 0) SDivider()
                    SRow(name, "mcp_$name", detail = listOfNotNull(status.replaceFirstChar { it.uppercase() }, s.num("tools")?.let { "${it.toLong()} tools" } ?: s.num("toolCount")?.let { "${it.toLong()} tools" }).joinToString(" · "),
                        chevron = false, enabled = busyName == null, onClick = {
                            busyName = name
                            error = null
                            scope.launch {
                                val action = if (connected) "disconnect" else "connect"
                                val r = relay(host, "POST", "/api/v1/mcp/${LibraryNet.escape(name)}/$action", "{}")
                                if (r is Load.Ready) reload() else error = (r as? Load.Failed)?.message ?: "Connect a computer first."
                                busyName = null
                            }
                        },
                        trailing = { androidx.compose.material3.Text(if (busyName == name) "..." else if (connected) "Disconnect" else "Connect", color = com.allternit.core.design.AllternitTheme.colors.textSecondary, fontSize = 14.sp) })
                }
            }
        }
        SCard(Modifier.padding(top = 18.dp)) { SRow("Add MCP server", "mcp_add", chevron = false, onClick = { adding = true }) }
        SError(error)
    }
}

/** iOS WebhooksSettingsView: `GET webhook-triggers`, make one for a bot, see deliveries, delete. */
@Composable
internal fun WebhooksPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("webhooks") { Net.get("webhook-triggers").toLoad("load webhooks") }
    val (bots, _) = rememberLoad("webhook-bots") { Net.get("agents").toLoad("load your bots") }
    var creating by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var botId by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf<JsonObject?>(null) }
    var deliveries by remember { mutableStateOf<Load<JsonElement>>(Load.Loading) }
    var confirmDelete by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage("Webhooks", tag = "settingsWebhooks", onBack = if (selected != null) ({ selected = null }) else back) {
        val open = selected
        if (open != null) {
            val id = open.str("id") ?: ""
            SCard {
                SRow(open.str("name") ?: "Webhook", "webhook_detail_name", detail = open.str("url"), chevron = false)
            }
            SLabel("Recent deliveries")
            LoadView(deliveries, { selected = open }, "load deliveries") { body ->
                val rows = body.listUnder("deliveries", "items")
                if (rows.isEmpty()) SNote("Nothing has called this webhook yet.") else SCard {
                    rows.forEachIndexed { i, d ->
                        if (i > 0) SDivider()
                        SRow(d.str("status") ?: d.str("result") ?: "Delivery", "webhook_delivery_$i", detail = d.str("created_at") ?: d.str("at"), chevron = false)
                    }
                }
            }
            SCard(Modifier.padding(top = 18.dp)) { SRow("Delete webhook", "webhook_delete", destructive = true, chevron = false, onClick = { confirmDelete = id }) }
            androidx.compose.runtime.LaunchedEffect(id) { deliveries = Net.get("webhook-triggers/${LibraryNet.escape(id)}/deliveries").toLoad("load deliveries") }
        } else {
            SNote("A webhook is a URL an outside service can call to start one of your bots.")
            LoadView(load, reload, "load webhooks") { body ->
                val rows = body.listUnder("triggers", "webhooks", "items")
                if (rows.isEmpty()) SNote("No webhooks yet.") else SCard {
                    rows.forEachIndexed { i, t ->
                        val id = t.str("id") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        SRow(t.str("name") ?: id, "webhook_$id", detail = "/webhooks/inbound/$id", onClick = { selected = t })
                    }
                }
            }
            if (creating) {
                SLabel("New webhook")
                SField("Name", name, "webhook_name", { name = it })
                LoadView(bots, {}, "load your bots") { b ->
                    val list = b.listUnder("agents", "bots", "items")
                    SCard(Modifier.padding(top = 8.dp)) {
                        list.forEachIndexed { i, bot ->
                            val id = bot.str("id") ?: return@forEachIndexed
                            if (i > 0) SDivider()
                            SRow(bot.str("name") ?: id, "webhook_bot_$id", chevron = false, onClick = { botId = id }, trailing = { if (botId == id) CheckMark() })
                        }
                    }
                }
                Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    SButton("Create", "webhook_create", enabled = name.isNotBlank() && botId != null) {
                        scope.launch {
                            val r = Net.send("POST", "webhook-triggers", buildJsonObject { put("name", name.trim()); put("target_bot_id", botId ?: "") })
                            if (r is ApiResult.Ok) { creating = false; name = ""; botId = null; message = (r.value.obj["trigger"] as? JsonObject)?.str("id")?.let { "Created. Inbound URL: /webhooks/inbound/$it" } ?: "Created."; reload() } else error = r.friendly("create the webhook")
                        }
                    }
                    SButton("Cancel", "webhook_cancel") { creating = false }
                }
            } else {
                SCard(Modifier.padding(top = 18.dp)) { SRow("New webhook", "webhook_new", chevron = false, onClick = { creating = true }) }
            }
            message?.let { SNote(it) }
            SError(error)
        }
        confirmDelete?.let { id ->
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { confirmDelete = null },
                title = { androidx.compose.material3.Text("Delete this webhook?") },
                text = { androidx.compose.material3.Text("Its URL stops working right away.") },
                confirmButton = {
                    androidx.compose.material3.TextButton(onClick = {
                        confirmDelete = null
                        scope.launch {
                            val r = Net.send("DELETE", "webhook-triggers/${LibraryNet.escape(id)}")
                            if (r is ApiResult.Ok) { selected = null; reload() } else error = r.friendly("delete the webhook")
                        }
                    }) { androidx.compose.material3.Text("Delete", color = Danger) }
                },
                dismissButton = { androidx.compose.material3.TextButton(onClick = { confirmDelete = null }) { androidx.compose.material3.Text("Cancel") } },
            )
        }
    }
}
