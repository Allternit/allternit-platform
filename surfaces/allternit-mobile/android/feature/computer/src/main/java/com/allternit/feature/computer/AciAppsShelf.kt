package com.allternit.feature.computer

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/*
 * The Apps shelf + share list (ACI > MCP Apps > Apps, behind the local flag `apps.shelf`), mirroring
 * iOS AppsShelfView (PR #2961): two sections, "Made by you / your org" and "From a connected server",
 * then "Your links". Routes (cloud-api, relative to /api/v1):
 *   GET apps -> {yours, servers}; POST apps/:kind/:ref/install {granted_tier, pinned?, consent?};
 *   POST apps/:kind/:ref/pin {pinned}; DELETE apps/:kind/:ref; POST apps/:slug/handoff -> {open_url};
 *   GET share-links -> {links}; POST share-links/:id/pause {paused}; DELETE share-links/:id.
 */

/** What an app may do. Raised tiers ask again; the server answers `needs_consent`. */
internal enum class AppsTier(val label: String, val detail: String) {
    Browser("Browser", "Runs in a sandbox. Can show pages and nothing else."),
    App("App", "Can save its own data and use the tools you list for it."),
    Full("Full", "Can use your connected apps and run tasks for you.");

    companion object {
        /** Unknown tiers read as the strictest one. */
        fun from(raw: String?): AppsTier = entries.firstOrNull { it.name.equals(raw, ignoreCase = true) } ?: Browser
    }
}

internal data class ShelfRow(
    val kind: String,
    val provenance: String?,
    val tier: String?,
    val installed: Boolean,
    val pinned: Boolean,
    val link: String?,
    val slug: String,
    val icon: String?,
    val name: String,
) {
    val id get() = "$kind:$slug"
    val appTier get() = AppsTier.from(tier)
    val isVendor get() = kind.lowercase().contains("vendor")

    /** "you" and "org" rows are the person's own; "shared" came from someone's link. */
    val provenanceLabel: String?
        get() = when (provenance) { "you" -> "You"; "org" -> "Your org"; "shared" -> "Shared with you"; else -> null }

    /** The app record's icon when it is short enough to draw (an emoji or a letter), else the name's first letter. */
    val glyph: String get() = icon?.takeIf { it.isNotEmpty() && it.codePointCount(0, it.length) <= 2 } ?: name.take(1).uppercase()

    companion object {
        fun from(o: JsonObject): ShelfRow {
            val slug = o.str("slug").orEmpty()
            return ShelfRow(
                kind = o.str("kind") ?: "artifact", provenance = o.str("provenance"), tier = o.str("tier"),
                installed = o.bool("installed") ?: false, pinned = o.bool("pinned") ?: false,
                link = o.str("link"), slug = slug, icon = o.str("icon"), name = o.str("name") ?: slug,
            )
        }
    }
}

internal data class ShareLinkRow(val id: String, val name: String?, val subdomain: String?, val url: String?, val paused: Boolean, val revoked: Boolean, val killed: Boolean) {
    /** Live links run on the person's own computer (`<name>.live.<domain>`). */
    val isLive get() = url.orEmpty().contains(".live.")
    val isActive get() = !revoked && !killed
    val title get() = name ?: subdomain ?: url ?: "Link"

    companion object {
        fun from(o: JsonObject): ShareLinkRow? {
            val id = o.str("id") ?: return null
            return ShareLinkRow(id, o.str("name"), o.str("subdomain"), o.str("url"), o.bool("paused") ?: false,
                (o.bool("revoked") ?: false) || o.str("revoked_at") != null, (o.bool("killed") ?: false) || o.str("killed_at") != null)
        }
    }
}

internal data class ConsentRequest(val row: ShelfRow, val tier: AppsTier, val added: List<String>, val raised: Boolean)

internal class AppsShelfModel {
    var yours by mutableStateOf<List<ShelfRow>>(emptyList()); private set
    var servers by mutableStateOf<List<ShelfRow>>(emptyList()); private set
    var links by mutableStateOf<List<ShareLinkRow>>(emptyList()); private set
    var loaded by mutableStateOf(false); private set
    var failure by mutableStateOf<String?>(null); private set
    var linksFailure by mutableStateOf<String?>(null); private set
    var notice by mutableStateOf<String?>(null)
    val busy = mutableStateListOf<String>()
    /** An install waiting on the person's yes (first install, or a raised tier). */
    var consent by mutableStateOf<ConsentRequest?>(null)

