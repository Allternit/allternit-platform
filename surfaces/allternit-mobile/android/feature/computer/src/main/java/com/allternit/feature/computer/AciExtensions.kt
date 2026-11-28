package com.allternit.feature.computer

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyBox
import com.allternit.core.design.FilterPills
import com.allternit.core.design.ProvideDockScroll
import com.allternit.core.design.EmptyArtKind
import org.json.JSONArray
import org.json.JSONObject

/** iOS ACIExtension: a browser extension the agent can use (Allternit's own, the marketplace, and ones the person adds). */
internal data class AciExtension(
    val id: String, val name: String, val description: String, val icon: String, val version: String, val author: String,
    val category: String, val enabled: Boolean, val installed: Boolean, val owned: Boolean = false,
    val storeUrl: String? = null, val permissions: List<String> = emptyList(),
)

internal val EXTENSION_CATEGORIES = listOf("productivity" to "Productivity", "automation" to "Automation", "ai" to "AI Powered", "utilities" to "Utilities", "office" to "Office")

private val ALLTERNIT_EXTENSIONS = listOf(
    AciExtension("allternit-agent", "Allternit Computer Agent", "The attached browser extension for Allternit browser-use and computer-use.", "◎", "1.0.0", "Allternit", "ai", true, true, true, null, listOf("scripts", "clipboard", "downloads")),
    AciExtension("allternit-office-word", "Allternit for Word", "Allternit Office agent and document tools for Microsoft Word.", "W", "1.0.0", "Allternit", "office", false, false, true),
    AciExtension("allternit-office-excel", "Allternit for Excel", "Allternit Office agent and workbook tools for Microsoft Excel.", "X", "1.0.0", "Allternit", "office", false, false, true),
    AciExtension("allternit-office-powerpoint", "Allternit for PowerPoint", "Allternit Office agent and presentation tools for Microsoft PowerPoint.", "P", "1.0.0", "Allternit", "office", false, false, true),
)

private val MARKETPLACE = listOf(
    AciExtension("market-claude-chrome", "Claude in Chrome", "AI assistant for navigating sites and completing multi-step browser workflows.", "✦", "latest", "Anthropic", "ai", false, false, false,
        "https://chromewebstore.google.com/detail/claude/fcoeoabgfenejglbffodgkkbkcdhcgfn", listOf("tabs", "scripts")),
    AciExtension("market-ublock-origin", "uBlock Origin", "Fast, efficient and privacy-focused content blocking for the browser.", "U", "latest", "Raymond Hill", "utilities", false, false, false,
        "https://chromewebstore.google.com/detail/ublock-origin/cjpalhdlnbpafiamejdnhcphjbkeiagm", listOf("tabs")),
    AciExtension("market-1password", "1Password", "Secure password manager and digital wallet with browser autofill.", "1P", "latest", "1Password", "productivity", false, false, false,
        "https://chromewebstore.google.com/detail/1password-password-manage/aeblfdkhhhdcdjpifhhbdiojplfjncoa", listOf("tabs", "clipboard")),
    AciExtension("market-grammarly", "Grammarly", "Writing assistant for grammar, spelling, clarity and tone.", "G", "latest", "Grammarly", "productivity", false, false, false,
        "https://chromewebstore.google.com/detail/grammarly-grammar-checker/kbfnbcaeplbcioakkpcpgfkobkghlhen", listOf("scripts")),
)

/** iOS ACIExtensionsStore: kept on this phone (UserDefaults `allternit.aci-extensions` there, SharedPreferences here). */
internal class AciExtensionsStore(private val context: Context) {
    private val key = "allternit.aci-extensions"
    var items by mutableStateOf(load()); private set

