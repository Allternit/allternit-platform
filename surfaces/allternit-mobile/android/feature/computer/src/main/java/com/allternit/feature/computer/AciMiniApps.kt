package com.allternit.feature.computer

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
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
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyArtKind
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.OkHttpClient
import okhttp3.Request

/** iOS MiniApp / MiniAppCategory (Core/API/Models/MiniApp.swift). */
internal enum class MiniAppCategory(val label: String) { Runtime("Runtime"), Connector("Connector"), Tool("Tool"), Data("Data"), Communication("Communication"), Custom("Custom") }

internal data class MiniApp(
    val id: String, val name: String, val description: String, val category: MiniAppCategory, val url: String,
    val repo: String?, val publisher: String?, val capabilities: List<String>, val verified: Boolean, val source: String,
)

/** iOS MiniAppCatalogStore: four community apps plus the official MCP registry (`registry.modelcontextprotocol.io`), pins on this phone. */
internal object MiniAppCatalog {
    private const val REGISTRY = "https://registry.modelcontextprotocol.io/v0.1/servers?limit=24"
    private const val PINNED_KEY = "allternit-mini-apps-pinned"
    private val http by lazy { OkHttpClient() }

    private val community = listOf(
        MiniApp("github:activepieces/activepieces", "Activepieces", "Open-source automation platform with a visual workflow builder and a large connector ecosystem.", MiniAppCategory.Connector,
            "https://github.com/activepieces/activepieces", "activepieces/activepieces", "Activepieces", listOf("Workflow automation", "Connectors", "Self-hosted"), false, "github"),
        MiniApp("github:n8n-io/n8n", "n8n", "Community workflow automation platform for connecting APIs, services, data, and AI tools.", MiniAppCategory.Connector,
            "https://github.com/n8n-io/n8n", "n8n-io/n8n", "n8n", listOf("Workflow automation", "Integrations", "Self-hosted"), false, "github"),
        MiniApp("github:lobehub/lobe-chat", "LobeChat", "Open-source AI workspace with model providers, agents, knowledge bases, and plugins.", MiniAppCategory.Communication,
            "https://github.com/lobehub/lobe-chat", "lobehub/lobe-chat", "LobeHub", listOf("AI workspace", "Agents", "Knowledge bases"), false, "github"),
        MiniApp("github:mintplex-labs/anything-llm", "AnythingLLM", "Open-source AI application for document chat, agents, models, and private workspaces.", MiniAppCategory.Data,
            "https://github.com/Mintplex-Labs/anything-llm", "Mintplex-Labs/anything-llm", "Mintplex Labs", listOf("Document chat", "Agents", "Local models"), false, "github"),
    )

    /** Community apps then the registry's servers; throws on a failed fetch (the screen shows Retry). */
    suspend fun load(): List<MiniApp> = withContext(Dispatchers.IO) {
        http.newCall(Request.Builder().url(REGISTRY).build()).execute().use { r ->
            if (!r.isSuccessful) error("registry ${r.code}")
            val root = Json.parseToJsonElement(r.body?.string().orEmpty()).obj
            community + root.list("servers").mapNotNull { rec ->
                val server = rec["server"] as? JsonObject ?: return@mapNotNull null
                val name = server.str("name") ?: return@mapNotNull null
                val remoteUrl = server.list("remotes").firstOrNull().str("url")
                val repoUrl = (server["repository"] as? JsonObject).str("url")
                MiniApp("mcp:$name", server.str("title") ?: name.substringAfterLast('/'), server.str("description") ?: "MCP server from the official registry.",
                    MiniAppCategory.Connector, remoteUrl ?: repoUrl ?: "https://registry.modelcontextprotocol.io", repoUrl, name.substringBefore('/'),
                    listOf(if (remoteUrl != null) "Remote MCP" else "Package MCP"), true, "mcp")
            }
        }
    }

    fun pinned(context: Context): Set<String> = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getStringSet(PINNED_KEY, emptySet()) ?: emptySet()
    fun setPinned(context: Context, id: String, on: Boolean) {
        val next = pinned(context).toMutableSet().also { if (on) it.add(id) else it.remove(id) }
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putStringSet(PINNED_KEY, next).apply()
    }
}

