package com.allternit.feature.computer

import androidx.compose.foundation.ExperimentalFoundationApi
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.AppLogo
import kotlinx.coroutines.launch

/** iOS AppDirectory.Collection (CONNECTOR_COLLECTIONS). */
internal data class McpCollection(val id: String, val title: String, val blurb: String, val ids: List<String>)

internal val mcpCollections = listOf(
    McpCollection("developers", "Allternit for developers", "Ship from chat: repos, issues, deploys, and errors in one place.", listOf("github", "gitlab", "linear", "sentry", "vercel", "jira", "bitbucket", "supabase")),
    McpCollection("design", "Allternit for design", "Sketch, diagram, and iterate with your creative tools.", listOf("figma", "canva", "miro", "adobe", "dribbble", "webflow", "framer", "notion")),
    McpCollection("operations", "Allternit for operations", "Money, customers, and pipeline, read straight from the source.", listOf("stripe", "hubspot", "salesforce", "quickbooks", "xero", "shopify", "intercom", "zendesk")),
)

/** iOS AppDirectory.rankKey. */
internal fun mcpRankKey(id: String) = id.replace(Regex("^mcp:"), "").replace("-", "_")

/** iOS DirectoryHero: one slide per curated collection with its real logos and Explore; dots switch slides. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun AciDirectoryHero(pool: List<McpEntry>, onOpen: (McpEntry) -> Unit, onCollection: (String) -> Unit) {
    val c = AllternitTheme.colors
    val byKey = pool.associateBy { mcpRankKey(it.id) }
    val slides = mcpCollections.mapNotNull { col -> col.ids.mapNotNull { byKey[it] }.takeIf { it.size >= 3 }?.let { col to it } }
    if (slides.isEmpty()) return
    val pager = rememberPagerState(pageCount = { slides.size })
    val scope = rememberCoroutineScope()
    val shape = RoundedCornerShape(16.dp)
    Box(Modifier.fillMaxWidth().clip(shape).background(c.textPrimary).testTag("aciMcpHero")) {
        HorizontalPager(pager) { page ->
            val (col, members) = slides[page]
            Column(Modifier.fillMaxWidth().heightIn(min = 248.dp).padding(start = 24.dp, end = 24.dp, top = 28.dp, bottom = 40.dp)) {
                Text("A://TERNIT  APPS", color = c.bgPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 20.dp))
                Text("CURATED BY ALLTERNIT", color = c.bgPrimary.copy(alpha = 0.7f), fontSize = 11.sp, fontFamily = FontFamily.Monospace, letterSpacing = 1.3.sp)
                Text(col.title, color = c.bgPrimary, fontSize = 22.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 4.dp))
                Text(col.blurb, color = c.bgPrimary.copy(alpha = 0.7f), fontSize = 14.sp, modifier = Modifier.padding(top = 6.dp))
                Row(Modifier.padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    members.take(6).forEach { m -> AppLogo(m.name, Modifier.clickable(role = Role.Button) { onOpen(m) }.testTag("aciMcpHeroApp-${m.id}"), id = m.id, url = m.serverUrl, size = 36.dp) }
                }
                Text("Explore", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.padding(top = 20.dp).clip(RoundedCornerShape(8.dp)).background(c.bgPrimary).heightIn(min = 36.dp).clickable(role = Role.Button) { onCollection(col.id) }.testTag("aciMcpHeroExplore-${col.id}").padding(horizontal = 16.dp, vertical = 9.dp))
            }
        }
        if (slides.size > 1) Row(Modifier.align(Alignment.BottomCenter).padding(bottom = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            slides.forEachIndexed { i, (col, _) ->
                val on = pager.currentPage == i
                Box(Modifier.heightIn(min = 24.dp).clickable(role = Role.Tab) { scope.launch { pager.animateScrollToPage(i) } }.testTag("aciMcpHeroDot-${col.id}"), contentAlignment = Alignment.Center) {
                    Box(Modifier.width(if (on) 20.dp else 6.dp).height(6.dp).clip(CircleShape).background(if (on) c.bgPrimary else c.bgPrimary.copy(alpha = 0.4f)))
                }
            }
        }
    }
}

/** An AciListRow with the app's logo tile first (iOS ConnectorCard). */
@Composable
internal fun AciAppRow(entry: McpEntry, trailing: String?, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        AppLogo(entry.name, id = entry.id, url = entry.serverUrl, size = 44.dp)
        Box(Modifier.weight(1f)) { AciListRow((if (entry.official) "✓ " else "") + entry.name, entry.description.ifBlank { entry.category }, trailing, tag) }
    }
}