    suspend fun refresh() {
        when (val r = LibraryNet.get("apps")) {
            is ApiResult.Ok -> {
                yours = r.value.obj.list("yours").map(ShelfRow::from)
                servers = r.value.obj.list("servers").map(ShelfRow::from)
                failure = null
            }
            else -> failure = r.friendly("load your apps")
        }
        when (val r = LibraryNet.get("share-links")) {
            is ApiResult.Ok -> { links = r.value.obj.list("links").mapNotNull(ShareLinkRow::from).filter { it.isActive }; linksFailure = null }
            else -> linksFailure = r.friendly("load your links")
        }
        loaded = true
    }

    fun requestInstall(row: ShelfRow) { consent = ConsentRequest(row, row.appTier, emptyList(), false) }

    suspend fun confirmInstall(request: ConsentRequest) {
        consent = null
        install(request.row, request.tier)
    }

    private suspend fun install(row: ShelfRow, tier: AppsTier) = withBusy(row.id) {
        val body = buildJsonObject { put("granted_tier", tier.name.lowercase()); put("consent", true) }
        when (val r = LibraryNet.send("POST", "apps/${esc(row.kind)}/${esc(row.slug)}/install", body)) {
            is ApiResult.Ok -> {
                val o = r.value.obj
                if (o.bool("needs_consent") == true) {
                    // The tier went up since the person last said yes: ask again.
                    val added = (o?.get("added_capabilities") as? kotlinx.serialization.json.JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.content }
                    consent = ConsentRequest(row, AppsTier.from(o.str("requested_tier") ?: tier.name), added, true)
                } else refresh()
            }
            else -> notice = "Couldn’t install ${row.name}. ${r.friendly("install this app")}"
        }
    }

    suspend fun uninstall(row: ShelfRow) = withBusy(row.id) {
        val r = LibraryNet.send("DELETE", "apps/${esc(row.kind)}/${esc(row.slug)}")
        if (r is ApiResult.Ok) refresh() else notice = "Couldn’t remove ${row.name}. ${r.friendly("remove this app")}"
    }

    suspend fun togglePin(row: ShelfRow) = withBusy(row.id) {
        val r = LibraryNet.send("POST", "apps/${esc(row.kind)}/${esc(row.slug)}/pin", buildJsonObject { put("pinned", !row.pinned) })
        if (r is ApiResult.Ok) refresh() else notice = "Couldn’t change the pin on ${row.name}. ${r.friendly("pin this app")}"
    }

    suspend fun togglePause(link: ShareLinkRow) = withBusy(link.id) {
        val r = LibraryNet.send("POST", "share-links/${esc(link.id)}/pause", buildJsonObject { put("paused", !link.paused) })
        if (r is ApiResult.Ok) refresh() else notice = "Couldn’t ${if (link.paused) "resume" else "pause"} the link. ${r.friendly("change this link")}"
    }

    suspend fun revoke(link: ShareLinkRow) = withBusy(link.id) {
        val r = LibraryNet.send("DELETE", "share-links/${esc(link.id)}")
        if (r is ApiResult.Ok) refresh() else notice = "Couldn’t stop the link. ${r.friendly("stop this link")}"
    }

    private suspend fun withBusy(id: String, block: suspend () -> Unit) {
        busy.add(id)
        try { block() } finally { busy.remove(id) }
    }

    companion object {
        private fun esc(s: String) = LibraryNet.escape(s)

        /** The URL to load on Open: the minted `open_url` for own/org/shared apps, else the plain link. Vendor apps have no handoff. */
        suspend fun openLink(slug: String, link: String?, vendor: Boolean): String? {
            if (!vendor && slug.isNotEmpty()) {
                val r = LibraryNet.send("POST", "apps/${esc(slug)}/handoff")
                val minted = AppsShelfLinks.openUrl((r as? ApiResult.Ok)?.value.obj.str("open_url"))
                if (minted != null) return minted
            }
            return AppsShelfLinks.openUrl(link)
        }
    }
}

