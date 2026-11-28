package com.allternit.feature.computer

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiResult
import com.allternit.core.api.FeatureFlags
import com.allternit.core.design.AppLogo
import androidx.compose.ui.Alignment
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyBox
import com.allternit.core.design.FilterPills
import com.allternit.core.design.GlassPill
import com.allternit.core.design.ProvideDockScroll
import com.allternit.core.design.EmptyArtKind
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

internal data class McpInstalledApp(
    val appId: String, val connectorId: String, val name: String, val url: String?, val connected: Boolean,
    val permissionMode: String, val logoKey: String?, val readOnly: Boolean = false, val description: String? = null,
) { val host: String? get() = url?.let { runCatching { android.net.Uri.parse(it).host }.getOrNull() } }

internal data class McpEntry(
    val id: String, val name: String, val description: String, val category: String, val serverUrl: String?,
    val connected: Boolean, val official: Boolean, val shelfConnector: Boolean,
)

internal data class McpTool(val effect: String, val name: String, val description: String?)
internal data class McpSubmission(val id: String, val state: String, val name: String, val rejection: String?)

/** iOS APP_PERMISSION_MODE_LABELS. */
internal val MCP_MODES = listOf("always_ask" to "Always ask", "ask_before_changes" to "Ask before changes", "ask_before_important_changes" to "Ask before important changes")
private const val DEFAULT_MODE = "ask_before_changes"
private val REAUTH = setOf("expired", "error", "revoked", "pending", "needs_reauth")

/** iOS ACIMcpAppsModel: `mcp-app-installs`, `mcp/connectors`, the `connectors` catalog and the runtime's `tools/list`. */
internal class McpAppsModel(private val backend: ChatBackend) {
    var apps by mutableStateOf<List<McpInstalledApp>>(emptyList()); private set
    var catalog by mutableStateOf<List<McpEntry>>(emptyList()); private set
    var loaded by mutableStateOf(false); private set
    var failure by mutableStateOf<String?>(null); private set
    var notice by mutableStateOf<String?>(null)
    var signIn by mutableStateOf<String?>(null)

    private fun stripPrefix(id: String) = id.replace(Regex("^(connector:|mcp:|url:)"), "")
    private fun esc(s: String) = LibraryNet.escape(s)

    private fun isConnected(c: JsonObject?): Boolean {
        if (c == null) return false
        if (c.bool("enabled") == false || c.bool("needs_reauth") == true) return false
        return c.str("oauth_status")?.let { it !in REAUTH } ?: true
    }

    suspend fun refresh() {
        val installsR = LibraryNet.get("mcp-app-installs")
        val connectorsR = LibraryNet.get("mcp/connectors")
        val catalogR = LibraryNet.get("connectors")
        failure = if (installsR is ApiResult.Ok) null else installsR.friendly("load your apps")
        (catalogR as? ApiResult.Ok)?.let { r ->
            catalog = r.value.obj.list("connectors").mapNotNull { o ->
                McpEntry(o.str("id") ?: return@mapNotNull null, o.str("name").orEmpty(), o.str("description").orEmpty(), o.str("category") ?: "Other",
                    o.str("base_url"), o.bool("connected") == true, false, true)
            }
        }
        var installs = (installsR as? ApiResult.Ok)?.value.obj.list("items")
        val connectors = (connectorsR as? ApiResult.Ok)?.value.obj.list("connectors").orEmpty()
        // A connector added elsewhere has no install record yet: adopt it (connectorsToAdopt).
        if (installsR is ApiResult.Ok) {
            val known = installs.mapNotNull { it.str("connectorId") }.toSet()
            val orphans = connectors.filter { (it.str("id") ?: "") !in known }
            orphans.forEach { o ->
                LibraryNet.send("PUT", "mcp-app-installs", buildJsonObject {
                    put("appId", "mcp:" + (o.str("name_id") ?: o.str("id").orEmpty())); put("connectorId", o.str("id").orEmpty())
                })
            }
            if (orphans.isNotEmpty()) (LibraryNet.get("mcp-app-installs") as? ApiResult.Ok)?.let { installs = it.value.obj.list("items") }
        }
        val byId = connectors.associateBy { it.str("id") }
        val joined = installs.mapNotNull { rec ->
            val appId = rec.str("appId") ?: return@mapNotNull null
            val cid = rec.str("connectorId").orEmpty()
            val c = byId[cid]
            McpInstalledApp(appId, cid, c.str("name") ?: stripPrefix(appId), c.str("url"), isConnected(c), rec.str("permissionMode") ?: DEFAULT_MODE, c.str("name_id"))
        }.toMutableList()
        val names = joined.map { it.name.lowercase() }.toSet()
        catalog.filter { it.connected && it.name.lowercase() !in names }.forEach {
            joined += McpInstalledApp("connector:${it.id}", "", it.name, it.serverUrl, true, DEFAULT_MODE, it.id, readOnly = true, description = it.description)
        }
        apps = joined
        loaded = true
    }

