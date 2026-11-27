package com.allternit.feature.settings

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.DockSpec
import com.allternit.core.design.DockingBlockTitle
import com.allternit.core.design.FilterPills
import com.allternit.core.design.GlassCircleButton
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.PolishHaptic
import com.allternit.core.design.SlidingSegmented
import com.allternit.core.design.TabPage
import com.allternit.core.design.dockDescription
import com.allternit.core.design.rememberDockOffset
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
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * The Catalog (iOS twin), Skills and Plugins (CapabilitiesSettingsView) and Connectors (ConnectorsListView).
 * Skills and plugins are folders on the person's computer, read and written through the runtime's file routes the
 * way iOS does, so they need a computer. Connectors are an account list.
 */

private val SKILL_ROOTS = listOf("~/.allternit/skills", "~/.agents/skills", "~/.codex/skills")
private val PLUGIN_ROOTS = listOf("~/.allternit/plugins", "~/.agents/plugins", "~/.codex/plugins")

/** One installed skill or plugin: a folder with a SKILL.md (skill) and an optional config.json holding `enabled`. */
internal data class Cap(val name: String, val dir: String, val enabled: Boolean, val hasConfig: Boolean)

internal suspend fun runtimeLoad(host: SettingsHost, method: String, path: String, body: String? = null): Load<JsonElement> =
    when (val r = host.backend.runtimeJson(method, path, body)) {
        is RuntimeCall.Ok -> Load.Ready(r.body)
        RuntimeCall.NoComputer -> Load.NoComputer
        is RuntimeCall.Http -> Load.Failed(r.message ?: "Your computer answered ${r.status}.")
        is RuntimeCall.Failed -> Load.Failed(r.message)
    }

private fun JsonObject.isDir(): Boolean =
    bool("isDirectory") ?: bool("is_directory") ?: bool("is_dir") ?: bool("isDir") ?: (str("type") in setOf("directory", "dir", "folder"))

private fun JsonObject.entryName() = str("name") ?: (str("path") ?: "").substringAfterLast('/')

private suspend fun listDir(host: SettingsHost, path: String): Load<List<JsonObject>> =
    when (val r = runtimeLoad(host, "GET", "/api/v1/files/list?path=${LibraryNet.escape(path)}&details=true")) {
        is Load.Ready -> Load.Ready(r.value.listUnder("entries", "files", "items"))
        Load.NoComputer -> Load.NoComputer
        is Load.Failed -> Load.Failed(r.message)
        Load.Loading -> Load.Loading
    }

private suspend fun readEnabled(host: SettingsHost, dir: String): Boolean {
    val r = runtimeLoad(host, "GET", "/api/v1/files/read?path=${LibraryNet.escape("$dir/config.json")}") as? Load.Ready ?: return true
    val text = r.value.obj.str("content") ?: return true
    val config = runCatching { Json.parseToJsonElement(text).obj }.getOrNull()
    return config.bool("enabled") ?: true
}

private suspend fun writeFile(host: SettingsHost, path: String, content: String): Load<JsonElement> =
    runtimeLoad(host, "POST", "/api/v1/files/write", buildJsonObject { put("path", path); put("content", content) }.toString())