/** The "Apps" tab body inside the MCP Apps screen (flag `apps.shelf`). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciAppsShelf() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val model = remember { AppsShelfModel() }
    var opened by remember { mutableStateOf<Pair<String, String>?>(null) }
    var stopping by remember { mutableStateOf<ShareLinkRow?>(null) }
    LaunchedEffect(Unit) { model.refresh() }

    fun open(row: ShelfRow) {
        scope.launch {
            val url = AppsShelfModel.openLink(row.slug, row.link, row.isVendor)
            if (url == null) model.notice = "${row.name} has no link to open yet." else opened = url to row.name
        }
    }

    Column(Modifier.fillMaxWidth().testTag("aciAppsShelf"), verticalArrangement = Arrangement.spacedBy(24.dp)) {
        when {
            !model.loaded -> Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                repeat(4) { Box(Modifier.fillMaxWidth().heightIn(min = 68.dp).clip(RoundedCornerShape(14.dp)).background(c.fillNeutral)) }
            }
            model.failure != null && model.yours.isEmpty() && model.servers.isEmpty() ->
                AciStateCard("Couldn’t load your apps", model.failure.orEmpty(), "Try again", { scope.launch { model.refresh() } })
            else -> {
                model.notice?.let { n ->
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(n, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.weight(1f).testTag("aciAppsNotice"))
                        Text("Dismiss", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.heightIn(min = 44.dp).clickable { model.notice = null }.padding(8.dp))
                    }
                }
                ShelfSection("Made by you / your org", model.yours, "Apps you make, or your org shares, show up here.", model, context, ::open)
                ShelfSection("From a connected server", model.servers, "Connect an MCP server and its apps show up here.", model, context, ::open)
                LinksSection(model, context) { stopping = it }
            }
        }
    }

    model.consent?.let { request ->
        ModalBottomSheet(onDismissRequest = { model.consent = null }, containerColor = c.bgPrimary) {
            ConsentSheet(request, onAllow = { scope.launch { model.confirmInstall(request) } }, onCancel = { model.consent = null })
        }
    }
    stopping?.let { link ->
        AlertDialog(
            onDismissRequest = { stopping = null },
            title = { Text("Stop this link?") },
            text = { Text("Anyone with the link loses access. You can make a new one later.") },
            confirmButton = { TextButton({ stopping = null; scope.launch { model.revoke(link) } }) { Text("Stop link") } },
            dismissButton = { TextButton({ stopping = null }) { Text("Keep it") } },
        )
    }
    opened?.let { (url, name) ->
        Dialog(onDismissRequest = { opened = null }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            Column(Modifier.fillMaxSize().background(c.bgPrimary)) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text("Done", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium, modifier = Modifier.heightIn(min = 48.dp).clickable { opened = null }.padding(8.dp).testTag("aciAppsDone"))
                }
                var failed by remember(url) { mutableStateOf(false) }
                var reload by remember(url) { mutableStateOf(0) }
                Box(Modifier.fillMaxSize()) {
                    AciPlainWebView(url, Modifier.fillMaxSize(), reload = reload, onFail = { failed = true })
                    if (failed) Column(Modifier.fillMaxSize().background(c.bgPrimary), verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically), horizontalAlignment = Alignment.CenterHorizontally) {
                        Text("Couldn’t load $name.", color = c.textSecondary, fontSize = 14.sp)
                        Text("Try again", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                            modifier = Modifier.heightIn(min = 44.dp).clickable { failed = false; reload += 1 }.padding(12.dp).testTag("aciAppsRetry"))
                    }
                }
            }
        }
    }
}

@Composable
private fun ShelfSection(title: String, rows: List<ShelfRow>, empty: String, model: AppsShelfModel, context: Context, onOpen: (ShelfRow) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(title, color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        if (rows.isEmpty()) Text(empty, color = c.textSecondary.copy(alpha = 0.7f), fontSize = 14.sp, modifier = Modifier.padding(vertical = 8.dp))
        rows.forEach { row ->
            val busy = row.id in model.busy
            ShelfCard {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Box(Modifier.size(40.dp).clip(RoundedCornerShape(10.dp)).background(c.bgSecondary), contentAlignment = Alignment.Center) {
                        Text(row.glyph, color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                    }
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(row.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(listOfNotNull(row.provenanceLabel, row.appTier.label).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                    }
                    if (busy) androidx.compose.material3.CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (row.installed) {
                        ShelfPill("Open", "Open ${row.name}", filled = true, enabled = !busy) { onOpen(row) }
                        ShelfPill(if (row.pinned) "Unpin" else "Pin", "${if (row.pinned) "Unpin" else "Pin"} ${row.name}", enabled = !busy) { scope.launch { model.togglePin(row) } }
                        if (AppsShelfLinks.openUrl(row.link) != null) {
                            ShelfPill("Add to home screen", "Add ${row.name} to home screen", enabled = !busy) {
                                if (!AppShortcuts.pin(context, row)) model.notice = "This launcher can’t add shortcuts to the home screen."
                            }
                        }
                        ShelfPill("Remove", "Remove ${row.name}", enabled = !busy) { scope.launch { model.uninstall(row) } }
                    } else {
                        ShelfPill("Install", "Install ${row.name}", filled = true, enabled = !busy) { model.requestInstall(row) }
                        if (row.link != null) ShelfPill("Open", "Open ${row.name}", enabled = !busy) { onOpen(row) }
                    }
                }
            }
        }
    }
}

@Composable
private fun LinksSection(model: AppsShelfModel, context: Context, onStop: (ShareLinkRow) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Your links", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        if (model.linksFailure != null && model.links.isEmpty()) Text("Couldn’t load your links. ${model.linksFailure}", color = c.textSecondary.copy(alpha = 0.7f), fontSize = 14.sp)
        else if (model.links.isEmpty()) Text("Share an app from Desktop and its link shows up here.", color = c.textSecondary.copy(alpha = 0.7f), fontSize = 14.sp, modifier = Modifier.padding(vertical = 8.dp))
        model.links.forEach { link ->
            val busy = link.id in model.busy
            var copied by remember(link.id) { mutableStateOf(false) }
            LaunchedEffect(copied) { if (copied) { delay(1_500); copied = false } }
            ShelfCard {
                Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(link.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val kind = if (link.isLive) "Live from your computer" else "Hosted"
                    Text(if (link.paused) "$kind · Paused" else kind, color = c.textSecondary, fontSize = 12.sp)
                    link.url?.let { Text(it, color = c.textSecondary.copy(alpha = 0.7f), fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    link.url?.let { url ->
                        ShelfPill(if (copied) "Copied" else "Copy link", "Copy link ${link.title}", enabled = !busy) {
                            (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Allternit link", url))
                            copied = true
                        }
                    }
                    ShelfPill(if (link.paused) "Resume" else "Pause", "${if (link.paused) "Resume" else "Pause"} ${link.title}", enabled = !busy) { scope.launch { model.togglePause(link) } }
                    ShelfPill("Stop", "Stop ${link.title}", enabled = !busy) { onStop(link) }
                }
            }
        }
    }
}

@Composable
private fun ShelfCard(content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) { content() }
}

/** A 44 dp pill: filled is the primary action. */
@Composable
private fun ShelfPill(title: String, label: String, filled: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).clip(CircleShape)
            .background(if (filled) c.accentPrimary else c.bgSecondary)
            .then(if (filled) Modifier else Modifier.border(1.dp, c.borderSubtle, CircleShape))
            .clickable(enabled = enabled, onClick = onClick)
            .androidSemantics(label)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = if (filled) c.onAccent else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
}

