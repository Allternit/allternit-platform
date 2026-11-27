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
 * Customize (iOS CustomizeScreen), Skills and Plugins (CapabilitiesSettingsView) and Connectors (ConnectorsListView).
 * Skills and plugins are folders on the person's computer, read and written through the runtime's file routes the
 * way iOS does, so they need a computer. Connectors are an account list.
 */

private val SKILL_ROOTS = listOf("~/.allternit/skills", "~/.agents/skills", "~/.codex/skills")
private val PLUGIN_ROOTS = listOf("~/.allternit/plugins", "~/.agents/plugins", "~/.codex/plugins")

/** One installed skill or plugin: a folder with a SKILL.md (skill) and an optional config.json holding `enabled`. */
internal data class Cap(val name: String, val dir: String, val enabled: Boolean, val hasConfig: Boolean)

private suspend fun runtimeLoad(host: SettingsHost, method: String, path: String, body: String? = null): Load<JsonElement> =
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

private class CatalogSkill(val id: String, val name: String, val blurb: String, val category: String, val author: String)

/** iOS SkillCatalog.entries: the skills directory that Add installs onto the computer (`POST skills/catalog/install`). */
private val CATALOG = listOf(
    CatalogSkill("claude-algorithmic-art", "Algorithmic Art", "Make generative art with p5.js.", "create", "Anthropic"),
    CatalogSkill("claude-brand-guidelines", "Brand Guidelines", "Apply brand colors and type to your work.", "create", "Anthropic"),
    CatalogSkill("claude-canvas-design", "Canvas Design", "Visual art as .png and .pdf documents.", "create", "Anthropic"),
    CatalogSkill("claude-claude-api", "Claude API", "Build, debug and tune Claude API apps.", "build", "Anthropic"),
    CatalogSkill("claude-doc-coauthoring", "Doc Co-Authoring", "A structured workflow for writing docs together.", "productivity", "Anthropic"),
    CatalogSkill("claude-docx", "DOCX", "Create and edit Word documents.", "productivity", "Anthropic"),
    CatalogSkill("claude-frontend-design", "Frontend Design", "Distinctive, production-grade frontend interfaces.", "create", "Anthropic"),
    CatalogSkill("claude-internal-comms", "Internal Comms", "Write all kinds of internal communications.", "productivity", "Anthropic"),
    CatalogSkill("claude-mcp-builder", "MCP Builder", "Build high-quality MCP servers.", "build", "Anthropic"),
    CatalogSkill("claude-pdf", "PDF", "Read, make and edit PDF files.", "productivity", "Anthropic"),
    CatalogSkill("claude-pptx", "PPTX", "Create and edit slide decks.", "productivity", "Anthropic"),
    CatalogSkill("claude-skill-creator", "Skill Creator", "Create and improve skills.", "build", "Anthropic"),
    CatalogSkill("claude-slack-gif-creator", "Slack GIF Creator", "Animated GIFs sized for Slack.", "create", "Anthropic"),
    CatalogSkill("claude-theme-factory", "Theme Factory", "Style artifacts with a theme.", "create", "Anthropic"),
    CatalogSkill("claude-web-artifacts-builder", "Web Artifacts Builder", "Build multi-component web artifacts.", "create", "Anthropic"),
    CatalogSkill("claude-webapp-testing", "Web App Testing", "Test local web apps.", "build", "Anthropic"),
    CatalogSkill("claude-xlsx", "XLSX", "Work with spreadsheets.", "productivity", "Anthropic"),
    CatalogSkill("allternit-alabs-course-pipeline", "ALabs Course Pipeline", "End-to-end A://Labs course creation.", "productivity", "Allternit"),
    CatalogSkill("allternit-allternit-codebase-to-course", "Allternit Codebase To Course", "Turn a codebase into an interactive course.", "productivity", "Allternit"),
    CatalogSkill("codex-clone-website", "Clone Website", "Rebuild a public website as a pixel copy.", "build", "Codex"),
    CatalogSkill("codex-mobile-app-design", "Mobile App Design", "Mobile app UI and UX design.", "create", "Codex"),
)