    private fun load(): List<AciExtension> {
        val raw = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(key, null)
        val saved = runCatching {
            val a = JSONArray(raw ?: return@runCatching null)
            (0 until a.length()).map { i ->
                val o = a.getJSONObject(i)
                AciExtension(o.getString("id"), o.getString("name"), o.optString("description"), o.optString("icon"), o.optString("version"), o.optString("author"), o.optString("category", "utilities"),
                    o.optBoolean("enabled"), o.optBoolean("installed"), o.optBoolean("owned"), o.optString("storeUrl").takeIf { it.isNotEmpty() },
                    o.optJSONArray("permissions")?.let { p -> (0 until p.length()).map { p.getString(it) } }.orEmpty())
            }
        }.getOrNull()
        // Allternit's own always exist; keep their saved state.
        return if (saved == null) ALLTERNIT_EXTENSIONS else ALLTERNIT_EXTENSIONS.filter { o -> saved.none { it.id == o.id } } + saved
    }

    private fun save(next: List<AciExtension>) {
        items = next
        val a = JSONArray()
        next.forEach { e ->
            a.put(JSONObject().put("id", e.id).put("name", e.name).put("description", e.description).put("icon", e.icon).put("version", e.version).put("author", e.author)
                .put("category", e.category).put("enabled", e.enabled).put("installed", e.installed).put("owned", e.owned).put("storeUrl", e.storeUrl ?: "")
                .put("permissions", JSONArray(e.permissions)))
        }
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(key, a.toString()).apply()
    }

    private fun update(id: String, f: (AciExtension) -> AciExtension) = save(items.map { if (it.id == id) f(it) else it })
    fun toggle(id: String) = update(id) { it.copy(enabled = !it.enabled) }
    fun install(id: String) = update(id) { it.copy(installed = true, enabled = true) }
    fun remove(id: String) = save(items.filterNot { it.id == id })
    fun installFromMarketplace(e: AciExtension) {
        val existing = items.firstOrNull { it.name == e.name }
        if (existing != null) install(existing.id)
        else save(items + e.copy(id = "ext-" + java.util.UUID.randomUUID().toString().take(8), installed = true, enabled = true))
    }
    fun addCustom(name: String, description: String, storeUrl: String?) =
        save(items + AciExtension("ext-" + java.util.UUID.randomUUID().toString().take(8), name, description.ifEmpty { "Custom ACI extension" }, "E", "1.0.0", "Custom", "utilities", true, true, false, storeUrl))
}