private fun Modifier.androidSemantics(label: String): Modifier =
    this.then(Modifier.semantics { contentDescription = label })

/** Says what the app may do before it installs. Tier names match the web sheet. */
@Composable
private fun ConsentSheet(request: ConsentRequest, onAllow: () -> Unit, onCancel: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).testTag("aciAppsConsent"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Text(if (request.raised) "${request.row.name} wants more access" else "Install ${request.row.name}?", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AppsTier.entries.forEach { tier ->
                    val on = tier == request.tier
                    Text(tier.label, color = if (on) c.onAccent else c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                        modifier = Modifier.clip(CircleShape).background(if (on) c.accentPrimary else c.fillNeutral).padding(horizontal = 12.dp, vertical = 6.dp))
                }
            }
            Text(request.tier.detail, color = c.textSecondary, fontSize = 14.sp)
        }
        if (request.added.isNotEmpty()) Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("New access", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            request.added.forEach { Text("• $it", color = c.textPrimary, fontSize = 14.sp) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.weight(1f).heightIn(min = 48.dp).clip(CircleShape).background(c.fillNeutral).clickable(onClick = onCancel), contentAlignment = Alignment.Center) {
                Text("Cancel", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            }
            Box(Modifier.weight(1f).heightIn(min = 48.dp).clip(CircleShape).background(c.accentPrimary).clickable(onClick = onAllow), contentAlignment = Alignment.Center) {
                Text(if (request.raised) "Allow" else "Install", color = c.onAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}