/** iOS MiniAppsStoreView: search, category pills, a card per app with Open and Pin; Open runs it (MiniAppRuntimeView). */
@Composable
fun AciMiniAppsStore(onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var apps by remember { mutableStateOf<List<MiniApp>?>(null) }
    var failed by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var category by remember { mutableStateOf<MiniAppCategory?>(null) }
    var pins by remember { mutableStateOf(MiniAppCatalog.pinned(context)) }
    var running by remember { mutableStateOf<MiniApp?>(null) }
    suspend fun load() { failed = false; apps = runCatching { MiniAppCatalog.load() }.getOrElse { failed = true; null } }
    LaunchedEffect(Unit) { load() }
    BackHandler { if (running != null) running = null else onClose() }
    val runningApp = running
    if (runningApp != null) { MiniAppRuntime(runningApp, onBack = { running = null }); return }
    AciFullPage {
        Column(Modifier.fillMaxSize().testTag("aciMiniAppsStore")) {
            AciTopBar("Mini Apps", "Back", onClose)
            Column(Modifier.padding(horizontal = 16.dp)) {
                Text("Browse connectors, runtimes, and tools", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp))
                AciSearchField(query, "Search mini apps…", { query = it }, "aciMiniSearch")
                AciChipRow {
                    AciChip("All", category == null, "aciMiniCat-all") { category = null }
                    MiniAppCategory.entries.forEach { cat -> AciChip(cat.label, category == cat, "aciMiniCat-${cat.name}") { category = cat } }
                }
            }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                val list = apps
                val q = query.trim().lowercase()
                val filtered = list.orEmpty().filter { (category == null || it.category == category) && (q.isEmpty() || it.name.lowercase().contains(q) || it.description.lowercase().contains(q)) }
                when {
                    failed && list == null -> AciStateCard("Couldn't load catalog", "Check your connection and try again.", "Retry", { scope.launch { load() } })
                    list == null -> AciStateCard("Loading…", "Reading the catalog.")
                    filtered.isEmpty() -> AciStateCard("No mini apps found", if (q.isEmpty()) "Browse connectors, runtimes, and tools." else "No mini apps match your search.", art = EmptyArtKind.Search)
                    else -> filtered.forEach { app ->
                        AciCard(Modifier.testTag("aciMiniCard-${app.id}")) {
                            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                                Text(app.name + if (app.verified) "  ✓" else "", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                                Text(app.publisher ?: app.source.uppercase(), color = c.textSecondary, fontSize = 12.sp)
                                Text(app.description, color = c.textSecondary, fontSize = 12.sp, maxLines = 3)
                                Text((listOf(app.category.label) + app.capabilities.take(2)).joinToString("  ·  "), color = c.textTertiary, fontSize = 11.sp)
                                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                    AciPillButton("Open", "aciMiniOpen-${app.id}", "Open ${app.name}") { running = app }
                                    val pinned = app.id in pins
                                    AciPillButton(if (pinned) "Unpin" else "Pin", "aciMiniPin-${app.id}", if (pinned) "Unpin app" else "Pin app") {
                                        MiniAppCatalog.setPinned(context, app.id, !pinned); pins = MiniAppCatalog.pinned(context)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** iOS MiniAppRuntimeView: the app's page in a web view; an app with no page shows its native capabilities. */
@Composable
internal fun MiniAppRuntime(app: MiniApp, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val isWeb = app.url.startsWith("http://") || app.url.startsWith("https://")
    AciFullPage {
        Column(Modifier.fillMaxSize().testTag("aciMiniRuntime")) {
            AciTopBar(app.name, "Mini Apps", onBack)
            if (isWeb) AciPlainWebView(app.url, Modifier.fillMaxSize())
            else Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(app.name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Text("Native Allternit capabilities", color = c.textSecondary, fontSize = 12.sp)
                (app.capabilities.ifEmpty { listOf("Managed miniapp") }).forEach { AciListRow(it, tag = "aciMiniCap") }
            }
        }
    }
}