/** ACI › Extensions (iOS ACIExtensionsView): My Extensions and the Marketplace, category filter, detail sheet, Upload. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciExtensionsView() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val store = remember { AciExtensionsStore(context) }
    var query by remember { mutableStateOf("") }
    var category by remember { mutableStateOf<String?>(null) }
    var marketplace by remember { mutableStateOf(false) }
    var uploadOpen by remember { mutableStateOf(false) }
    var detail by remember { mutableStateOf<AciExtension?>(null) }
    var toast by remember { mutableStateOf<String?>(null) }
    val q = query.trim().lowercase()
    fun match(e: AciExtension) = (q.isEmpty() || e.name.lowercase().contains(q) || e.description.lowercase().contains(q)) && (category == null || e.category == category)
    val mine = store.items.filter(::match)
    val market = MARKETPLACE.filter(::match)

    val scroll = rememberScrollState()
    // The header lives in the scroll so its title docks as the page scrolls (see the clipping note in the PR).
    ProvideDockScroll(scroll) {
    AciDockPage("ACI Extensions", scroll, Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().testTag("aciExtensions")) {
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(scroll).padding(horizontal = 16.dp).padding(bottom = 56.dp)) {
            AciViewHeader("ACI Extensions", bottom = 16.dp)
            AciSearchField(query, "Search extensions", { query = it }, "aciExtSearch")
            FilterPills(
                options = listOf("All") + EXTENSION_CATEGORIES.map { it.second }, selectedIndex = EXTENSION_CATEGORIES.indexOfFirst { it.first == category } + 1,
                onSelect = { i -> category = if (i == 0) null else EXTENSION_CATEGORIES[i - 1].first },
                modifier = Modifier.padding(vertical = 4.dp), itemTag = { i -> if (i == 0) "aciExtCat-all" else "aciExtCat-${EXTENSION_CATEGORIES[i - 1].first}" },
            )
            Row(Modifier.padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AciGlassAction(if (marketplace) "My Extensions" else "Marketplace", "aciExtMarketplace") { marketplace = !marketplace }
                AciGlassAction("＋ Upload Extension", "aciExtUpload") { uploadOpen = true }
            }
            toast?.let { Text(it, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp).testTag("aciExtToast")) }
            Column(Modifier.fillMaxWidth().padding(top = 8.dp)) {
            if (marketplace) {
                Text("Extension Marketplace", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text("Install curated browser capabilities into Allternit.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp))
                if (market.isEmpty()) EmptyBox("No extensions found", "Nothing in the marketplace matches.", Modifier.testTag("aciState"), art = EmptyArtKind.Search)
                else AciRoundedCard {
                    market.forEach { e ->
                        val installed = store.items.any { it.name == e.name && it.installed }
                        AciListRow("${e.icon}  ${e.name}", "${e.description}\nv${e.version} · ${e.author}", if (installed) "Installed" else "Install", "aciExtMarket-${e.id}") {
                            if (installed) detail = e else { store.installFromMarketplace(e); toast = "Added and enabled ${e.name}" }
                        }
                    }
                }
            } else {
                if (mine.isEmpty()) EmptyBox("No extensions found", "Install one from the Marketplace, or upload your own.", Modifier.testTag("aciState"), onClick = { marketplace = true }, art = EmptyArtKind.Search)
                else AciRoundedCard {
                    mine.forEach { e ->
                        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                AciListRow("${e.icon}  ${e.name}", "${e.description}\nv${e.version} · ${e.author}", null, "aciExt-${e.id}") { detail = e }
                            }
                            if (e.installed) AciGlassAction(if (e.enabled) "On" else "Off", "aciExtToggle-${e.id}", "${if (e.enabled) "Turn off" else "Turn on"} ${e.name}") { store.toggle(e.id) }
                            else AciGlassAction("Install", "aciExtInstall-${e.id}") { store.install(e.id); toast = "Added and enabled ${e.name}" }
                        }
                    }
                }
                Text("Build your own extensions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 20.dp))
                Text("Our SDK allows you to create native ACI extensions that interact with agent context, handle page automation, and surface unique tools.", color = c.textSecondary, fontSize = 13.sp)
                AciPillButton("Open Developer Documentation", "aciExtDocs") { openExternal(context, "https://allternit.com/developers") }
            }
            }
        }
    }
    }
    }
    if (uploadOpen) AciUploadExtensionSheet({ uploadOpen = false }) { name, desc, url -> store.addCustom(name, desc, url); toast = "Added and enabled $name"; uploadOpen = false }
    detail?.let { e ->
        val saved = store.items.firstOrNull { it.id == e.id }
        ModalBottomSheet(onDismissRequest = { detail = null }, containerColor = c.bgPrimary) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("aciExtDetail"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("${e.icon}  ${e.name}", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                Text("v${e.version} · ${e.author}", color = c.textSecondary, fontSize = 13.sp)
                Text(e.description, color = c.textPrimary, fontSize = 14.sp)
                if (e.permissions.isNotEmpty()) {
                    Text("Permissions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text(e.permissions.joinToString(", "), color = c.textSecondary, fontSize = 13.sp)
                }
                e.storeUrl?.let { u -> AciPillButton("Open store page", "aciExtStore") { openExternal(context, u) } }
                if (saved != null && !saved.owned) AciPillButton("Remove", "aciExtRemove") { store.remove(saved.id); detail = null }
                AciPillButton("Back", "aciExtBack") { detail = null }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciUploadExtensionSheet(onDismiss: () -> Unit, onAdd: (String, String, String?) -> Unit) {
    val c = AllternitTheme.colors
    var name by remember { mutableStateOf("") }
    var desc by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("aciExtUploadSheet"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Upload Extension", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            AciSearchField(name, "Name", { name = it }, "aciExtName")
            AciSearchField(desc, "What it does", { desc = it }, "aciExtDesc")
            AciSearchField(url, "Store page (optional)", { url = it }, "aciExtUrl")
            AciPrimaryButton("Add extension", "aciExtAdd") { if (name.isNotBlank()) onAdd(name.trim(), desc.trim(), url.trim().ifEmpty { null }) }
        }
    }
}