/** Skills: every folder holding a SKILL.md under the skill roots (breadth first, a skill's own subfolders aren't searched). Plugins: each folder under the plugin roots. */
internal suspend fun scanCaps(host: SettingsHost, plugins: Boolean): Load<List<Cap>> {
    val found = LinkedHashMap<String, String>() // folder name -> dir
    var anyOk = false
    var failure: Load<List<Cap>>? = null
    var budget = 80
    for (root in if (plugins) PLUGIN_ROOTS else SKILL_ROOTS) {
        val queue = ArrayDeque<Pair<String, Int>>().apply { add(root to 0) }
        while (queue.isNotEmpty() && budget > 0) {
            val (path, depth) = queue.removeFirst()
            budget -= 1
            val entries = when (val l = listDir(host, path)) {
                is Load.Ready -> { anyOk = true; l.value }
                Load.NoComputer -> return Load.NoComputer
                is Load.Failed -> { if (depth == 0) failure = l; continue }
                Load.Loading -> continue
            }
            if (plugins) {
                if (depth == 0) entries.filter { it.isDir() }.forEach { e -> found.putIfAbsent(e.entryName(), e.str("path") ?: "$path/${e.entryName()}") }
                continue
            }
            if (entries.any { !it.isDir() && it.entryName().equals("SKILL.md", ignoreCase = true) }) {
                found.putIfAbsent(path.substringAfterLast('/'), path)
                continue
            }
            if (depth < 4) entries.filter { it.isDir() && !it.entryName().startsWith(".") && it.entryName() != "node_modules" }.forEach { queue.add((it.str("path") ?: "$path/${it.entryName()}") to depth + 1) }
        }
    }
    if (!anyOk) return failure ?: Load.Failed("Couldn't read your computer's ${if (plugins) "plugins" else "skills"} folder.")
    return Load.Ready(found.entries.take(60).map { (name, dir) -> Cap(name, dir, readEnabled(host, dir), true) }.sortedBy { it.name.lowercase() })
}

private fun slugify(v: String) = v.trim().lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-')

/** The Yours list of skills or plugins, with a switch per item, a remove, and (skills) a create form. */
@Composable
internal fun CapabilitiesBody(host: SettingsHost, plugins: Boolean, query: String, allowCreate: Boolean) {
    val scope = rememberCoroutineScope()
    val kind = if (plugins) "plugin" else "skill"
    val (load, reload) = rememberLoad("caps_$kind") { scanCaps(host, plugins) }
    var error by remember { mutableStateOf<String?>(null) }
    var removing by remember { mutableStateOf<Cap?>(null) }
    var creating by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var description by remember { mutableStateOf("") }
    var instructions by remember { mutableStateOf("") }
    LoadView(load, reload, "load your ${kind}s") { caps ->
        val shown = caps.filter { query.isBlank() || it.name.contains(query, ignoreCase = true) }
        if (shown.isEmpty()) StateBox(if (query.isBlank()) "No ${kind}s yet" else "No ${kind}s match", if (plugins) "Plugins you install on your computer show up here." else "Add one from Discover, or write your own.", null, null) else CCard {
            shown.forEachIndexed { i, cap ->
                if (i > 0) SDivider()
                SToggleRow(cap.name, "${kind}_${cap.name}", cap.enabled, detail = cap.dir, onChange = { on ->
                    scope.launch {
                        error = null
                        val current = runtimeLoad(host, "GET", "/api/v1/files/read?path=${LibraryNet.escape(cap.dir + "/config.json")}") as? Load.Ready
                        val old = runCatching { Json.parseToJsonElement(current?.value.obj.str("content") ?: "{}").obj }.getOrNull() ?: JsonObject(emptyMap())
                        val merged = JsonObject(old + ("enabled" to JsonPrimitive(on)))
                        val r = writeFile(host, cap.dir + "/config.json", merged.toString())
                        if (r is Load.Ready) reload() else error = "Couldn't turn ${cap.name} ${if (on) "on" else "off"}. ${(r as? Load.Failed)?.message.orEmpty()}"
                    }
                })
            }
        }
        if (shown.isNotEmpty()) CCard(Modifier.padding(top = 12.dp)) {
            shown.forEachIndexed { i, cap ->
                if (i > 0) SDivider()
                SRow("Remove ${cap.name}", "${kind}_remove_${cap.name}", destructive = true, chevron = false, onClick = { removing = cap })
            }
        }
    }
    if (allowCreate) {
        if (creating) {
            SLabel("New skill")
            SField("Name", name, "skill_name", { name = it })
            SField("What it does", description, "skill_description", { description = it })
            SField("Instructions", instructions, "skill_instructions", { instructions = it }, singleLine = false)
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Create skill", "skill_create", enabled = slugify(name).isNotEmpty() && instructions.isNotBlank()) {
                    scope.launch {
                        error = null
                        val slug = slugify(name)
                        val dir = "${SKILL_ROOTS[0]}/$slug"
                        val exists = runtimeLoad(host, "GET", "/api/v1/files/read?path=${LibraryNet.escape("$dir/SKILL.md")}") is Load.Ready
                        if (exists) { error = "A skill called \"${name.trim()}\" is already on this computer. Pick another name."; return@launch }
                        val md = "---\nname: ${name.trim()}\ndescription: ${description.trim().replace("\n", " ")}\n---\n\n${instructions.trim()}\n"
                        val a = writeFile(host, "$dir/SKILL.md", md)
                        val b = if (a is Load.Ready) writeFile(host, "$dir/config.json", buildJsonObject { put("name", name.trim()); put("description", description.trim()); put("enabled", true) }.toString()) else a
                        if (b is Load.Ready) { creating = false; name = ""; description = ""; instructions = ""; reload() } else error = (b as? Load.Failed)?.message ?: "Connect a computer first."
                    }
                }
                SButton("Cancel", "skill_cancel") { creating = false }
            }
        } else CCard(Modifier.padding(top = 18.dp)) { SRow("Create skill", "skill_new", chevron = false, onClick = { creating = true }) }
    }
    SError(error)
    removing?.let { cap ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { removing = null },
            title = { androidx.compose.material3.Text("Remove ${cap.name}?") },
            text = { androidx.compose.material3.Text("Its folder is deleted from your computer.") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    removing = null
                    scope.launch {
                        val r = runtimeLoad(host, "DELETE", "/api/v1/files/delete?path=${LibraryNet.escape(cap.dir)}")
                        if (r is Load.Ready) reload() else error = (r as? Load.Failed)?.message ?: "Connect a computer first."
                    }
                }) { androidx.compose.material3.Text("Remove", color = Danger) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { removing = null }) { androidx.compose.material3.Text("Cancel") } },
        )
    }
}