@Composable
private fun SkillDirectory(host: SettingsHost, query: String) {
    val scope = rememberCoroutineScope()
    val (installed, reload) = rememberLoad("catalogInstalled") { runtimeLoad(host, "GET", "/api/v1/skills/catalog/installed") }
    var busy by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var category by remember { mutableStateOf<String?>(null) }
    val cats = listOf("all", "create", "build", "productivity")
    fun install(id: String) {
        busy = id; error = null
        scope.launch {
            val r = runtimeLoad(host, "POST", "/api/v1/skills/catalog/install", buildJsonObject { put("id", id) }.toString())
            if (r is Load.Ready) reload() else error = (r as? Load.Failed)?.message ?: "Connect a computer first."
            busy = null
        }
    }
    SLabel("Categories")
    FilterPills(
        options = cats.map { it.replaceFirstChar { ch -> ch.uppercase() } },
        selectedIndex = cats.indexOf(category ?: "all").coerceAtLeast(0),
        onSelect = { category = if (it == 0) null else cats[it] },
        modifier = Modifier.padding(bottom = 8.dp),
    )
    LoadView(installed, reload, "load the skills directory") { body ->
        val have = body.listUnder("skills").mapNotNull { it.str("id") }.toSet()
        val rows = CATALOG
            .filter { (category == null || it.category == category) && (query.isBlank() || it.name.contains(query, ignoreCase = true) || it.blurb.contains(query, ignoreCase = true)) }
        if (rows.isEmpty()) StateBox("No skills match", "Try another word or category.", null, null, art = com.allternit.core.design.EmptyArtKind.Search) else {
            CCard(Modifier.padding(top = 8.dp)) {
                rows.forEachIndexed { i, s ->
                    if (i > 0) SDivider()
                    val added = s.id in have
                    SRow(s.name, "directory_${s.id}", detail = "${s.blurb} · ${s.author}", chevron = false, enabled = !added && busy == null, onClick = { install(s.id) }, trailing = {
                        GlassCircleButton(
                            onClick = { if (!added && busy == null) install(s.id) },
                            contentDescription = if (added) "${s.name} added" else "Add ${s.name}",
                            size = 28.dp,
                        ) {
                            if (busy == s.id) androidx.compose.material3.Text("...", color = AllternitTheme.colors.textPrimary, fontSize = 11.sp)
                            else Icon(painterResource(if (added) DesignR.drawable.ph_check_bold else DesignR.drawable.ph_plus_bold), contentDescription = null, tint = AllternitTheme.colors.textPrimary, modifier = Modifier.size(13.dp))
                        }
                    })
                }
            }
        }
    }
    SError(error)
}

/** The connector list. [onlyConnected] is the Customize "Yours" view. */
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

/** iOS ConnectorsListView: the + sheet's Connectors row, Settings > Connectors and Customize's Connectors all land here. */
@Composable
fun ConnectorsRoot(onBack: () -> Unit) {
    var query by remember { mutableStateOf("") }
    BackHandler(onBack = onBack)
    SPage("Connectors", tag = "connectorsScreen", onBack = onBack) {
        SNote("Connect apps like Gmail, Calendar or GitHub so your bots can use them. You choose what each one can do.")
        SField("Search connectors", query, "connectors_search", { query = it })
        ConnectorsBody(query, onlyConnected = false)
    }
}

private val CUSTOMIZE_TABS = listOf("Skills", "Connectors", "Plugins")

/**
 * iOS CustomizeScreen (section: customize): Skills, Connectors and Plugins, each with Yours and Discover, a search
 * field, and the More group (MCP servers, Webhooks, Sources, Publish). The drawer's Customize row and Settings > Customize open it.
 * Polished: the docking block title, search on the neutral fill, and tabs whose underline slides.
 */
@Composable
fun CustomizeRoot(host: SettingsHost, onBack: () -> Unit) {
    var tab by remember { mutableIntStateOf(0) }
    var discover by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    val scroll = rememberScrollState()
    var more by remember { mutableStateOf<String?>(null) }
    BackHandler { if (more != null) more = null else onBack() }
    when (more) {
        "mcp" -> { McpPage(host) { more = null }; return }
        "webhooks" -> { WebhooksPage(host) { more = null }; return }
        "sources" -> { SourcesPage { more = null }; return }
        "publish" -> { PublishPage({ more = null }, { more = null; tab = 2 }, host.onClose); return }
    }
        val c = AllternitTheme.colors
    val dockOffset = rememberDockOffset(scroll)
    Box(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("customizeScreen")) {
        Column(Modifier.fillMaxSize().verticalScroll(scroll)) {
            Spacer(Modifier.height(52.dp))
            TabPage(
                description = "Skills, connectors, and plugins that shape what the agent can do.",
                modifier = Modifier.dockDescription(dockOffset),
                title = { Box(Modifier.padding(top = 4.dp).height(DockSpec.BH.dp).semantics { heading(); contentDescription = "Customize" }) },
            ) {}
            Column(Modifier.padding(horizontal = 14.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (host.account == null) {
                    StateBox("Sign in to customize", "Skills, connectors and plugins belong to your account.", "Sign in", host.onClose)
                } else {
                    CustomizeSearch(query, "Search ${CUSTOMIZE_TABS[tab].lowercase()}", { query = it }, Modifier.fillMaxWidth())
                    SlidingTabRow(CUSTOMIZE_TABS, tab, { tab = it; query = "" }, Modifier.padding(top = 4.dp))
                    SlidingSegmented(listOf("Yours", "Discover"), if (discover) 1 else 0, { discover = it == 1 })
                    when (tab) {
                        0 -> if (discover) SkillDirectory(host, query) else CapabilitiesBody(host, plugins = false, query = query, allowCreate = true)
                        1 -> ConnectorsBody(query, onlyConnected = !discover)
                        else -> if (discover) StateBox("Plugin marketplaces", "Add a marketplace source and publish plugins from Allternit on the web or iPhone. Plugins you install show up under Yours.", null, null) else CapabilitiesBody(host, plugins = true, query = query, allowCreate = false)
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
        DockingBlockTitle("Customize", dockOffset, barCenter = 26.dp, modifier = Modifier.fillMaxSize(), top = 56.dp)
        Box(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
            GlassCircleButton(onClick = onBack, contentDescription = "Back", modifier = Modifier.testTag("settingsBack")) {
                Icon(painterResource(DesignR.drawable.ph_caret_left), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
            }
        }
        if (host.account != null) CustomizeIntroHost()
    }
}
