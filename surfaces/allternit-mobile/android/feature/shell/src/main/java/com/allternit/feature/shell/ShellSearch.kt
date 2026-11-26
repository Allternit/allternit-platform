package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/** A named thing search can open (a cowork project or a bot). */
internal data class SearchEntry(val id: String, val title: String)

/**
 * What search matches besides sessions: `GET /api/v1/cowork/projects` → `{ projects: [{id, title}] }`
 * and `GET /api/v1/agents` → `{ agents: [{id, name}] }` (iOS ProjectStore, AgentModeStore). A
 * failure leaves that list empty: search still covers the chats.
 */
internal object SearchSources {
    private val config = ApiConfig()

    suspend fun list(path: String, arrayKey: String, titleKey: String): List<SearchEntry> {
        val url = config.apiBaseUrl.newBuilder().addPathSegments(path).build()
        val r = ApiCall.execute(ShellApi.okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())
        if (r !is ApiResult.Ok) return emptyList()
        return (r.value[arrayKey] as? JsonArray).orEmpty().mapNotNull { el ->
            val o = el as? JsonObject ?: return@mapNotNull null
            val id = (o["id"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
            val title = (o[titleKey] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
            SearchEntry(id, title)
        }
    }
}

/**
 * Full-screen search over chats, code threads, ACI runs, projects and bots (iOS ShellSearchView,
 * the drawer's round search button). Matches titles locally against what the drawer loaded, plus
 * the project and bot lists fetched on open. [signedIn] false skips those fetches.
 */
@Composable
internal fun ShellSearchScreen(
    store: ShellSessionsStore,
    signedIn: Boolean,
    onBack: () -> Unit,
    onOpenSession: (AgentSession) -> Unit,
    onOpenProject: (SearchEntry) -> Unit,
    onOpenBot: (SearchEntry) -> Unit,
) {
    val c = AllternitTheme.colors
    var query by remember { mutableStateOf("") }
    var projects by remember { mutableStateOf<List<SearchEntry>>(emptyList()) }
    var bots by remember { mutableStateOf<List<SearchEntry>>(emptyList()) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(signedIn) {
        runCatching { focus.requestFocus() }
        if (signedIn) {
            projects = SearchSources.list("cowork/projects", "projects", "title")
            bots = SearchSources.list("agents", "agents", "name")
        }
    }
    val needle = query.trim().lowercase()
    val sessions = if (needle.isEmpty()) emptyList() else store.sessions.filter {
        it.name.orEmpty().lowercase().contains(needle) || it.description.orEmpty().lowercase().contains(needle)
    }.take(40)
    val projectHits = if (needle.isEmpty()) emptyList() else projects.filter { it.title.lowercase().contains(needle) }
    val botHits = if (needle.isEmpty()) emptyList() else bots.filter { it.title.lowercase().contains(needle) }

    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("shellSearch")) {
        PhoneHeader(title = "Search", leading = {
            PhoneIconButton(com.allternit.core.design.R.drawable.ph_x, "Close", testTag = "shellSearchClose", iconSize = 18, onClick = onBack)
        })
        SearchField(query, "Search chats and projects", "shellSearchField", focus = focus) { query = it }
        when {
            needle.isEmpty() -> Text(
                "Search chats and projects.",
                color = c.textSecondary,
                fontSize = 14.sp,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().padding(top = 32.dp),
            )
            sessions.isEmpty() && projectHits.isEmpty() && botHits.isEmpty() -> FriendlyState(
                style = FriendlyStyle.Empty,
                title = "No results",
                message = "Nothing matches “$query”.",
                art = EmptyArtKind.Search,
                modifier = Modifier.padding(20.dp).testTag("shellSearchEmpty"),
            ) { Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary) }
            else -> LazyColumn(Modifier.fillMaxSize()) {
                if (sessions.isNotEmpty()) {
                    item(key = "h-chats") { SearchSectionHeader("Chats & threads") }
                    items(sessions, key = { "s-${it.id}" }) { s ->
                        SearchRow(sessionIcon(s), s.title, sessionSurfaceLabel(s), "searchSession${s.id}") { onOpenSession(s) }
                    }
                }
                if (projectHits.isNotEmpty()) {
                    item(key = "h-projects") { SearchSectionHeader("Projects") }
                    items(projectHits, key = { "p-${it.id}" }) { p ->
                        SearchRow(com.allternit.core.design.R.drawable.ph_folder, p.title, null, "searchProject${p.id}") { onOpenProject(p) }
                    }
                }
                if (botHits.isNotEmpty()) {
                    item(key = "h-bots") { SearchSectionHeader("Bots") }
                    items(botHits, key = { "b-${it.id}" }) { b ->
                        SearchRow(DesignR.drawable.ph_cpu, b.title, null, "searchBot${b.id}") { onOpenBot(b) }
                    }
                }
            }
        }
    }
}

@Composable
private fun SearchSectionHeader(title: String) {
    Text(
        title.uppercase(),
        color = AllternitTheme.colors.textTertiary,
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(start = 24.dp, top = 16.dp, bottom = 6.dp),
    )
}

@Composable
private fun SearchRow(icon: Int, title: String, subtitle: String?, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier
            .padding(horizontal = 16.dp, vertical = 1.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(c.bgPanel)
            .testTag(testTag)
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .heightIn(min = 48.dp)
            .padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            subtitle?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
        }
    }
}