/** Settings > Skills and Settings > Plugins. */
@Composable
internal fun CapabilitiesPage(host: SettingsHost, plugins: Boolean, back: () -> Unit) {
    var query by remember { mutableStateOf("") }
    SPage(if (plugins) "Plugins" else "Skills", tag = if (plugins) "settingsPlugins" else "settingsSkills", onBack = back) {
        SNote(if (plugins) "Plugins add tools and commands to your bots. They live on your computer." else "Skills teach your bots how to do a job. They live on your computer.")
        SField("Search", query, "capabilities_search", { query = it })
        if (!plugins) ServerSkillsSection(query)
        CapabilitiesBody(host, plugins, query, allowCreate = !plugins)
    }
}

/** "From your servers": skills your MCP servers and installed apps offer, one switch each (`PUT /api/v1/mcp/skills/prefs`). Hidden with the flag off or when there are none. A failed save puts the switch back and says so. */
@Composable
private fun ServerSkillsSection(query: String) {
    val context = androidx.compose.ui.platform.LocalContext.current
    if (!com.allternit.core.api.FeatureFlags.isOn(context, com.allternit.core.api.FeatureFlags.MCP_SKILLS)) return
    val scope = rememberCoroutineScope()
    var skills by remember { mutableStateOf<List<com.allternit.core.api.ServerSkill>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { skills = com.allternit.feature.chat.ServerSkillsRepository.list(force = true) }
    val rows = skills.filter { query.isBlank() || it.name.contains(query, ignoreCase = true) || it.id.contains(query, ignoreCase = true) || it.description.contains(query, ignoreCase = true) }
    if (rows.isEmpty()) return
    SLabel("From your servers")
    SCard(Modifier.padding(bottom = 12.dp)) {
        rows.forEachIndexed { i, sk ->
            if (i > 0) SDivider()
            SToggleRow(
                title = sk.name, tag = "serverSkill_${sk.id}", checked = sk.enabled,
                detail = if (sk.source == "app") "From the ${sk.from} app" else "From ${sk.from}",
                onChange = { on ->
                    error = null
                    skills = skills.map { if (it.id == sk.id) it.copy(enabled = on) else it }
                    scope.launch {
                        if (!com.allternit.feature.chat.ServerSkillsRepository.setEnabled(sk.id, on)) {
                            skills = skills.map { if (it.id == sk.id) it.copy(enabled = !on) else it }
                            error = "Couldn't save that change for ${sk.name}. Try again."
                        }
                    }
                },
            )
        }
    }
    SError(error)
}

/** The connector list. [onlyConnected] shows only what is connected. */
@Composable
internal fun ConnectorsBody(query: String, onlyConnected: Boolean) {
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    val (load, reload) = rememberLoad("connectorsList") { Net.get("connectors").toLoad("load connectors") }
    var busy by remember { mutableStateOf<String?>(null) }
    var keyFor by remember { mutableStateOf<String?>(null) }
    var apiKey by remember { mutableStateOf("") }
    var confirm by remember { mutableStateOf<String?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // "Who can use this" for a connected connector (Team map, flag `team.map`; iOS ConnectorsListView).
    val teamMapOn = com.allternit.core.api.FeatureFlags.isOn(androidx.compose.ui.platform.LocalContext.current, com.allternit.core.api.FeatureFlags.TEAM_MAP)
    var whoFor by remember { mutableStateOf<Pair<String, String?>?>(null) }
    fun connect(id: String, key: String?) {
        busy = id; error = null; message = null
        scope.launch {
            val body = if (key != null) buildJsonObject { put("api_key", key) } else buildJsonObject {}
            val r = Net.send("POST", "connectors/${LibraryNet.escape(id)}/connect", body)
            if (r is ApiResult.Ok) {
                val o = r.value.obj
                o.str("authorize_url")?.let { openLink(it) }
                message = o.str("message")
                keyFor = null; apiKey = ""
                reload()
            } else error = r.friendly("connect")
            busy = null
        }
    }
    LoadView(load, reload, "load connectors") { body ->
        val rows = body.listUnder("connectors", "items").filter {
            val connected = it["connection"].obj.str("status") == "connected"
            (!onlyConnected || connected) && (query.isBlank() || (it.str("name") ?: "").contains(query, ignoreCase = true) || (it.str("category") ?: "").contains(query, ignoreCase = true))
        }
        if (rows.isEmpty()) StateBox(if (query.isBlank()) (if (onlyConnected) "Nothing connected yet" else "No connectors yet") else "No connectors match", if (onlyConnected) "Connect an app from Discover and it shows up here." else "Try another word.", null, null, if (query.isBlank()) com.allternit.core.design.EmptyArtKind.Sheets else null) else CCard {
            rows.forEachIndexed { i, c ->
                val id = c.str("id") ?: return@forEachIndexed
                val connected = c["connection"].obj.str("status") == "connected"
                val account = c["connection"].obj.str("account")
                if (i > 0) SDivider()
                SRow(c.str("name") ?: id, "connector_$id", detail = listOfNotNull(c.str("description"), account).joinToString(" · ").ifBlank { null }, chevron = false, logoName = c.str("name") ?: id, logoId = c.str("logo") ?: id,
                    enabled = busy == null && (connected || c.bool("connectable") != false),
                    onClick = { if (connected) confirm = id else if (c.str("auth_type") == "api_key") keyFor = id else connect(id, null) },
                    trailing = {
                        Column(horizontalAlignment = Alignment.End) {
                            androidx.compose.material3.Text(if (busy == id) "..." else if (connected) "Connected" else "Connect", color = com.allternit.core.design.AllternitTheme.colors.textSecondary, fontSize = 14.sp)
                            if (connected && teamMapOn) Box(
                                Modifier.heightIn(min = 44.dp).clickable(role = Role.Button) { whoFor = id to c.str("name") }.testTag("connectorWho_$id"),
                                contentAlignment = Alignment.CenterEnd,
                            ) {
                                androidx.compose.material3.Text("Who can use this", color = com.allternit.core.design.AllternitTheme.colors.accentPrimary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                            }
                        }
                    })
            }
        }
    }
    message?.let { SNote(it) }
    SError(error)
    whoFor?.let { (id, name) -> com.allternit.feature.bots.TeamWhoCanUseHost("connector", id, name, onDismiss = { whoFor = null }) }
    keyFor?.let { id ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { keyFor = null; apiKey = "" },
            title = { androidx.compose.material3.Text("Enter the API key") },
            text = { SField("API key", apiKey, "connector_apiKey", { apiKey = it }) },
            confirmButton = { androidx.compose.material3.TextButton(enabled = apiKey.isNotBlank(), onClick = { connect(id, apiKey.trim()) }) { androidx.compose.material3.Text("Connect") } },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { keyFor = null; apiKey = "" }) { androidx.compose.material3.Text("Cancel") } },
        )
    }
    confirm?.let { id ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirm = null },
            title = { androidx.compose.material3.Text("Disconnect?") },
            text = { androidx.compose.material3.Text("Your bots stop being able to use it.") },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    confirm = null
                    scope.launch {
                        val r = Net.send("DELETE", "connectors/${LibraryNet.escape(id)}/disconnect")
                        if (r is ApiResult.Ok) reload() else error = r.friendly("disconnect")
                    }
                }) { androidx.compose.material3.Text("Disconnect", color = Danger) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { confirm = null }) { androidx.compose.material3.Text("Cancel") } },
        )
    }
}