    suspend fun setPermissionMode(app: McpInstalledApp, mode: String) {
        val before = apps
        apps = apps.map { if (it.appId == app.appId) it.copy(permissionMode = mode) else it }
        val r = LibraryNet.send("PATCH", "mcp-app-installs/${esc(app.appId)}", buildJsonObject { put("permissionMode", mode) })
        if (r !is ApiResult.Ok && r !is ApiResult.Decode) { apps = before; notice = "Couldn’t save that change. Try again." }
    }

    suspend fun tools(connectorId: String): List<McpTool>? {
        val r = backend.runtimeJson("POST", "/api/mcp/apps", buildJsonObject { put("action", "tools/list"); put("connectorId", connectorId) }.toString())
        if (r !is RuntimeCall.Ok) return null
        return r.body.obj.list("tools").map { row ->
            val ann = row["annotations"] as? JsonObject
            val effect = if (ann.bool("readOnlyHint") == true) "reads" else if (ann.bool("destructiveHint") == true) "important" else "changes"
            McpTool(effect, row.str("title") ?: row.str("name").orEmpty(), row.str("description"))
        }
    }

    suspend fun startOAuth(connectorId: String): String? {
        val r = LibraryNet.send("POST", "mcp/connectors/${esc(connectorId)}/oauth/start", buildJsonObject {})
        return (r as? ApiResult.Ok)?.value.obj.str("authorize_url")
    }

    /** install-server.ts: create the connector (or reuse one from an earlier try), record the install, start OAuth. */
    suspend fun installServer(appId: String, name: String, serverUrl: String): String? {
        val nameId = name.lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-')
        val created = LibraryNet.send("POST", "mcp/connectors", buildJsonObject {
            put("name", name); put("name_id", nameId); put("url", serverUrl); put("type", "streamable-http")
        })
        var cid = (created as? ApiResult.Ok)?.value.obj.str("id")
        if (cid == null) {
            cid = ((LibraryNet.get("mcp/connectors") as? ApiResult.Ok)?.value.obj.list("connectors"))?.firstOrNull { it.str("url") == serverUrl }?.str("id")
        }
        if (cid == null) { notice = "Couldn’t add $name. Try again."; return null }
        LibraryNet.send("PUT", "mcp-app-installs", buildJsonObject { put("appId", appId); put("connectorId", cid) })
        val authorize = startOAuth(cid)
        refresh()
        notice = "Added $name. Finish sign-in in the new window."
        return authorize
    }

    suspend fun addByUrl(raw: String): String? {
        val trimmed = raw.trim()
        val host = runCatching { android.net.Uri.parse(trimmed).host }.getOrNull()
        if (host.isNullOrEmpty()) { notice = "Enter a full server address, like https://example.com/mcp."; return null }
        return installServer("url:$host", host, trimmed)
    }

    /** A catalog connector: `POST connectors/:id/connect` answers with the sign-in address. */
    suspend fun connectCatalog(entry: McpEntry): String? {
        val r = LibraryNet.send("POST", "connectors/${esc(entry.id)}/connect", buildJsonObject {})
        val o = (r as? ApiResult.Ok)?.value.obj
        if (r !is ApiResult.Ok && r !is ApiResult.Decode) { notice = "Couldn’t connect ${entry.name}. Try again."; return null }
        o.str("message")?.let { notice = it }
        refresh()
        return o.str("authorize_url")
    }

    suspend fun remove(app: McpInstalledApp) {
        if (app.connectorId.isNotEmpty()) LibraryNet.send("DELETE", "mcp/connectors/${esc(app.connectorId)}")
        val r = LibraryNet.send("DELETE", "mcp-app-installs/${esc(app.appId)}")
        if (r is ApiResult.Ok || r is ApiResult.Decode) refresh() else notice = "Couldn’t remove ${app.name}. Try again."
    }

    suspend fun submissions(): List<McpSubmission>? {
        val r = LibraryNet.get("miniapps/submissions") as? ApiResult.Ok ?: return null
        return r.value.obj.list("items").map { McpSubmission(it.str("id").orEmpty(), it.str("state").orEmpty(), (it["submission"] as? JsonObject).str("name").orEmpty(), it.str("rejectionReason")) }
    }

