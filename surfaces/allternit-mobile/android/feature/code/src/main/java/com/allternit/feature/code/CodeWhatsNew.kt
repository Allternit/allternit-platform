package com.allternit.feature.code

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.util.Locale

/** One gizzi-code release (iOS GizziRelease). */
internal data class GizziRelease(val version: String, val date: String?, val summary: String?, val sections: List<Section>) {
    data class Section(val kind: String, val items: List<String>)
}

/**
 * The gizzi-code releases for What's new (iOS GizziChangelog, views/code/code-whats-new.ts): read
 * live from the CHANGELOG on GitHub, cached (30 min TTL), the cache used when offline.
 */
internal object GizziChangelog {
    const val RAW = "https://raw.githubusercontent.com/Allternit/allternit-platform/main/cmd/gizzi-code/CHANGELOG.md"
    const val PAGE = "https://github.com/Allternit/allternit-platform/blob/main/cmd/gizzi-code/CHANGELOG.md"
    const val SEEN_KEY = "allternit:code-whats-new:seen"
    private const val CACHE_KEY = "allternit:gizzi-code-changelog:v1"
    private const val CACHE_TIME_KEY = "allternit:gizzi-code-changelog:v1:at"
    private const val TTL_MS = 30 * 60 * 1000L
    private const val MAX_RELEASES = 10

    private fun prefs(context: Context) = context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE)

    fun cached(context: Context): List<GizziRelease>? = prefs(context).getString(CACHE_KEY, null)?.let { parse(it) }

    /** (releases, live). The cache while fresh, else the live file; the cache on failure; throws with neither. */
    suspend fun load(context: Context, force: Boolean = false): Pair<List<GizziRelease>, Boolean> = withContext(Dispatchers.IO) {
        val p = prefs(context)
        val at = p.getLong(CACHE_TIME_KEY, 0L)
        val cachedText = p.getString(CACHE_KEY, null)
        if (!force && cachedText != null && System.currentTimeMillis() - at < TTL_MS) return@withContext parse(cachedText) to false
        try {
            val conn = URL(RAW).openConnection() as HttpURLConnection
            conn.connectTimeout = 10_000
            conn.readTimeout = 10_000
            try {
                if (conn.responseCode != 200) error("HTTP ${conn.responseCode}")
                val text = conn.inputStream.bufferedReader().use { it.readText() }
                p.edit().putString(CACHE_KEY, text).putLong(CACHE_TIME_KEY, System.currentTimeMillis()).apply()
                parse(text) to true
            } finally {
                conn.disconnect()
            }
        } catch (e: Exception) {
            if (cachedText != null) return@withContext parse(cachedText) to false
            throw e
        }
    }

    /** Latest released (non-Unreleased) version: what the unread dot keys on. */
    fun latestVersion(releases: List<GizziRelease>): String? = releases.firstOrNull { it.version != "Unreleased" }?.version

    private val heading = Regex("""^##\s+(Unreleased|v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?))\b(.*)$""", RegexOption.IGNORE_CASE)
    private val dateRe = Regex("""(\d{4}-\d{2}-\d{2})""")

    private fun clean(text: String): String = text
        .replace(Regex("""\[([^\]]+)\]\([^)]+\)"""), "$1")
        .replace(Regex("""\*\*([^*]+)\*\*"""), "$1")
        .replace(Regex("""\s+"""), " ")
        .trim()

    /** iOS parse: Keep a Changelog headings, `###` sections, bullets that may wrap, prose under a version as its summary. */
    fun parse(markdown: String): List<GizziRelease> {
        val releases = mutableListOf<GizziRelease>()
        var version: String? = null
        var date: String? = null
        var summaryText: String? = null
        val sections = mutableListOf<Pair<String, MutableList<String>>>()
        var item: MutableList<String>? = null
        var summary: MutableList<String>? = null

        fun flushItem() {
            val lines = item
            if (lines != null && sections.isNotEmpty() && version != null) {
                val text = clean(lines.joinToString(" "))
                if (text.isNotEmpty()) sections.last().second.add(text)
            }
            item = null
        }
        fun flushSummary() {
            val lines = summary
            if (lines != null && version != null && summaryText == null) {
                val text = clean(lines.joinToString(" "))
                if (text.isNotEmpty()) summaryText = text
            }
            summary = null
        }
        fun flushRelease() {
            flushItem(); flushSummary()
            val v = version
            if (v != null) {
                val kept = sections.filter { it.second.isNotEmpty() }.map { GizziRelease.Section(it.first, it.second.toList()) }
                if (kept.isNotEmpty() || summaryText != null) releases.add(GizziRelease(v, date, summaryText, kept))
            }
            version = null; date = null; summaryText = null; sections.clear()
        }

        for (raw in markdown.lines()) {
            val line = raw.trimEnd()
            if (line.startsWith("## ") || line == "##") {
                flushRelease()
                if (releases.count { it.version != "Unreleased" } >= MAX_RELEASES) break
                val m = heading.find(line)
                if (m != null) {
                    val unreleased = m.groupValues[1].lowercase() == "unreleased"
                    version = if (unreleased) "Unreleased" else m.groupValues[2]
                    date = dateRe.find(m.groupValues[3])?.groupValues?.get(1)
                }
                continue
            }
            if (version == null) continue
            if (line.startsWith("###")) {
                flushItem(); flushSummary()
                sections.add(clean(line.trimStart('#').trim()) to mutableListOf())
                continue
            }
            val trimmed = line.trim()
            if (trimmed.isEmpty()) { flushItem(); flushSummary(); continue }
            if (trimmed.length >= 2 && trimmed[0] in "-*+" && trimmed[1] == ' ') {
                flushItem(); flushSummary()
                if (sections.isEmpty()) sections.add("" to mutableListOf())
                item = mutableListOf(trimmed.drop(2))
                continue
            }
            val it = item
            if (it != null) it.add(trimmed)
            else if (sections.isEmpty()) summary = (summary ?: mutableListOf()).also { s -> s.add(trimmed) }
        }
        flushRelease()
        return releases
    }
}

