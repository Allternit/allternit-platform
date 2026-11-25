package com.allternit.feature.artifacts

import com.allternit.core.api.gen.AgentModes
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
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.unit.Dp
import com.allternit.core.design.DockSpec
import com.allternit.core.design.DockingBlockTitle
import com.allternit.core.design.EmptyBox
import com.allternit.core.design.GlassPill
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.PolishHaptic
import com.allternit.core.design.TabPage
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
import androidx.compose.ui.text.style.TextAlign
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
        fun load(context: Context, key: String, default: LibraryLayout = Grid): LibraryLayout {
            val raw = context.getSharedPreferences("allternit_library", Context.MODE_PRIVATE).getString(key, null)
            return entries.firstOrNull { it.raw == raw } ?: default
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

enum class StudioKind(val raw: String, private val fallbackLabel: String, val description: String, val emptyTitle: String, val studioHint: String, val modeId: String?, val studioKind: String, val filterKind: String?, val deck: Int) {
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

    /** Label and Beta badge come from the generated registry (AgentModes.gen.kt), the same text iOS and web show. */
    val label: String get() = AgentModes.mode(raw)?.label ?: fallbackLabel
    val isBeta: Boolean get() = AgentModes.mode(raw)?.beta ?: false

    companion object {
        /** Registry order (the mode list order), kinds the registry lacks last. */
        /** The studio kind a registry mode id starts. App has no kind of its own: it runs as a Build session (site or app). */
        fun forMode(id: String): StudioKind? = entries.firstOrNull { it.raw == id } ?: if (id == "app") Build else null

        val ordered: List<StudioKind> by lazy {
            val order = AgentModes.all.map { it.id }
            entries.sortedBy { k -> order.indexOf(k.raw).let { if (it < 0) Int.MAX_VALUE else it } }
        }
    }

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
 * pill, "Artifacts" with "+ New" (the mode list sheet), Your artifacts in List / Grid / Deck,
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
    /** A studio session the composer just started (Artifacts redesign 6.2): the library opens straight into it. */
    initialStudio: StudioSession? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { ArtifactLibraryStore() }
    var layout by remember { mutableStateOf(LibraryLayout.load(context, LAYOUT_KEY)) }
    var searching by remember { mutableStateOf(false) }
    var filterOpen by remember { mutableStateOf(false) }
    var openId by remember { mutableStateOf(initialArtifactId) }
    var officeOpen by remember { mutableStateOf(false) }
    var actionsFor by remember { mutableStateOf<ArtifactSummary?>(null) }
    var confirmDelete by remember { mutableStateOf<ArtifactSummary?>(null) }
    var studio by remember { mutableStateOf(initialStudio) }
    var launching by remember { mutableStateOf<StudioKind?>(null) }
    var searchJob by remember { mutableStateOf<Job?>(null) }
    var newOpen by remember { mutableStateOf(false) }

    fun reload() { scope.launch { store.load() } }
    val pdfPicker = rememberPdfPicker(
        scope, onUploading = { store.notice = it },
        onUploaded = { r -> r.onSuccess { id -> store.notice = null; reload(); openId = id }.onFailure { store.notice = it.message } },
    )

    LaunchedEffect(signedIn) {
        if (signedIn) {
            store.load()
            store.loadOrgSettings()
        }
    }

    // Full-screen covers, like iOS fullScreenCover.
    ArtifactOfficeHooks.documents?.let { office ->
        if (officeOpen) { office({ officeOpen = false; reload() }); return }
    }
    openId?.let { id ->
        V2ArtifactScreen(artifactId = id, onClose = { openId = null; reload() }, onOpenChat = onOpenChat, onSignIn = onSignIn)
        return
    }
    studio?.let { s ->
        ArtifactStudioScreen(session = s, chatPane = studioChat, onClose = { studio = null; reload() }, onOpenArtifact = { openId = it })
        return
    }

    fun startSession(kind: StudioKind, sessionKind: String? = null) {
        if (launching != null) return
        launching = kind
        scope.launch {
            val r = ArtifactStudioLauncher.launch(context, kind, sessionKind = sessionKind)
            launching = null
            r.onSuccess { studio = it }.onFailure { store.notice = "Couldn't start a ${ArtifactKinds.noun(kind.studioKind).lowercase()} session. ${it.message ?: ""}".trim() }
        }
    }

    // One list for "+ New" and the empty library: the deck's modes. PDF picks a file; the rest start a session.
    fun launchMode(e: com.allternit.core.api.gen.AgentModeEntry) {
        if (e.launchType == com.allternit.core.api.gen.AgentModeLaunchType.PICK) { pdfPicker(); return }
        StudioKind.forMode(e.id)?.let { startSession(it, sessionKind = e.id.takeIf { m -> m == "app" }) }
    }

    Box(Modifier.fillMaxSize()) {
    Box(Modifier.fillMaxSize().statusBarsPadding().testTag("artifactsLibrary")) {
    val gridState = rememberLazyGridState()
    val density = LocalDensity.current.density
    val dockOffset = remember(gridState, density) {
        { if (gridState.firstVisibleItemIndex > 0) DockSpec.DOCK * 2f else gridState.firstVisibleItemScrollOffset / density }
    }
    Column(Modifier.fillMaxSize()) {
        Spacer(Modifier.height(52.dp))
        if (searching) {
            LibrarySearchField(store.query, "Search artifacts", "artifactsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) { q ->
                store.query = q
                searchJob?.cancel()
                searchJob = scope.launch { delay(300); store.load() }
            }
        }
        val modes = newArtifactModes(store.allows)
        val columns = if (layout == LibraryLayout.Grid) 2 else 1
        LazyVerticalGrid(
            columns = GridCells.Fixed(columns),
            state = gridState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(start = 14.dp, end = 14.dp, bottom = 84.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            val full: androidx.compose.foundation.lazy.grid.LazyGridItemSpanScope.() -> GridItemSpan = { GridItemSpan(maxLineSpan) }
            item(key = "title", span = full) {
                Box(Modifier.fillMaxWidth()) {
                    TabPage(
                        description = "Docs, sheets, slides and pages you can edit, share and export.",
                        modifier = Modifier.bleed(14.dp),
                        title = { Box(Modifier.padding(top = 4.dp).height(DockSpec.BH.dp).semantics { heading(); contentDescription = "Artifacts" }) },
                    ) {}
                    // R2-C: "+ New" sits on the title line and opens the mode list sheet.
                    LibraryNewPill("New artifact", "artifactsNew", Modifier.align(Alignment.TopEnd).padding(top = 4.dp)) { newOpen = true }
                }
            }
            if (ArtifactOfficeHooks.documents != null) item(key = "office", span = full) {
                GlassPill(onClick = { officeOpen = true }, modifier = Modifier.testTag("artifactsOfficeDocuments"), height = 38.dp) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                        Icon(painterResource(DesignR.drawable.deck_docs), null, tint = c.textPrimary, modifier = Modifier.size(14.dp))
                        Text("Allternit Office", color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
            item(key = "yours", span = full) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Your artifacts", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(top = 8.dp))
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
                    DashedEmpty("Sign in to see your artifacts", "Artifacts are saved to your Allternit account, so they follow you to every device.", actionTitle = "Sign in", onAction = onSignIn)
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
                            FriendlyState(FriendlyStyle.Empty, "No ${ArtifactKinds.label(k).lowercase()} yet", "Tap + New to make one.", actionTitle = "Clear filters", onAction = { store.clearFilters(); reload() }) {
                                Icon(painterResource(ArtifactKinds.icon(k)), null, tint = c.textSecondary)
                            }
                        store.show == ArtifactLibraryStore.Show.Archive && !store.filteredBeyondArchive() ->
                            FriendlyState(FriendlyStyle.Empty, "No archived artifacts", "Archived work stays available here. Restore an artifact to return it to Saved work.", art = com.allternit.core.design.EmptyArtKind.Sheets) {
                                Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                            }
                        store.filtered ->
                            FriendlyState(FriendlyStyle.Empty, "Nothing matches", "Try another filter or search.", actionTitle = "Clear filters", onAction = { store.clearFilters(); reload() }) {
                                Icon(painterResource(DesignR.drawable.ph_magnifying_glass), null, tint = c.textSecondary)
                            }
                        else -> Column(Modifier.testTag("artifactsEmpty"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                            Text("Nothing here yet", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.fillMaxWidth(), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                            Text("Pick what you want to make. It opens in the composer.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.fillMaxWidth(), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                            modes.chunked(2).forEach { pair ->
                                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                    pair.forEach { e -> ModeTile(e, busy = launching == StudioKind.forMode(e.id), modifier = Modifier.weight(1f)) { launchMode(e) } }
                                    if (pair.size == 1) Spacer(Modifier.weight(1f))
                                }
                            }
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
    DockingBlockTitle("Artifacts", dockOffset, barCenter = 26.dp, modifier = Modifier.fillMaxSize(), top = 56.dp)
    LibraryHeader(
        title = "",
        onBack = null,
        leading = leading,
        glass = true,
        trailing = {
            LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search artifacts", "artifactsSearch") { searching = !searching }
            LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Filter and layout", "artifactsFilter") { filterOpen = true }
        },
    )
    }
    }

    ArtifactsIntroHost(signedIn)

    if (newOpen) {
        NewArtifactSheet(
            modes = newArtifactModes(store.allows),
            signedIn = signedIn,
            onMode = { launchMode(it) },
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

/** A mode tile for the empty library (R2-C-empty): the deck art on the neutral fill, the label under it, Beta in the corner. */
@Composable
private fun ModeTile(e: com.allternit.core.api.gen.AgentModeEntry, busy: Boolean, modifier: Modifier = Modifier, onTap: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val s by animateFloatAsState(if (pressed) 0.94f else 1f, spring(dampingRatio = 0.49f, stiffness = 600f), label = "tilePress")
    val shape = RoundedCornerShape(14.dp)
    Column(
        modifier.graphicsLayer { scaleX = s; scaleY = s }.clip(shape)
            .testTag("artifactsTile${e.id}")
            .clickable(interactionSource = source, indication = null, role = Role.Button, onClickLabel = "New ${e.label}") {
                PolishHaptic.perform(view, PolishHaptic.Kind.Light); onTap()
            }
            .semantics { contentDescription = "New ${e.label}" },
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier.fillMaxWidth().height(95.dp).clip(shape).background(PolishGlass.track()).border(BorderStroke(1.dp, PolishGlass.border()), shape),
            contentAlignment = Alignment.Center,
        ) {
            androidx.compose.foundation.Image(painterResource(modeDeckArt(e.id)), null, Modifier.fillMaxSize().padding(3.dp))
            if (e.beta) Text("Beta", color = c.textSecondary, fontSize = 10.sp, modifier = Modifier.align(Alignment.TopEnd).padding(6.dp))
            if (busy) Text("Starting…", color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.align(Alignment.BottomCenter).padding(6.dp))
        }
        Text(e.label, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
    }
}

/** Widens a child by [by] on each side so a component with its own gutter lines up with the grid's. */
private fun Modifier.bleed(by: Dp): Modifier = layout { measurable, constraints ->
    val px = by.roundToPx() * 2
    val w = constraints.maxWidth + px
    val p = measurable.measure(constraints.copy(minWidth = w, maxWidth = w))
    layout(constraints.maxWidth, p.height) { p.place(-px / 2, 0) }
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

/** Empty state in the soft neutral box (no dashes), with an optional action pill. */
@Composable
private fun DashedEmpty(title: String, message: String, modifier: Modifier = Modifier, actionTitle: String? = null, onAction: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(20.dp)).background(PolishGlass.track()).padding(horizontal = 18.dp, vertical = 22.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Text(message, color = c.textSecondary, fontSize = 12.sp, lineHeight = 17.sp, textAlign = TextAlign.Center)
        if (actionTitle != null && onAction != null) {
            Text(actionTitle, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                modifier = Modifier.padding(top = 4.dp).clip(CircleShape).background(Color.White).clickable(role = Role.Button, onClick = onAction).padding(horizontal = 14.dp, vertical = 8.dp))
        }
    }
}
