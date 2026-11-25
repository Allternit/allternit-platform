package com.allternit.feature.shell.firstrun

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.LibrarySearchField
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** Rows or cards the capped panel shows at a time (plan D9). */
private const val SHOWN = 6
private val ROW_H = 68.dp
private val CARD_H = 176.dp

/**
 * Connecting something that came from the catalog (More panel or the browse page). Keys with a
 * known provider go through the existing [ConnectSheetHost] key sheet; apps go through the same
 * connector gateway call Customize > Connectors uses (`POST connectors/{id}/connect`, optional
 * `api_key`, optional `authorize_url`), with [ConnectionsStore.markAppConnecting] while it runs and
 * [ConnectionsStore.loadPromotedApps] after, so the app lands in Apps as a normal row (plan D4).
 */
internal class MoreConnector(
    private val store: ConnectionsStore,
    private val scope: CoroutineScope,
    private val onSheet: (ConnSheet) -> Unit,
    private val onMessage: (String?) -> Unit,
    private val openUrl: (String) -> Unit,
) {
    var keyFor by mutableStateOf<CatalogEntry?>(null)

    fun connect(e: CatalogEntry) {
        onMessage(null)
        when {
            ConnectionsCatalog.keyProvider(e.id) != null -> onSheet(ConnSheet.Keys(e.id))
            e.group == "brains" -> e.keyPage?.let(openUrl) ?: onMessage("Add this key from the provider's site, then paste it under Brains.")
            e.ways.firstOrNull() == "paste" -> keyFor = e
            else -> run(e, null)
        }
    }

    fun submitKey(e: CatalogEntry, key: String) {
        keyFor = null
        run(e, key)
    }

    private fun run(e: CatalogEntry, key: String?) {
        val cid = e.connectorId ?: e.id
        store.markAppConnecting(e, true)
        scope.launch {
            val url = runCatching { AppConnectorsClient.connect(cid, key) }
            url.onFailure { store.markAppConnecting(e, false); onMessage(ConnectionsClient.describe(it)); return@launch }
            url.getOrNull()?.let(openUrl)
            // The sign-in finishes in the browser: look for the app a few times, then give up quietly.
            repeat(20) {
                store.loadPromotedApps()
                if (store.promotedApps.any { it.id == cid || it.id == e.id }) return@launch
                delay(3000)
            }
            store.markAppConnecting(e, false)
        }
    }
}

@Composable
internal fun rememberMoreConnector(store: ConnectionsStore, onSheet: (ConnSheet) -> Unit, onMessage: (String?) -> Unit): MoreConnector {
    val scope = rememberCoroutineScope()
    val uri = LocalUriHandler.current
    return remember(store) { MoreConnector(store, scope, onSheet, onMessage) { runCatching { uri.openUri(it) } } }
}

/** The API key prompt for an app that connects with a pasted key. */
@Composable
internal fun MoreConnectorDialog(connector: MoreConnector) {
    val e = connector.keyFor ?: return
    var key by remember(e.id) { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { connector.keyFor = null },
        title = { Text("${e.name} API key") },
        text = { LibraryField("API key", key, "Paste the key", "moreConnectKey") { key = it } },
        confirmButton = { TextButton(enabled = key.isNotBlank(), onClick = { connector.submitKey(e, key.trim()) }) { Text("Connect") } },
        dismissButton = { TextButton(onClick = { connector.keyFor = null }) { Text("Cancel") } },
    )
}

/**
 * The More tile at the end of a group (D3: only when the catalog count is above 0, hidden when the
 * counts are unknown) and, when open, the capped in-place panel.
 */