/** iOS ConnectorsListView: the + sheet's Connectors row, Settings > Connectors and the Catalog's connector rows all land here. */
@Composable
fun ConnectorsRoot(onBack: () -> Unit) {
    var query by remember { mutableStateOf("") }
    BackHandler(onBack = onBack)
    SPage("Connectors", tag = "connectorsScreen", onBack = onBack) {
        SNote("Connect apps like Gmail, Calendar or GitHub so your bots can use them. You choose what each one can do.")
        SField("Search connectors", query, "connectors_search", { query = it })
        ConnectorsBody(query, onlyConnected = false)
        if (query.isBlank()) UseAllternitInOtherApps()
    }
}

private val CATALOG_TABS = listOf("All", "Connectors", "Skills")

/**
 * The Catalog (iOS twin; settings section id stays `customize`): search, All / Connectors / Skills, an Interactive filter, and the sections
 * Your connections, Most used, Trending, New and Categories. A row opens [CatalogDetail]. Skills and plugins already on the computer, and the
 * More group (MCP servers, Webhooks, Sources, Publish), stay below the list. Polished: the docking block title, search on the neutral fill,
 * and tabs whose underline slides.
 */
@Composable
fun CustomizeRoot(host: SettingsHost, onBack: () -> Unit) {
    var tab by remember { mutableIntStateOf(0) }
    var query by remember { mutableStateOf("") }
    var interactive by remember { mutableStateOf(false) }
    var category by remember { mutableStateOf<String?>(null) }
    var cats by remember { mutableStateOf<List<String>>(emptyList()) }
    var open by remember { mutableStateOf<CatEntry?>(null) }
    val scroll = rememberScrollState()
    var more by remember { mutableStateOf<String?>(null) }
    val (load, reload) = rememberLoad(Triple(query.trim(), category, interactive)) {
        if (query.isNotBlank()) kotlinx.coroutines.delay(250) // a pause in typing, not a request per key
        CatalogClient.list(query, category, interactive)
    }
    val (yours, _) = rememberLoad("catalogYours") { Net.get("connectors").toLoad("load connectors") }
    LaunchedEffect(load) {
        if (category == null && !interactive && query.isBlank()) (load as? Load.Ready)?.let { r -> cats = r.value.flatMap { it.categories }.distinct().sorted() }
    }
    BackHandler { if (open != null) open = null else if (more != null) more = null else onBack() }
    open?.let { CatalogDetail(host, it) { open = null }; return }
    when (more) {
        "mcp" -> { McpPage(host) { more = null }; return }
        "webhooks" -> { WebhooksPage(host) { more = null }; return }
        "sources" -> { SourcesPage { more = null }; return }
        "skills" -> { CapabilitiesPage(host, plugins = false) { more = null }; return }
        "plugins" -> { CapabilitiesPage(host, plugins = true) { more = null }; return }
        "publish" -> { PublishPage({ more = null }, { more = "plugins" }, host.onClose); return }
    }
    val c = AllternitTheme.colors
    val dockOffset = rememberDockOffset(scroll)
    Box(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("customizeScreen")) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll)) {
            Spacer(Modifier.height(52.dp))
            TabPage(
                description = "Connectors and skills that shape what the agent can do.",
                modifier = Modifier.dockDescription(dockOffset),
                title = { Box(Modifier.padding(top = 4.dp).height(DockSpec.BH.dp).semantics { heading(); contentDescription = "Catalog" }) },
            ) {}
            Column(Modifier.padding(horizontal = 14.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (host.account == null) {
                    StateBox("Sign in to browse the Catalog", "Connectors and skills belong to your account.", "Sign in", host.onClose)
                } else {
                    CustomizeSearch(query, "Search the Catalog", { query = it }, Modifier.fillMaxWidth())
                    Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        SlidingTabRow(CATALOG_TABS, tab, { tab = it }, Modifier.weight(1f))
                        Box(
                            Modifier.heightIn(min = 44.dp).clickable(role = Role.Switch) { interactive = !interactive }.testTag("catalogInteractive")
                                .semantics { contentDescription = "Interactive only" },
                            contentAlignment = Alignment.Center,
                        ) {
                            Box(Modifier.clip(RoundedCornerShape(50)).background(if (interactive) c.textPrimary else c.fillNeutral).padding(horizontal = 12.dp, vertical = 6.dp)) {
                                androidx.compose.material3.Text("Interactive", color = if (interactive) c.bgPrimary else c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                            }
                        }
                    }
                    CatalogBody(load, reload, tab, query, category, interactive, cats, yours, { category = it }) { open = it }
                    SLabel("On your computer")
                    CCard {
                        SRow("Skills", "catalog_more_skills", icon = DesignR.drawable.ph_plugs_connected_thin, onClick = { more = "skills" })
                        SDivider()
                        SRow("Plugins", "catalog_more_plugins", icon = DesignR.drawable.ph_plugs_connected_thin, onClick = { more = "plugins" })
                    }
                    SLabel("More")
                    CCard {
                        SRow("MCP servers", "customize_more_mcp", icon = DesignR.drawable.ph_plugs_connected_thin, onClick = { more = "mcp" })
                        SDivider()
                        SRow("Webhooks", "customize_more_webhooks", icon = DesignR.drawable.ph_webhooks_logo, onClick = { more = "webhooks" })
                        SDivider()
                        SRow("Sources", "customize_more_sources", icon = DesignR.drawable.ph_plugs_connected_thin, onClick = { more = "sources" })
                        SDivider()
                        SRow("Publish", "customize_more_publish", icon = DesignR.drawable.ph_arrow_square_out, onClick = { more = "publish" })
                    }
                }
            }
        }
        DockingBlockTitle("Catalog", dockOffset, barCenter = 26.dp, modifier = Modifier.fillMaxSize(), top = 56.dp)
        Box(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
            GlassCircleButton(onClick = onBack, contentDescription = "Back", modifier = Modifier.testTag("settingsBack")) {
                Icon(painterResource(DesignR.drawable.ph_caret_left), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
            }
        }
        if (host.account != null) CustomizeIntroHost()
    }
}

