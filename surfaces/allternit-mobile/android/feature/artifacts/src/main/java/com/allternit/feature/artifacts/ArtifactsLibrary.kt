package com.allternit.feature.artifacts

import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// MARK: - Layout (iOS OverviewLayout: List, Grid, Deck), shared with Projects

enum class LibraryLayout(val raw: String, val label: String) {
    List("list", "List"), Grid("grid", "Grid"), Deck("deck", "Deck");

    companion object {
        fun load(context: Context, key: String): LibraryLayout {
            val raw = context.getSharedPreferences("allternit_library", Context.MODE_PRIVATE).getString(key, null)
            return entries.firstOrNull { it.raw == raw } ?: Grid
        }

        fun save(context: Context, key: String, layout: LibraryLayout) {
            context.getSharedPreferences("allternit_library", Context.MODE_PRIVATE).edit().putString(key, layout.raw).apply()
        }
    }
}

/** iOS OverviewLayoutSwitch: three pills, the overview's layout ids (`overviewLayout<raw>`). */
@Composable
fun LibraryLayoutSwitch(selected: LibraryLayout, onPick: (LibraryLayout) -> Unit) {
    PillRow(LibraryLayout.entries, selected, { it.label }, { "overviewLayout${it.raw}" }, onPick = onPick)
}

/** A section label in a filter sheet. */
@Composable
fun FilterLabel(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 14.dp, bottom = 8.dp))
}

// MARK: - Studio kinds (iOS ArtifactStudioKind, gallery order)

enum class StudioKind(val raw: String, val label: String, val description: String, val emptyTitle: String, val studioHint: String, val modeId: String?, val studioKind: String, val filterKind: String?, val deck: Int) {
    Docs("docs", "Docs", "A document you write and edit in place, with Allternit drafting alongside.", "Draft a plan, memo or brief with Allternit", "The document appears here as Allternit writes it, and opens in the editor.", "docs", "doc", "doc", DesignR.drawable.deck_docs),
    Sheets("sheets", "Sheets", "A spreadsheet for data, formulas, and charts.", "Paste your data and turn it into a sheet", "The sheet appears here as Allternit builds it, and opens in the editor.", "data", "sheet", "sheet", DesignR.drawable.deck_sheets),
    Slides("slides", "Slides", "A deck you can present, restyle, and export to PowerPoint.", "Paste your ideas and turn them into slides", "The deck appears here as Allternit builds it, and opens in the editor.", "slides", "slides", "slides", DesignR.drawable.deck_slides),
    Dashboard("dashboard", "Dashboards", "Live charts and tiles that query your connected tools.", "Describe the numbers you want to watch", "Live charts and tiles that query your connected tools appear here.", null, "dashboard", "dashboard", DesignR.drawable.deck_dashboard),
    Ops("ops", "Ops sheets", "Live status, logs and actions for a deploy, site, app or server.", "Describe the deploy, site, app or server to watch", "Live status, logs and actions appear here.", null, "ops", null, DesignR.drawable.deck_ops),
    Motion("motion", "Motion", "An animated explainer you can export as video.", "Describe a short animated explainer", "Scenes play here, and you can export the result as an MP4 on the web or Desktop.", null, "motion", "motion", DesignR.drawable.deck_motion),
    Design("design", "Design", "Screens, flows, and graphics laid out on a canvas in Allternit Design.", "Describe an idea and turn it into a design", "The design appears here when it's saved.", "website", "design", "design", DesignR.drawable.deck_design),
    Page("page", "Page", "A single web page you can share.", "Describe a page you want to share", "The page renders here as a live preview you can share.", "website", "page", "page", DesignR.drawable.deck_page),
    Research("research", "Deep Research", "A cited research report built from many sources.", "Ask a question and get a cited report", "The report fills in here as sources are read, with citations you can open.", "research", "doc", null, DesignR.drawable.deck_research),
    Build("build", "Build", "A website or app, built and previewed live.", "Describe a site or app and watch it build", "The site appears here as it builds, with a live preview you can click through.", "website", "page", null, DesignR.drawable.deck_build),
    Image("image", "Image", "Generate or edit an image from a prompt.", "Describe an image and Allternit will make it", "Each image renders here, and you can refine it or ask for variations.", "image", "image", "image", DesignR.drawable.deck_image),
    Video("video", "Video", "A generated video clip, with the length and format you ask for.", "Describe a clip, its length and format", "The clip renders here when it's ready.", "video", "video", null, DesignR.drawable.deck_video);