/** Curated Code mode notes for the Allternit tab (iOS CodeChangelogEntry). */
private data class AppEntry(val date: String, val title: String, val items: List<String>)

private val AppEntries = listOf(
    AppEntry("2026-09-26", "Code mode top deck and sessions", listOf(
        "Top deck: environment picker, working-folder chip, and folder opening wired to a real workspace",
        "Worktree and Canvas pills appear only when they apply",
        "Permission modes (Manual / Auto-accept / Plan / Read only) in a Claude-style overlay, enforced by the runtime",
        "Sessions name themselves from the first turn — header and rail together",
        "Usage overview card with activity heatmap",
    )),
    AppEntry("2026-09-26", "Skills and connectors directory", listOf(
        "One-click install for directory skills, with progress and errors",
        "Skill and plugin detail pages: file tree, bundled skills and connectors",
        "Actions: check for updates, edit, duplicate, remove",
        "Connectors: shared directory of 1,100+ providers with real sign-in",
    )),
    AppEntry("2026-09-26", "Memory that reaches your chats", listOf(
        "Model-based memory extraction with add, update, and forget",
        "Settings → Memory: search, fact list, delete, and import",
        "Fixed a backend defect where no memory reached any chat",
    )),
    AppEntry("2026-09-26", "Projects, Claude-style", listOf(
        "Project pages: Memory, Context, Folder, and Scheduled in one column",
        "Every new project ships the Allternit prompting guide as context",
        "Native folder picker and a create flow that matches Claude’s",
    )),
    AppEntry("2026-09-25", "Shell and rail", listOf(
        "Account menu with plan, Usage, and platform console link",
        "Denser recents and a working pinned section",
        "Quick task stack with its own approval modes",
    )),
)

private val Coral = Color(0xFFD97757)

/**
 * The "What's new" pill (iOS CodeWhatsNewButton): a coral dot while the latest gizzi-code release
 * is unseen; a tap opens the sheet and marks it seen. The shell puts it right under the sessions button.
 */
@Composable
fun CodeWhatsNewButton(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val prefs = remember { context.applicationContext.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE) }
    var seen by remember { mutableStateOf(prefs.getString(GizziChangelog.SEEN_KEY, "").orEmpty()) }
    var latest by remember { mutableStateOf(GizziChangelog.cached(context)?.let { GizziChangelog.latestVersion(it) }) }
    var open by remember { mutableStateOf(false) }
    val unseen = latest != null && latest != seen
    LaunchedEffect(Unit) {
        runCatching { GizziChangelog.load(context) }.getOrNull()?.let { latest = GizziChangelog.latestVersion(it.first) }
    }
    val label = if (unseen) "What's new (unread)" else "What's new"
    Box(modifier) {
        Row(
            Modifier.height(28.dp).clip(CircleShape).background(c.fillNeutral).border(1.dp, c.borderSubtle, CircleShape)
                .testTag("codeWhatsNew")
                .clickable(role = Role.Button, onClickLabel = label) {
                    open = true
                    latest?.let { seen = it; prefs.edit().putString(GizziChangelog.SEEN_KEY, it).apply() }
                }
                .semantics { contentDescription = label }
                .padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_sparkle), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(11.dp))
            Text("What's new", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        }
        if (unseen) {
            Box(Modifier.align(Alignment.TopEnd).offset(x = 2.dp, y = (-2).dp).size(8.dp).clip(CircleShape).background(Coral).border(1.5.dp, c.bgPrimary, CircleShape).testTag("codeWhatsNewDot"))
        }
    }
    if (open) WhatsNewSheet(onDismiss = { open = false })
}