/** Sections when nothing is searched or filtered; one Results list when something is. */
@Composable
private fun CatalogBody(
    load: Load<List<CatEntry>>, retry: () -> Unit, tab: Int, query: String, category: String?, interactive: Boolean,
    cats: List<String>, yours: Load<JsonElement>, onCategory: (String?) -> Unit, onOpen: (CatEntry) -> Unit,
) {
    LoadView(load, retry, "load the Catalog") { all ->
        val entries = all.filter { tab == 0 || (tab == 1 && !it.isSkill) || (tab == 2 && it.isSkill) }
        val filtered = query.isNotBlank() || category != null || interactive
        if (!filtered) {
            val mine = (yours as? Load.Ready)?.value.listUnder("connectors", "items").filter { it["connection"].obj.str("status") == "connected" }
            if (tab != 2 && mine.isNotEmpty()) {
                SLabel("Your connections")
                CCard {
                    mine.forEachIndexed { i, m ->
                        val id = m.str("id") ?: return@forEachIndexed
                        if (i > 0) SDivider()
                        val match = all.firstOrNull { it.connectorId == id }
                        SRow(m.str("name") ?: id, "catalogYours_$id", detail = m["connection"].obj.str("account"), chevron = match != null, logoName = m.str("name") ?: id, logoId = m.str("logo") ?: id,
                            onClick = match?.let { e -> { onOpen(e) } })
                    }
                }
            }
            CatalogSection("Most used", entries.sortedByDescending { it.popularity }.take(5), onOpen)
            CatalogSection("Trending", entries.filter { it.trending }.take(5), onOpen)
            CatalogSection("New", entries.filter { it.isNew }.take(5), onOpen)
        }
        SLabel("Categories")
        if (cats.isNotEmpty()) FilterPills(
            options = listOf("All") + cats.map { it.replaceFirstChar { ch -> ch.uppercase() } },
            selectedIndex = if (category == null) 0 else cats.indexOf(category) + 1,
            onSelect = { onCategory(if (it == 0) null else cats[it - 1]) },
            modifier = Modifier.padding(bottom = 8.dp),
        )
        if (filtered) {
            if (entries.isEmpty()) StateBox("Nothing matches", "Try another word or turn a filter off.", null, null, art = com.allternit.core.design.EmptyArtKind.Search)
            else CatalogSection(null, entries, onOpen)
        }
    }
}

@Composable
private fun CatalogSection(title: String?, entries: List<CatEntry>, onOpen: (CatEntry) -> Unit) {
    if (entries.isEmpty()) return
    if (title != null) SLabel(title)
    CCard {
        entries.forEachIndexed { i, e ->
            if (i > 0) SDivider()
            CatalogRow(e) { onOpen(e) }
        }
    }
}

/** Logo tile 32 dp, name with verified, one-line blurb, "Connector · By author", and the Interactive chip. */
@Composable
private fun CatalogRow(e: CatEntry, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(role = Role.Button, onClick = onClick).testTag("catalog_${e.id}").padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        CatalogIcon(e, size = 32.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                androidx.compose.material3.Text(e.name, color = c.textPrimary, fontSize = 16.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (e.verified) androidx.compose.material3.Text("Verified", color = c.textTertiary, fontSize = 11.sp)
            }
            e.blurb?.let { androidx.compose.material3.Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis) }
            androidx.compose.material3.Text(e.kindLine, color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
        }
        if (e.interactive) Box(Modifier.clip(RoundedCornerShape(50)).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 3.dp)) {
            androidx.compose.material3.Text("Interactive", color = c.textSecondary, fontSize = 11.sp)
        }
    }
}