    /** GALLERY_TILES `beta`. */
    val isBeta: Boolean get() = this !in setOf(Docs, Sheets, Slides, Page)

    /** "Untitled <noun>" (Deep Research: "Untitled research"). */
    val newTitle: String get() = if (this == Research) "Untitled research" else "Untitled ${ArtifactKinds.noun(studioKind).lowercase()}"

    /** v2 template kind the org can turn off. */
    val templateKind: String? get() = when (this) { Research, Build, Image, Video -> null; else -> studioKind }
}

// MARK: - Store (iOS ArtifactLibraryStore)

class ArtifactLibraryStore {
    enum class Show { Saved, Archive }
    enum class Owner(val scope: String, val label: String) { All("all", "Everything"), Mine("mine", "Mine"), Shared("shared", "Shared with me") }

    var items by mutableStateOf<List<ArtifactSummary>>(emptyList()); private set
    var loading by mutableStateOf(false); private set
    var loaded by mutableStateOf(false); private set
    var loadingMore by mutableStateOf(false); private set
    var failure by mutableStateOf<ApiResult<*>?>(null); private set
    var notice by mutableStateOf<String?>(null)
    var show by mutableStateOf(Show.Saved)
    var owner by mutableStateOf(Owner.All)
    var kind by mutableStateOf<String?>(null)
    var query by mutableStateOf("")
    var allows by mutableStateOf<(String) -> Boolean>({ true }); private set
    private var cursor: String? = null

    val filtered: Boolean get() = kind != null || owner != Owner.All || show != Show.Saved || query.isNotBlank()

    suspend fun load() {
        loading = true
        val r = ArtifactsV2Client.list(owner.scope, kind, query, null, show == Show.Archive)
        loading = false
        loaded = true
        when (r) {
            is ApiResult.Ok -> { items = r.value.items; cursor = r.value.nextCursor; failure = null }
            else -> failure = r
        }
    }

    suspend fun loadOrgSettings() { allows = ArtifactsV2Client.allowedKinds() }

    suspend fun loadMore() {
        val c = cursor ?: return
        if (loadingMore) return
        loadingMore = true
        val r = ArtifactsV2Client.list(owner.scope, kind, query, c, show == Show.Archive)
        loadingMore = false
        if (r is ApiResult.Ok) { items = items + r.value.items.filter { n -> items.none { it.id == n.id } }; cursor = r.value.nextCursor }
    }

    val hasMore: Boolean get() = cursor != null

    suspend fun setArchived(a: ArtifactSummary, archived: Boolean) {
        val r = ArtifactsV2Client.setArchived(a.id, archived)
        if (r is ApiResult.Ok) {
            items = items.filterNot { it.id == a.id }
            notice = if (archived) "Archived. You can restore it from Archive." else "Restored to saved work."
        } else {
            notice = "Couldn’t update your archive. Try again."
        }
    }

    suspend fun delete(a: ArtifactSummary): Boolean {
        val r = ArtifactsV2Client.delete(a.id)
        if (r is ApiResult.Ok || (r is ApiResult.HttpError && r.status == 404)) {
            items = items.filterNot { it.id == a.id }
            return true
        }
        notice = artifactError(r)
        return false
    }

    fun clearFilters() { kind = null; owner = Owner.All; show = Show.Saved; query = "" }
}

// MARK: - Library screen