@Composable
private fun WhatsNewSheet(onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val uri = LocalUriHandler.current
    var tab by remember { mutableStateOf(0) }
    var releases by remember { mutableStateOf(GizziChangelog.cached(context).orEmpty()) }
    var live by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        runCatching { GizziChangelog.load(context) }
            .onSuccess { releases = it.first; live = it.second }
            .onFailure { error = "Couldn't load the gizzi code changelog. Check your connection, then try again." }
    }
    LibrarySheet(title = "What's new", testTag = "codeWhatsNewSheet", onDismiss = onDismiss, leading = LibrarySheetAction("Close", onClick = onDismiss)) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            listOf("gizzi code", "Allternit").forEachIndexed { i, name ->
                Box(
                    Modifier.height(24.dp).clip(RoundedCornerShape(6.dp))
                        .background(if (tab == i) c.textPrimary.copy(alpha = 0.09f) else Color.Transparent)
                        .testTag("codeWhatsNewTab-$i")
                        .clickable(role = Role.Tab) { tab = i }
                        .padding(horizontal = 8.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(name, color = if (tab == i) c.textPrimary else c.textTertiary, fontSize = 11.5.sp, fontWeight = FontWeight.SemiBold) }
            }
        }
        Column(Modifier.fillMaxWidth().heightIn(max = 480.dp).verticalScroll(rememberScrollState()).padding(14.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            if (tab == 0) {
                if (releases.isEmpty() && error != null) Text(error.orEmpty(), color = c.textSecondary, fontSize = 12.sp)
                else if (releases.isEmpty()) Box(Modifier.fillMaxWidth().padding(top = 24.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.size(24.dp), color = c.textSecondary, strokeWidth = 2.dp) }
                releases.forEach { ReleaseEntry(it) }
            } else {
                AppEntries.forEach { e ->
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
                            Text(e.title, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                            Text(formatDate(e.date), color = c.textTertiary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                        }
                        e.items.forEach { Bullet(it, c.accentPrimary) }
                    }
                }
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text(if (tab == 0) (if (live) "Live from GitHub" else "Offline snapshot") else "Code mode", color = c.textTertiary, fontSize = 10.5.sp)
            if (tab == 0) {
                Row(Modifier.testTag("codeWhatsNewFull").clickable(role = Role.Button) { uri.openUri(GizziChangelog.PAGE) }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                    Text("Full changelog", color = c.textTertiary, fontSize = 10.5.sp)
                    Icon(painterResource(DesignR.drawable.ph_arrow_square_out), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(11.dp))
                }
            }
        }
        Box(Modifier.padding(bottom = 12.dp))
    }
}

@Composable
private fun ReleaseEntry(release: GizziRelease) {
    val c = AllternitTheme.colors
    val unreleased = release.version == "Unreleased"
    Column(verticalArrangement = Arrangement.spacedBy(5.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically) {
            Text(if (unreleased) "Unreleased · on main" else "v${release.version}", color = if (unreleased) c.textSecondary else c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
            release.date?.let { Text(formatDate(it), color = c.textTertiary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold) }
        }
        release.summary?.let { Text(inline(it), color = c.textPrimary, fontSize = 12.sp) }
        release.sections.forEach { s ->
            val color = kindColor(s.kind)
            Column(Modifier.padding(top = 2.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (s.kind.isNotEmpty()) Text(s.kind.uppercase(), color = color, fontSize = 10.sp, fontWeight = FontWeight.Bold, letterSpacing = 0.8.sp)
                s.items.forEach { Bullet(it, color) }
            }
        }
    }
}

@Composable
private fun Bullet(text: String, color: Color) {
    val c = AllternitTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.padding(top = 7.dp).size(4.dp).clip(CircleShape).background(color))
        Text(inline(text), color = c.textSecondary, fontSize = 12.sp)
    }
}

/** `code` spans become mono chips (iOS InlineMarkdown). */
private fun inline(text: String): AnnotatedString = buildAnnotatedString {
    text.split("`").forEachIndexed { i, part ->
        if (i % 2 == 1) withStyle(SpanStyle(fontFamily = FontFamily.Monospace, fontSize = 11.sp, background = Color.Gray.copy(alpha = 0.18f))) { append(part) }
        else append(part)
    }
}

private fun kindColor(kind: String): Color = when (kind.lowercase()) {
    "added" -> Color(0xFF6B9A7B)
    "changed" -> Color(0xFFD4B08C)
    "fixed" -> Coral
    "removed" -> Color(0xFFA1A1AA)
    "security" -> Color(0xFFC06A6A)
    else -> Color(0xFF6B9A7B)
}

private fun formatDate(iso: String): String = runCatching {
    LocalDate.parse(iso).format(DateTimeFormatter.ofPattern("MMM d, yyyy", Locale.US))
}.getOrDefault(iso)