@Composable
internal fun MoreSection(
    group: String,
    title: String,
    store: ConnectionsStore,
    layout: LibraryLayout,
    connector: MoreConnector,
    onFallback: (String) -> Unit,
    onBrowse: () -> Unit,
) {
    val count = store.catalogCounts?.get(group) ?: 0
    if (count <= 0) return
    val c = setupColors
    var open by rememberSaveable(group) { mutableStateOf(false) }
    val label = "More ${title.lowercase()}, $count"
    Column(Modifier.fillMaxWidth().padding(top = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(c.card)
                .clickable(role = Role.Button) { open = !open }
                .semantics(mergeDescendants = true) {
                    contentDescription = label
                    stateDescription = if (open) "Expanded" else "Collapsed"
                }
                .padding(horizontal = 16.dp).testTag("connectionsMore_$group"),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text("More", color = AllternitTheme.colors.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
            Text("$count", color = c.mut, fontSize = 13.sp)
            Icon(painterResource(if (open) DesignR.drawable.ph_caret_up else DesignR.drawable.ph_caret_down), null, tint = c.sub, modifier = Modifier.size(14.dp))
        }
        if (open) {
            var query by remember(group) { mutableStateOf("") }
            val pager = remember(group) { CatalogPager(group) }
            PagerLoad(pager, query)
            Column(Modifier.fillMaxWidth().testTag("connectionsMorePanel_$group"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibrarySearchField(query, "Search ${title.lowercase()}", "connectionsMoreSearch_$group") { query = it }
                val area = when (layout) {
                    LibraryLayout.List -> ROW_H * SHOWN
                    LibraryLayout.Grid -> CARD_H * 3 + 12.dp * 2
                    LibraryLayout.Deck -> CARD_H
                }
                CatalogResults(pager, store, layout, connector, onFallback, Modifier.fillMaxWidth().height(area), query)
                val visible = pager.entries.count { it.id !in store.listedIds && it.runnable }
                if (visible > 0) Text("Showing ${minOf(SHOWN, visible)} of ${pager.total} · scroll for more", color = c.mut, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 6.dp))
                Text(
                    "Browse all connections", color = c.sub, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.clickable(role = Role.Button, onClick = onBrowse).padding(horizontal = 6.dp, vertical = 6.dp).testTag("connectionsMoreBrowse_$group"),
                )
            }
        }
    }
}

/** Loads the first page, and again (after a short pause) whenever the search text changes. */
@Composable
private fun PagerLoad(pager: CatalogPager, query: String) {
    LaunchedEffect(pager, query) {
        pager.query = query
        if (pager.loaded) delay(250)
        pager.reload()
    }
}

/** Skeleton, error with Retry, empty, or the entries in the chosen layout; asks for the next page near the end. */
@Composable
private fun CatalogResults(
    pager: CatalogPager,
    store: ConnectionsStore,
    layout: LibraryLayout,
    connector: MoreConnector,
    onFallback: (String) -> Unit,
    modifier: Modifier,
    query: String,
) {
    val c = setupColors
    val scope = rememberCoroutineScope()
    val visible = pager.entries.filter { it.id !in store.listedIds && it.runnable }
    LaunchedEffect(pager.entries) { store.rememberCatalog(pager.entries) }
    // Everything on this page is already listed: keep paging until something new shows up.
    LaunchedEffect(visible.isEmpty(), pager.hasMore, pager.loading) { if (visible.isEmpty() && pager.hasMore && !pager.loading) pager.loadMore() }
    when {
        pager.error != null && pager.entries.isEmpty() -> Column(modifier.testTag("connectionsMoreError"), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            Text(pager.error ?: "", color = c.errFg, fontSize = 13.sp)
            SetupQuietButton("Retry", Modifier.padding(top = 8.dp)) { scope.launch { pager.reload() } }
        }
        !pager.loaded || visible.isEmpty() && pager.hasMore -> Column(modifier.testTag("connectionsMoreSkeleton"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            repeat(if (layout == LibraryLayout.List) SHOWN else 3) {
                Box(Modifier.fillMaxWidth().height(if (layout == LibraryLayout.List) 56.dp else 84.dp).clip(RoundedCornerShape(12.dp)).background(c.fill))
            }
        }
        visible.isEmpty() -> Box(modifier.testTag("connectionsMoreEmpty"), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                if (query.isBlank()) com.allternit.core.design.EmptyArt(com.allternit.core.design.EmptyArtKind.Sheets, size = 72.dp)
                Text(
                    if (query.isBlank()) "Nothing more to add here." else "Nothing matches “${query.trim()}”.",
                    color = c.sub, fontSize = 14.sp,
                )
            }
        }
        else -> when (layout) {
            LibraryLayout.List -> {
                val ls = rememberLazyListState()
                val nearEnd by remember { derivedStateOf { ls.layoutInfo.let { it.totalItemsCount > 0 && (it.visibleItemsInfo.lastOrNull()?.index ?: 0) >= it.totalItemsCount - 3 } } }
                LaunchedEffect(nearEnd, visible.size) { if (nearEnd && pager.hasMore && !pager.loading) pager.loadMore() }
                LazyColumn(state = ls, modifier = modifier.clip(RoundedCornerShape(12.dp)).background(c.card)) {
                    itemsIndexed(visible, key = { _, e -> e.id }) { i, e ->
                        if (i > 0) HorizontalDivider(thickness = 0.5.dp, color = c.line)
                        ConnectionRow(e.entry(), store, null, null, onConnect = { connector.connect(e) }, onFallback = onFallback)
                    }
                    if (pager.loading) item { LoadingMore() }
                }
            }
            LibraryLayout.Grid -> {
                val gs = rememberLazyGridState()
                val nearEnd by remember { derivedStateOf { gs.layoutInfo.let { it.totalItemsCount > 0 && (it.visibleItemsInfo.lastOrNull()?.index ?: 0) >= it.totalItemsCount - 3 } } }
                LaunchedEffect(nearEnd, visible.size) { if (nearEnd && pager.hasMore && !pager.loading) pager.loadMore() }
                LazyVerticalGrid(GridCells.Fixed(2), state = gs, modifier = modifier, horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(visible, key = { it.id }) { e ->
                        ConnectionCard(e.entry(), store, null, onConnect = { connector.connect(e) }, onFallback = onFallback, modifier = Modifier.height(CARD_H))
                    }
                }
            }
            LibraryLayout.Deck -> {
                val ls = rememberLazyListState()
                val nearEnd by remember { derivedStateOf { ls.layoutInfo.let { it.totalItemsCount > 0 && (it.visibleItemsInfo.lastOrNull()?.index ?: 0) >= it.totalItemsCount - 3 } } }
                LaunchedEffect(nearEnd, visible.size) { if (nearEnd && pager.hasMore && !pager.loading) pager.loadMore() }
                val w = LocalConfiguration.current.screenWidthDp.dp * 0.78f
                LazyRow(state = ls, modifier = modifier, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(visible, key = { it.id }) { e ->
                        ConnectionCard(e.entry(), store, null, onConnect = { connector.connect(e) }, onFallback = onFallback, modifier = Modifier.width(w).height(CARD_H))
                    }
                }
            }
        }
    }
}

@Composable
private fun LoadingMore() {
    Box(Modifier.fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = setupColors.sub)
    }
}

