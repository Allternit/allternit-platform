package com.allternit.feature.settings

import android.annotation.SuppressLint
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** The real app screen lives on one small web page (sample data only); the Catalog detail frames it. */
private const val SCREEN_PAGE = "https://ai.allternit.com/embed/app-screen.html"

/**
 * Catalog detail (iOS twin): back, logo, name, "By author · blurb", Add or Connect through the same routes the
 * connector list uses, the connector URL to copy, In-chat UI, developer and verified, categories, Developer website,
 * and the app's real screen in a web frame when it has a screen pack.
 */
@Composable
internal fun CatalogDetail(host: SettingsHost, seed: CatEntry, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val openLink = rememberLinkOpener()
    val (detail, retry) = rememberLoad("catalogDetail_${seed.id}") { CatalogClient.detail(seed.id) }
    // The list row already holds the entry, so the page paints at once and the detail call only fills in more.
    val entry = ((detail as? Load.Ready)?.value) ?: seed
    val (conns, reloadConns) = rememberLoad("catalogConnectors") { Net.get("connectors").toLoad("load connectors") }
    val (installed, reloadInstalled) = rememberLoad("catalogSkillsInstalled") { runtimeLoad(host, "GET", "/api/v1/skills/catalog/installed") }
    var busy by remember { mutableStateOf(false) }
    var keyAsk by remember { mutableStateOf(false) }
    var apiKey by remember { mutableStateOf("") }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var copied by remember { mutableStateOf(false) }
    BackHandler(onBack = onBack)

    val row = (conns as? Load.Ready)?.value.listUnder("connectors", "items").firstOrNull { it.str("id") == entry.connectorId }
    val connected = row?.get("connection").obj.str("status") == "connected"
    val needsKey = row.str("auth_type") == "api_key"
    val skillAdded = (installed as? Load.Ready)?.value.listUnder("skills").any { it.str("id") == entry.id }

    fun connect(key: String?) {
        busy = true; error = null; message = null
        scope.launch {
            val body = if (key != null) buildJsonObject { put("api_key", key) } else buildJsonObject {}
            val r = Net.send("POST", "connectors/${LibraryNet.escape(entry.connectorId)}/connect", body)
            if (r is ApiResult.Ok) {
                val o = r.value.obj
                o.str("authorize_url")?.let { openLink(it) }
                message = o.str("message")
                keyAsk = false; apiKey = ""
                reloadConns()
            } else error = r.friendly("connect")
            busy = false
        }
    }
    fun addSkill() {
        busy = true; error = null
        scope.launch {
            val r = runtimeLoad(host, "POST", "/api/v1/skills/catalog/install", buildJsonObject { put("id", entry.id) }.toString())
            if (r is Load.Ready) reloadInstalled() else error = (r as? Load.Failed)?.message ?: "Connect a computer first."
            busy = false
        }
    }

    SPage(entry.name, tag = "catalogDetail", onBack = onBack) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp), horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
            CatalogIcon(entry, size = 56.dp)
            Column(Modifier.weight(1f)) {
                Text(entry.name + if (entry.verified) "  Verified" else "", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                Text(listOfNotNull(entry.author?.let { "By $it" }, entry.blurb).joinToString(" · "), color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp)
            }
        }
        val label = when {
            entry.isSkill -> if (skillAdded) "Added" else "Add"
            connected -> "Connected"
            else -> "Connect"
        }
        val canAct = entry.available && !busy && !(entry.isSkill && skillAdded) && !(!entry.isSkill && connected)
        Box(
            Modifier.fillMaxWidth().padding(top = 14.dp).heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp))
                .background(if (canAct) c.textPrimary else c.fillNeutral)
                .clickable(enabled = canAct, role = Role.Button) { if (entry.isSkill) addSkill() else if (needsKey) keyAsk = true else connect(null) }
                .testTag("catalogAction"),
            contentAlignment = Alignment.Center,
        ) { Text(if (busy) "..." else label, color = if (canAct) c.bgPrimary else c.textSecondary, fontSize = 16.sp, fontWeight = FontWeight.Medium) }
        if (!entry.available) SNote(entry.unavailableReason ?: "Not available right now.")
        message?.let { SNote(it) }
        SError(error)

        if (entry.screenPack != null) {
            SLabel("What it looks like")
            AppScreenFrame(entry.screenPack, dark = c.bgPrimary.luminance() < 0.5f)
            SNote("Sample data.")
        }

        SLabel("Details")
        SCard {
            entry.serverUrl?.let { url ->
                SRow("Connector URL", "catalog_url", detail = url, chevron = false, onClick = { clipboard.setText(AnnotatedString(url)); copied = true }, trailing = {
                    Text(if (copied) "Copied" else "Copy", color = c.accentPrimary, fontSize = 14.sp)
                })
                SDivider()
            }
            if (entry.interactive) { SRow("In-chat UI", "catalog_interactive", detail = "Shows this app's screen inside the chat.", chevron = false); SDivider() }
            SRow("Developer", "catalog_developer", value = entry.author ?: "Unknown", chevron = false)
            SDivider()
            SRow("Verified", "catalog_verified", value = if (entry.verified) "Yes" else "No", chevron = false)
            if (entry.categories.isNotEmpty()) { SDivider(); SRow("Categories", "catalog_categories", value = entry.categories.joinToString(", ") { it.replaceFirstChar { ch -> ch.uppercase() } }, chevron = false) }
        }
        entry.authorUrl?.let { url ->
            SCard(Modifier.padding(top = 10.dp)) { SRow("Developer website ↗", "catalog_website", chevron = false, onClick = { openLink(url) }) }
        }
        if (detail is Load.Failed) SNote("Some details didn't load.")
        if (detail is Load.Failed) Text("Try again", color = c.accentPrimary, fontSize = 14.sp, modifier = Modifier.heightIn(min = 44.dp).clickable(role = Role.Button) { retry() }.padding(horizontal = 6.dp))
    }

    if (keyAsk) AlertDialog(
        onDismissRequest = { keyAsk = false; apiKey = "" },
        title = { Text("Enter the API key") },
        text = { SField("API key", apiKey, "catalog_apiKey", { apiKey = it }) },
        confirmButton = { TextButton(enabled = apiKey.isNotBlank(), onClick = { connect(apiKey.trim()) }) { Text("Connect") } },
        dismissButton = { TextButton(onClick = { keyAsk = false; apiKey = "" }) { Text("Cancel") } },
    )
}

/** The app's real screen: the embed page in an in-app web frame. Nothing is stored and no sign-in happens in it. */
@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun AppScreenFrame(pack: String, dark: Boolean) {
    val url = "$SCREEN_PAGE?app=${LibraryNet.escape(pack)}&theme=${if (dark) "dark" else "light"}"
    AndroidView(
        modifier = Modifier.fillMaxWidth().height(420.dp).clip(RoundedCornerShape(20.dp)).testTag("catalogScreen"),
        factory = { ctx ->
            WebView(ctx).apply {
                settings.javaScriptEnabled = true
                settings.domStorageEnabled = false
                settings.allowFileAccess = false
                setBackgroundColor(0)
                webViewClient = object : WebViewClient() {
                    // Only the embed page itself; every other link stays out of the frame.
                    override fun shouldOverrideUrlLoading(view: WebView, request: android.webkit.WebResourceRequest) = !request.url.toString().startsWith(SCREEN_PAGE)
                }
                loadUrl(url)
            }
        },
        update = { if (it.url != url) it.loadUrl(url) },
        onRelease = { it.destroy() },
    )
}