/**
 * The Artifacts section (iOS ArtifactsLibraryView): the round ☰ header with the search and filter
 * pill, "Artifacts", Make something new (the studio tiles), Your artifacts in List / Grid / Deck,
 * and every empty, offline, signed-out and error state with the iOS copy. A card opens the V2
 * artifact view; its ⋯ (and a long press, like the iOS context menu) opens Archive / Delete.
 * [studioChat] is the chat pane the studio shows for its session (the shell's chat screen).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ArtifactsLibraryScreen(
    signedIn: Boolean,
    leading: @Composable () -> Unit,
    studioChat: @Composable (sessionId: String) -> Unit,
    onOpenChat: (sessionId: String) -> Unit,
    onConnectors: () -> Unit,
    onSignIn: () -> Unit,
    initialArtifactId: String? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { ArtifactLibraryStore() }
    var layout by remember { mutableStateOf(LibraryLayout.load(context, LAYOUT_KEY)) }
    var searching by remember { mutableStateOf(false) }
    var filterOpen by remember { mutableStateOf(false) }
    var openId by remember { mutableStateOf(initialArtifactId) }
    var actionsFor by remember { mutableStateOf<ArtifactSummary?>(null) }
    var confirmDelete by remember { mutableStateOf<ArtifactSummary?>(null) }
    var studio by remember { mutableStateOf<StudioSession?>(null) }
    var launching by remember { mutableStateOf<StudioKind?>(null) }
    var searchJob by remember { mutableStateOf<Job?>(null) }
    var newOpen by remember { mutableStateOf(false) }

    fun reload() { scope.launch { store.load() } }

    LaunchedEffect(signedIn) {
        if (signedIn) {
            store.load()
            store.loadOrgSettings()
        }
    }

    // Full-screen covers, like iOS fullScreenCover.
    openId?.let { id ->
        V2ArtifactScreen(artifactId = id, onClose = { openId = null; reload() }, onOpenChat = onOpenChat, onSignIn = onSignIn)
        return
    }
    studio?.let { s ->
        ArtifactStudioScreen(session = s, chatPane = studioChat, onClose = { studio = null; reload() }, onOpenArtifact = { openId = it })
        return
    }

    fun startSession(kind: StudioKind) {
        if (launching != null) return
        launching = kind
        scope.launch {
            val r = ArtifactStudioLauncher.launch(context, kind)
            launching = null
            r.onSuccess { studio = it }.onFailure { store.notice = "Couldn't start a ${ArtifactKinds.noun(kind.studioKind).lowercase()} session. ${it.message ?: ""}".trim() }
        }
    }

    fun launch(kind: StudioKind) {
        if (launching != null) return
        // A tile with a library kind toggles that filter (iOS ArtifactTileFilter; tap again clears).
        // Build, Deep Research, Ops and Video have none, so they start a session; while a kind
        // filter is on, the first row ("New <Tile>") starts that kind's session.
        val fk = kind.filterKind
        if (fk != null) {
            store.kind = if (store.kind == fk) null else fk
            reload()
            return
        }
        startSession(kind)
    }

    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("artifactsLibrary")) {
        LibraryHeader(
            title = "",
            onBack = null,
            leading = leading,
            trailing = {
                LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search artifacts", "artifactsSearch") { searching = !searching }
                LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Filter and layout", "artifactsFilter") { filterOpen = true }
            },
        )
        if (searching) {
            LibrarySearchField(store.query, "Search artifacts", "artifactsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { q ->
                store.query = q
                searchJob?.cancel()
                searchJob = scope.launch { delay(300); store.load() }
            }
        }
        val tiles = StudioKind.entries.filter { k -> k.templateKind?.let { store.allows(it) } ?: true }
        val columns = if (layout == LibraryLayout.Grid) 2 else 1
        LazyVerticalGrid(
            columns = GridCells.Fixed(columns),
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 84.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            val full: androidx.compose.foundation.lazy.grid.LazyGridItemSpanScope.() -> GridItemSpan = { GridItemSpan(maxLineSpan) }
            item(key = "title", span = full) {
                Text("Artifacts", color = c.textPrimary, fontSize = 30.sp, fontFamily = FontFamily.Serif, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 4.dp, bottom = 6.dp))
            }
            item(key = "make", span = full) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Make something new", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        Text("${tiles.size} types", color = c.textTertiary, fontSize = 13.sp)
                    }
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        items(tiles, key = { it.raw }) { k ->
                            StudioTile(k, selected = store.kind != null && store.kind == k.filterKind, busy = launching == k) { launch(k) }
                        }
                    }
                }
            }
            item(key = "tools", span = full) {
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text("Or start from your connected tools", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    Text(
                        "Connect your tools", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.clip(CircleShape).testTag("artifactsConnectTools").clickable(role = Role.Button, onClick = onConnectors).padding(horizontal = 10.dp, vertical = 8.dp),
                    )
                }
            }
            item(key = "yours", span = full) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Your artifacts", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                    store.notice?.let { n ->
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(start = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(n, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                            LibraryIconButton(DesignR.drawable.ph_x, "Dismiss", iconSize = 14) { store.notice = null }
                        }
                    }
                    if (store.filtered) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("Showing " + filterSummary(store), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text("Clear", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clip(CircleShape).testTag("artifactsClearFilters").clickable { store.clearFilters(); reload() }.padding(8.dp))
                        }
                    }
                }
            }
            when {
                !signedIn -> item(key = "signedout", span = full) {
                    FriendlyState(FriendlyStyle.Empty, "Sign in to see your artifacts", "Artifacts are saved to your Allternit account, so they follow you to every device.", actionTitle = "Sign in", onAction = onSignIn) {
                        Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                    }
                }
                store.loading && store.items.isEmpty() -> {
                    val (count, h) = when (layout) { LibraryLayout.Grid -> 6 to 190; LibraryLayout.Deck -> 2 to 260; LibraryLayout.List -> 5 to 90 }
                    items(count, key = { "sk$it" }) { i ->
                        Box(Modifier.fillMaxWidth().height(h.dp).clip(RoundedCornerShape(16.dp)).background(c.fillNeutral).then(if (i == 0) Modifier.semantics { contentDescription = "Loading artifacts" } else Modifier))
                    }
                }
                store.failure != null && store.items.isEmpty() -> item(key = "error", span = full) {
                    val f = store.failure!!
                    if (f is ApiResult.Network) {
                        FriendlyState(FriendlyStyle.Offline, "You’re offline", "Your artifacts will show here when you’re back online.", actionTitle = "Try again", onAction = ::reload) {
                            Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
                        }
                    } else if (f is ApiResult.HttpError && f.status == 401) {
                        FriendlyState(FriendlyStyle.Empty, "Sign in to see your artifacts", "Artifacts are saved to your Allternit account, so they follow you to every device.", actionTitle = "Sign in", onAction = onSignIn) {
                            Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                        }
                    } else {
                        FriendlyState(FriendlyStyle.Error, "Couldn’t load your artifacts", artifactError(f), actionTitle = "Try again", onAction = ::reload, modifier = Modifier.testTag("artifactsError")) {
                            Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
                        }
                    }
                }
                store.loaded && store.items.isEmpty() -> item(key = "empty", span = full) {
                    val k = store.kind
                    when {
                        k != null && store.query.isBlank() && store.owner == ArtifactLibraryStore.Owner.All && store.show == ArtifactLibraryStore.Show.Saved ->
                            FriendlyState(FriendlyStyle.Empty, "No ${ArtifactKinds.label(k).lowercase()} yet", "Tap the ${ArtifactKinds.label(k)} tile above to make one.", actionTitle = "Clear filters", onAction = { store.clearFilters(); reload() }) {
                                Icon(painterResource(ArtifactKinds.icon(k)), null, tint = c.textSecondary)
                            }
                        store.show == ArtifactLibraryStore.Show.Archive && !store.filteredBeyondArchive() ->
                            FriendlyState(FriendlyStyle.Empty, "No archived artifacts", "Archived work stays available here. Restore an artifact to return it to Saved work.") {
                                Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                            }
                        store.filtered ->
                            FriendlyState(FriendlyStyle.Empty, "Nothing matches", "Try another filter or search.", actionTitle = "Clear filters", onAction = { store.clearFilters(); reload() }) {
                                Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textSecondary)
                            }
                        else ->
                            FriendlyState(FriendlyStyle.Empty, "No artifacts yet", "Ask for a doc, deck, sheet or page in any chat, pick an Output in the message box, or tap + to make something new.", modifier = Modifier.testTag("artifactsEmpty")) {
                                Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                            }
                    }
                }
                layout == LibraryLayout.Deck -> item(key = "deck", span = full) {
                    LazyRow(horizontalArrangement = Arrangement.spacedBy(12.dp), contentPadding = PaddingValues(horizontal = 8.dp)) {
                        items(store.items, key = { it.id }) { a ->
                            ArtifactCard(a, LibraryLayout.Deck, Modifier.width(280.dp), onOpen = { openId = a.id }, onMore = { actionsFor = a })
                        }
                    }
                }
                else -> {
                    store.kind?.let { k ->
                        StudioKind.entries.firstOrNull { it.filterKind == k }?.let { tile ->
                            item(key = "newrow", span = full) {
                                LibraryRowButton("New ${tile.label}", DesignR.drawable.ph_plus, "artifactsNewRow") { startSession(tile) }
                            }
                        }
                    }
                    items(store.items, key = { it.id }) { a ->
                        ArtifactCard(a, layout, Modifier, onOpen = { openId = a.id }, onMore = { actionsFor = a })
                    }
                    if (store.hasMore) item(key = "more", span = full) {
                        LaunchedEffect(store.items.size) { store.loadMore() }
                        Text("Loading…", color = c.textTertiary, fontSize = 13.sp, modifier = Modifier.padding(16.dp))
                    }
                }
            }
        }
    }

    // iOS: the floating "New artifact" capsule (its menu is a sheet here, reached by tap).
    Text(
        "New artifact", color = c.bgPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 16.dp)
            .height(50.dp).clip(RoundedCornerShape(25.dp)).background(c.textPrimary).testTag("artifactsNew")
            .clickable(role = Role.Button) { newOpen = true }.padding(horizontal = 24.dp, vertical = 14.dp),
    )
    }

    if (newOpen) {
        NewArtifactSheet(
            tiles = StudioKind.entries.filter { k -> k.templateKind?.let { store.allows(it) } ?: true },
            signedIn = signedIn,
            onTile = { startSession(it) },
            onConnectors = onConnectors,
            onUploading = { store.notice = it },
            onUploaded = { r -> r.onSuccess { id -> store.notice = null; reload(); openId = id }.onFailure { store.notice = it.message } },
            onDismiss = { newOpen = false },
            scope = scope,
        )
    }

    if (filterOpen) {
        LibrarySheet("Filter", "artifactsFilterSheet", onDismiss = { filterOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { filterOpen = false; reload() }) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState())) {
                FilterLabel("Layout")
                LibraryLayoutSwitch(layout) { layout = it; LibraryLayout.save(context, LAYOUT_KEY, it) }
                FilterLabel("Show")
                PillRow(ArtifactLibraryStore.Show.entries, store.show, { if (it == ArtifactLibraryStore.Show.Saved) "Saved work" else "Archive" }, { "artifactsShow${it.name}" }) { store.show = it }
                FilterLabel("Owner")
                PillRow(ArtifactLibraryStore.Owner.entries, store.owner, { it.label }, { "artifactsOwner${it.scope}" }) { store.owner = it }
                FilterLabel("Kind")
                PillRow(listOf<String?>(null) + ArtifactKinds.filterKinds, store.kind, { it?.let(ArtifactKinds::label) ?: "All kinds" }, { "artifactsKind${it ?: "all"}" }) { store.kind = it }
            }
        }
    }

    actionsFor?.let { a ->
        LibrarySheet(a.title, "artifactActions", onDismiss = { actionsFor = null }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibraryRowButton("Open", DesignR.drawable.ph_arrow_square_out, "artifactActionOpen") { actionsFor = null; openId = a.id }
                val archive = store.show == ArtifactLibraryStore.Show.Archive
                LibraryRowButton(if (archive) "Restore to saved work" else "Archive", DesignR.drawable.ph_floppy_disk, "artifactActionArchive") {
                    actionsFor = null
                    scope.launch { store.setArchived(a, !archive) }
                }
                if (a.isMine) LibraryRowButton("Delete", DesignR.drawable.ph_trash, "artifactActionDelete", destructive = true) { actionsFor = null; confirmDelete = a }
            }
        }
    }

    confirmDelete?.let { a ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text("Delete permanently?") },
            text = { Text("This can’t be undone.") },
            confirmButton = {
                TextButton(onClick = { confirmDelete = null; scope.launch { store.delete(a) } }, modifier = Modifier.testTag("artifactDeleteConfirm")) {
                    Text("Delete permanently", color = AllternitStatic.StatusError)
                }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("Cancel") } },
        )
    }
}

private const val LAYOUT_KEY = "allternit-overview-layout:phone:artifacts"

private fun ArtifactLibraryStore.filteredBeyondArchive() = kind != null || owner != ArtifactLibraryStore.Owner.All || query.isNotBlank()

private fun filterSummary(s: ArtifactLibraryStore): String = listOfNotNull(
    if (s.show == ArtifactLibraryStore.Show.Archive) "Archive" else null,
    s.kind?.let(ArtifactKinds::label),
    if (s.owner != ArtifactLibraryStore.Owner.All) s.owner.label else null,
    s.query.takeIf { it.isNotBlank() }?.let { "“$it”" },
).joinToString(" · ")

/** A Make something new card (iOS makeSection tile, 136×95 art plus label and description). */
@Composable
private fun StudioTile(k: StudioKind, selected: Boolean, busy: Boolean, onTap: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Column(
        Modifier.width(136.dp).clip(RoundedCornerShape(14.dp))
            .testTag("artifactsTile${k.raw}")
            .clickable(role = Role.Button, onClickLabel = k.description) { Haptics.perform(view, Haptics.Kind.Tap); onTap() }
            .semantics { contentDescription = "New ${k.label}" },
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier.fillMaxWidth().height(95.dp).clip(RoundedCornerShape(14.dp)).background(c.fillNeutral)
                .border(BorderStroke(if (selected) 2.dp else 1.dp, if (selected) c.textPrimary else c.borderSubtle), RoundedCornerShape(14.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(painterResource(k.deck), null, tint = c.textPrimary, modifier = Modifier.size(34.dp))
            if (k.isBeta) Text("Beta", color = c.textSecondary, fontSize = 10.sp, modifier = Modifier.align(Alignment.TopEnd).padding(6.dp))
            if (busy) Text("Starting…", color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.align(Alignment.BottomCenter).padding(6.dp))
        }
        Text(k.label, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
        Text(k.description, color = c.textSecondary, fontSize = 11.5.sp, lineHeight = 15.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ArtifactCard(a: ArtifactSummary, layout: LibraryLayout, modifier: Modifier, onOpen: () -> Unit, onMore: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val thumbH = when (layout) { LibraryLayout.List -> 58; LibraryLayout.Grid -> 110; LibraryLayout.Deck -> 180 }
    val shape = RoundedCornerShape(16.dp)
    val subtitle = "${ArtifactKinds.noun(a.kind)} · Edited ${relativeTime(parseTime(a.updatedAt)).lowercase().ifEmpty { "recently" }}"
    val body: @Composable (Modifier) -> Unit = { m ->
        ArtifactCardPreview(a, m.clip(RoundedCornerShape(10.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(10.dp)))
    }
    val click = Modifier.combinedClickable(
        role = Role.Button,
        onLongClick = { Haptics.perform(view, Haptics.Kind.Tap); onMore() },
        onClick = { Haptics.perform(view, Haptics.Kind.Tap); onOpen() },
    )
    if (layout == LibraryLayout.List) {
        Row(
            modifier.fillMaxWidth().heightIn(min = 76.dp).clip(shape).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), shape)
                .testTag("artifactCard${a.id}").then(click).padding(9.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            body(Modifier.width(96.dp).height(thumbH.dp))
            Column(Modifier.weight(1f)) {
                Text(a.title, color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(subtitle, color = c.textTertiary, fontSize = 12.sp, maxLines = 1)
            }
            LibraryIconButton(DesignR.drawable.ph_dots_three_vertical, "More for ${a.title}", "artifactMore${a.id}", iconSize = 18, onClick = onMore)
        }
    } else {
        Column(
            modifier.fillMaxWidth().clip(shape).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), shape)
                .testTag("artifactCard${a.id}").then(click).padding(10.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Box {
                body(Modifier.fillMaxWidth().height(thumbH.dp))
                Text(
                    ArtifactKinds.noun(a.kind), color = c.textPrimary, fontSize = 10.5.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.padding(6.dp).clip(CircleShape).background(c.bgPanel).padding(horizontal = 8.dp, vertical = 3.dp),
                )
            }
            Row(verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f)) {
                    Text(a.title, color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(subtitle, color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (!a.isMine) {
                    Box(Modifier.size(22.dp).clip(CircleShape).background(c.fillNeutral), contentAlignment = Alignment.Center) {
                        Text((a.owner.name ?: "?").take(1).uppercase(), color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                Spacer(Modifier.weight(1f))
                LibraryIconButton(DesignR.drawable.ph_dots_three_vertical, "More for ${a.title}", "artifactMore${a.id}", iconSize = 18, onClick = onMore)
            }
        }
    }
}