/**
 * "Find more connections": every group of the catalog, with group chips and search. A full-screen
 * page inside the Connections screen (Back closes it), in the List look with the same rows and
 * connect flow as the More panels.
 */
@Composable
internal fun ConnectionsBrowseScreen(
    store: ConnectionsStore,
    connector: MoreConnector,
    onFallback: (String) -> Unit,
    onBack: () -> Unit,
) {
    val c = setupColors
    BackHandler(onBack = onBack)
    var group by rememberSaveable { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    val pager = remember(group) { CatalogPager(group) }
    PagerLoad(pager, query)
    Column(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("connectionsBrowse")) {
        LibraryHeader(
            title = "Find more connections",
            onBack = null,
            glass = true,
            leading = { LibraryIconButton(DesignR.drawable.ph_caret_left, "Back", "connectionsBrowseBack", onClick = onBack) },
        )
        LibrarySearchField(query, "Search all connections", "connectionsBrowseSearch", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { query = it }
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val chips = listOf<Pair<String?, String>>(null to "All") + CONNECTION_GROUPS.map { it.first to it.second }
            for ((g, name) in chips) {
                val on = group == g
                Text(
                    name, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                    color = if (on) AllternitTheme.colors.bgPrimary else AllternitTheme.colors.textPrimary,
                    modifier = Modifier.clip(RoundedCornerShape(50)).background(if (on) AllternitTheme.colors.textPrimary else c.fill)
                        .clickable(role = Role.Tab) { group = g }.padding(horizontal = 14.dp, vertical = 7.dp).testTag("connectionsBrowseChip_${g ?: "all"}"),
                )
            }
        }
        Spacer(Modifier.height(4.dp))
        CatalogResults(pager, store, LibraryLayout.List, connector, onFallback, Modifier.weight(1f).fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 16.dp), query)
    }
}