    /** Discover's list: the catalog plus the official servers the catalog doesn't already have (McpDirectory). */
    fun directory(): List<McpEntry> {
        val have = catalog.map { it.name.lowercase() }.toSet()
        val connectedIds = apps.filter { it.connected }.flatMap { listOf(it.appId, stripPrefix(it.appId)) }.toSet()
        val official = McpOfficialServers.servers.filter { it.name.lowercase() !in have }.map {
            McpEntry("mcp:${it.id}", it.name, "Official ${it.name} MCP server.", it.category, it.url, it.id in connectedIds || "mcp:${it.id}" in connectedIds, true, false)
        }
        return catalog + official
    }
}

private enum class McpTab(val label: String) { Installed("Installed"), Discover("Discover"), Build("Build"), Shelf("Apps") }

/** ACI › MCP Apps (iOS ACIMcpAppsView): Installed, Discover and Build tabs, Add by URL, and the installed app's sheet. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciMcpAppsView(host: AciHost) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val model = remember(host.backend) { McpAppsModel(host.backend) }
    var tab by remember { mutableStateOf(McpTab.Installed) }
    // The Apps tab (shelf + share list) only exists behind the local flag `apps.shelf` (iOS AppsShelfFlag).
    // One list, not two tabs (UI feedback 11-25 D1): the shelf shows under the installed MCP apps.
    val shelfOn = remember(context) { FeatureFlags.isOn(context, FeatureFlags.APPS_SHELF) }
    val tabs = remember { McpTab.entries.filter { it != McpTab.Shelf } }
    var query by remember { mutableStateOf("") }
    var addOpen by remember { mutableStateOf(false) }
    var miniOpen by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf<McpInstalledApp?>(null) }
    var category by remember { mutableStateOf<String?>(null) }
    var limit by remember { mutableStateOf(48) }
    var collection by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(host.signedIn) { if (host.signedIn) model.refresh() }
    LaunchedEffect(model.signIn) { model.signIn?.let { openExternal(context, it); model.signIn = null } }

    val scroll = rememberScrollState()
    // The header lives in the scroll so its title docks as the page scrolls (see the clipping note in the PR).
    ProvideDockScroll(scroll) {
    AciDockPage("MCP Apps", scroll, Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().testTag("aciMcpApps")) {
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(scroll).padding(horizontal = 16.dp).padding(bottom = 8.dp)) {
            AciViewHeader("MCP Apps", "Connect apps and services the agent can use.", bottom = 16.dp)
            AciSearchField(query, "Search apps", { query = it; limit = 48; if (it.isNotEmpty() && (tab == McpTab.Build || tab == McpTab.Shelf)) tab = McpTab.Discover }, "aciMcpSearch")
            FilterPills(
                options = tabs.map { it.label }, selectedIndex = tabs.indexOf(tab).coerceAtLeast(0),
                onSelect = { i -> tab = tabs[i]; category = null; collection = null; limit = 48 },
                modifier = Modifier.padding(vertical = 4.dp), itemTag = { i -> "aciMcpTab-${tabs[i].name}" },
            )
            Row(Modifier.padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AciGlassAction("＋ Add by URL", "aciMcpAdd") { addOpen = true }
                AciGlassAction("Mini Apps", "aciMiniApps") { miniOpen = true }
            }
            model.notice?.let { n ->
                Row(Modifier.fillMaxWidth().padding(vertical = 8.dp).clip(RoundedCornerShape(20.dp)).background(c.fillNeutral).padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Text(n, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.weight(1f).testTag("aciMcpNotice"))
                    Text("Dismiss", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.heightIn(min = 44.dp).clickable { model.notice = null }.padding(8.dp))
                }
            }
            Column(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
            when {
                !host.signedIn -> AciStateCard("Sign in to see your apps", "MCP Apps are saved to your Allternit account.", "Sign in", host.openSignIn)
                tab == McpTab.Installed -> {
                    val q = query.trim().lowercase()
                    val list = model.apps.filter { q.isEmpty() || it.name.lowercase().contains(q) }
                    when {
                        model.failure != null && !model.loaded -> AciStateCard("Couldn’t load your apps", model.failure.orEmpty(), "Retry", { scope.launch { model.refresh() } })
                        !model.loaded -> AciStateCard("Loading…", "Reading your connected apps.")
                        list.isEmpty() -> EmptyBox("No apps yet", "Find one in Discover, or add one by its address.", Modifier.testTag("aciState"), onClick = { tab = McpTab.Discover }, art = EmptyArtKind.Apps)
                        else -> AciRoundedCard {
                            list.forEach { app ->
                                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    AppLogo(app.name, id = app.logoKey ?: app.appId, size = 40.dp)
                                    Box(Modifier.weight(1f)) {
                                        AciListRow(app.name, listOfNotNull(app.host, if (app.connected) "Connected" else "Needs sign-in", MCP_MODES.firstOrNull { it.first == app.permissionMode }?.second).joinToString(" · "),
                                            null, "aciMcpInstalled-${app.appId}") { selected = app }
                                    }
                                }
                                if (!app.connected && app.connectorId.isNotEmpty()) {
                                    Box(Modifier.padding(bottom = 10.dp)) {
                                        AciGlassAction("Reconnect", "aciMcpReconnect-${app.appId}", "Reconnect ${app.name}") { scope.launch { model.signIn = model.startOAuth(app.connectorId) } }
                                    }
                                }
                            }
                        }
                    }
                    if (shelfOn) {
                        Text("Apps", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 24.dp, bottom = 12.dp))
                        AciAppsShelf()
                    }
                }
                tab == McpTab.Discover -> {
                    val all = model.directory()
                    val cats = all.map { it.category }.distinct().sorted()
                    fun add(e: McpEntry) { if (!e.connected) scope.launch { model.signIn = if (e.shelfConnector) model.connectCatalog(e) else model.installServer(e.id, e.name, e.serverUrl.orEmpty()) } }
                    // The landing shelf (iOS DirectoryHero): curated collections above the list until a search, category or collection narrows it.
                    if (query.isBlank() && category == null && collection == null) AciDirectoryHero(all, { add(it) }, { collection = it; limit = 48 })
                    mcpCollections.firstOrNull { it.id == collection }?.let { col -> AciGlassAction("${col.title}  ✕", "aciMcpCollectionClear", "Show every app") { collection = null } }
                    FilterPills(
                        options = listOf("All") + cats, selectedIndex = (cats.indexOf(category) + 1).coerceAtLeast(0),
                        onSelect = { i -> if (i == 0) { category = null; collection = null } else category = cats[i - 1]; limit = 48 },
                        modifier = Modifier.padding(vertical = 4.dp), itemTag = { i -> if (i == 0) "aciMcpCat-all" else "aciMcpCat-${cats[i - 1]}" },
                    )
                    val q = query.trim().lowercase()
                    val colKeys = mcpCollections.firstOrNull { it.id == collection }?.ids?.toSet()
                    val list = all.filter { (category == null || it.category == category) && (colKeys == null || mcpRankKey(it.id) in colKeys) && (q.isEmpty() || it.name.lowercase().contains(q) || it.description.lowercase().contains(q)) }
                    if (list.isEmpty()) EmptyBox("No apps found", if (q.isEmpty()) "Nothing is listed here yet." else "No apps match your search.", Modifier.testTag("aciState"), art = EmptyArtKind.Search)
                    else AciRoundedCard {
                        list.take(limit).forEach { e ->
                            AciAppRow(e, if (e.connected) "Connected" else "Add", "aciMcpEntry-${e.id}") { add(e) }
                        }
                    }
                    if (list.size > limit) AciGlassAction("Show more", "aciMcpMore") { limit += 48 }
                }
                tab == McpTab.Shelf -> AciAppsShelf()
                else -> AciMcpBuildTab(model)
            }
            }
        }
    }
    }
    }
    if (addOpen) AciAddByUrlSheet(onDismiss = { addOpen = false }) { url -> model.signIn = model.addByUrl(url); addOpen = false }
    selected?.let { app -> AciInstalledAppSheet(app, model, onDismiss = { selected = null }) }
    if (miniOpen) {
        Dialog(onDismissRequest = { miniOpen = false }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) { AciMiniAppsStore(onClose = { miniOpen = false }) }
    }
}

/** A glass pill action (Connect, Add by URL, Install...). 44 dp tall; [label] is the spoken name when it differs from the title. */
@Composable
internal fun AciGlassAction(title: String, testTag: String, label: String? = null, onClick: () -> Unit) {
    GlassPill(onClick, Modifier.testTag(testTag).semantics(mergeDescendants = true) { contentDescription = label ?: title }, height = 44.dp) {
        Text(title, color = AllternitTheme.colors.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** A rounded (radius 20) neutral-fill card that groups list rows. */
@Composable
internal fun AciRoundedCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(AllternitTheme.colors.fillNeutral).padding(horizontal = 14.dp), content = content)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciAddByUrlSheet(onDismiss: () -> Unit, onSubmit: suspend (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("aciMcpAddSheet"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Add by URL", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            Text("Paste the address of an MCP server. You’ll sign in to it next.", color = c.textSecondary, fontSize = 14.sp)
            AciSearchField(url, "https://example.com/mcp", { url = it }, "aciMcpUrlField")
            AciPrimaryButton(if (busy) "Adding…" else "Add", "aciMcpUrlSubmit") {
                if (!busy && url.isNotBlank()) scope.launch { busy = true; onSubmit(url); busy = false }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciInstalledAppSheet(app: McpInstalledApp, model: McpAppsModel, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var tools by remember { mutableStateOf<List<McpTool>?>(null) }
    var toolsFailed by remember { mutableStateOf(false) }
    var confirmRemove by remember { mutableStateOf(false) }
    val current = model.apps.firstOrNull { it.appId == app.appId } ?: app
    LaunchedEffect(app.appId) {
        if (app.connectorId.isNotEmpty() && app.connected) { val t = model.tools(app.connectorId); tools = t; toolsFailed = t == null }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("aciMcpInstalledSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(current.name, color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
            Text(listOfNotNull(current.host, if (current.connected) "Connected" else "Needs sign-in").joinToString(" · "), color = c.textSecondary, fontSize = 13.sp)
            current.description?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
            if (current.readOnly) {
                Text("This service is connected through Connectors. Manage or disconnect it there.", color = c.textSecondary, fontSize = 13.sp)
            } else {
                if (!current.connected) AciPrimaryButton("Reconnect", "aciMcpSheetReconnect") { scope.launch { model.startOAuth(current.connectorId)?.let { openExternal(context, it) } } }
                Text("When the app wants to act", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                MCP_MODES.forEach { (id, label) ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable { scope.launch { model.setPermissionMode(current, id) } }.testTag("aciMcpMode-$id"), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                        Text(if (current.permissionMode == id) "●" else "○", color = c.textPrimary, fontSize = 16.sp, modifier = Modifier.padding(end = 12.dp))
                        Text(label, color = c.textPrimary, fontSize = 15.sp)
                    }
                }
            }
            if (current.connected) {
                Text("What it can do" + (tools?.let { " · ${it.size}" } ?: ""), color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                when {
                    toolsFailed -> Text("Couldn’t read this app’s tools. Your computer has to be on.", color = c.textSecondary, fontSize = 13.sp)
                    tools == null && current.connectorId.isNotEmpty() -> Text("Loading…", color = c.textTertiary, fontSize = 13.sp)
                    else -> tools.orEmpty().forEach { t ->
                        Column(Modifier.padding(vertical = 4.dp).testTag("aciMcpTool")) {
                            Text("${t.name} · ${t.effect}", color = c.textPrimary, fontSize = 14.sp)
                            t.description?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 2) }
                        }
                    }
                }
                AppScreenSection(AppScreenTarget(app = (current.logoKey ?: current.appId.replace(Regex("^(connector:|mcp:|url:)"), "")), name = current.name, connected = true))
            }
            if (!current.readOnly) {
                if (!confirmRemove) AciPillButton("Remove", "aciMcpRemove") { confirmRemove = true }
                else Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Remove ${current.name}? The agent can’t use it any more.", color = c.textSecondary, fontSize = 13.sp)
                    AciPillButton("Yes, remove", "aciMcpRemoveConfirm") { scope.launch { model.remove(current); onDismiss() } }
                    AciPillButton("Cancel", "aciMcpRemoveCancel") { confirmRemove = false }
                }
            }
        }
    }
}

@Composable
private fun AciMcpBuildTab(model: McpAppsModel) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var items by remember { mutableStateOf<List<McpSubmission>?>(null) }
    var failed by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { val r = model.submissions(); items = r; failed = r == null }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("aciMcpBuild")) {
        Text("Your apps", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        when {
            failed -> Text("Couldn’t load your apps.", color = c.textSecondary, fontSize = 14.sp)
            items == null -> Text("Loading…", color = c.textTertiary, fontSize = 14.sp)
            items.orEmpty().isEmpty() -> Text("You haven’t submitted an app yet. Package one and it shows up here.", color = c.textSecondary, fontSize = 14.sp)
            else -> items.orEmpty().forEach { s ->
                AciListRow(s.name, s.rejection, when (s.state) { "approved" -> "Approved"; "rejected" -> "Rejected"; "pending", "in_review" -> "In review"; else -> s.state.replaceFirstChar { it.uppercase() } }, "aciMcpSubmission-${s.id}")
            }
        }
        Spacer(Modifier.size(8.dp))
        Text("New to building one? Read the MCP Apps docs.", color = c.textPrimary, fontSize = 14.sp,
            modifier = Modifier.heightIn(min = 44.dp).clickable { openExternal(context, "https://docs.allternit.com/mini-apps") }.testTag("aciMcpDocs"))
    }
}
