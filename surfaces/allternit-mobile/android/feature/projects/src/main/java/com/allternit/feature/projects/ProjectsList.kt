package com.allternit.feature.projects

import com.allternit.core.design.AllternitSerif
import android.content.Context
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
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
import androidx.compose.foundation.lazy.grid.LazyGridItemSpanScope
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
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
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.ui.platform.LocalDensity
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.BlockText
import com.allternit.core.design.DockSpec
import com.allternit.core.design.DockingBlockTitle
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.EmptyBox
import com.allternit.core.design.GlassPill
import com.allternit.core.design.IntroKeys
import com.allternit.core.design.IntroSeenStore
import com.allternit.core.design.IntroSheet
import com.allternit.core.design.LocalIntroSeenStore
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.SectionLabel
import com.allternit.core.design.TabPage
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.FilterLabel
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryLayout
import com.allternit.feature.artifacts.LibraryLayoutSwitch
import com.allternit.feature.artifacts.LibraryNewButton
import com.allternit.feature.artifacts.LibraryRowButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.PillRow
import kotlinx.coroutines.launch

/** Where a project screen sends the person outside the Projects section (the shell owns chat). */
class ProjectsNav(
    /** Opens a session in its mode (`chat` or `code`). */
    val openSession: (sessionId: String, mode: String) -> Unit,
    /** The StageBackdrop the shell draws behind project screens (iOS project variant). */
    val backdrop: @Composable (scope: String) -> Unit,
    /** A GET on the user's computer through the runtime relay (the folder picker's `v1/path`, `v1/file/tree`). */
    val runtimeGet: RuntimeGet = { Result.failure(IllegalStateException("No computer")) },
    /** Wave E: a bot project opens the bot project page in Bots mode (iOS BotProjectPage); null keeps the project detail. */
    val openBotProject: ((projectId: String) -> Unit)? = null,
    /** Opens the connect-a-computer flow; the no-computer card shows its glass Connect button only when this is set. */
    val onConnect: (() -> Unit)? = null,
    /** An intro store for this screen; null uses the shell's shared [LocalIntroSeenStore] (null there too = no intro). */
    val introStore: IntroSeenStore? = null,
)

private enum class ProjectSort(val label: String) { Recent("Recent"), Name("Name"), Created("Created") }

private sealed interface ProjectPage {
    data class Chat(val project: LibraryProject) : ProjectPage
    data class Code(val project: LibraryProject, val asksForFolder: Boolean) : ProjectPage
    data class Design(val project: LibraryProject) : ProjectPage
}

/**
 * The Projects section (iOS ProjectsListView): the header with the search and filter pill, the
 * counts line, List / Grid / Deck, folder covers, the ⋯ actions (Open, Pin, Edit details,
 * Archive, Delete; a long press opens the same sheet, like the iOS context menu), New project,
 * the one-time explainer, and every empty, loading and error state with the iOS copy.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ProjectsListScreen(
    signedIn: Boolean,
    leading: @Composable () -> Unit,
    nav: ProjectsNav,
    initialProjectId: String? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val store = remember { ProjectsLibraryStore(context) }
    val looks = remember { mutableStateMapOf<String, Appearance?>() }
    var layout by remember { mutableStateOf(LibraryLayout.load(context, LAYOUT_KEY)) }
    var query by remember { mutableStateOf("") }
    var searching by remember { mutableStateOf(false) }
    var filterOpen by remember { mutableStateOf(false) }
    var show by remember { mutableStateOf<ProjectMode?>(null) }
    var sort by remember { mutableStateOf(ProjectSort.Recent) }
    var showArchived by remember { mutableStateOf(false) }
    var page by remember { mutableStateOf<ProjectPage?>(null) }
    var newOpen by remember { mutableStateOf(false) }
    var explainerOpen by remember { mutableStateOf(false) }
    var actionsFor by remember { mutableStateOf<LibraryProject?>(null) }
    var editFor by remember { mutableStateOf<LibraryProject?>(null) }
    var deleteFor by remember { mutableStateOf<LibraryProject?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var pendingOpenId by remember { mutableStateOf(initialProjectId) }
    val gridState = rememberLazyGridState()
    // The floating New project button hides while the list scrolls down and returns on scroll up (UI feedback 11-25 A10).
    var newHidden by remember { mutableStateOf(false) }
    LaunchedEffect(gridState) {
        var last = 0
        androidx.compose.runtime.snapshotFlow { gridState.firstVisibleItemIndex * 100000 + gridState.firstVisibleItemScrollOffset }.collect { p ->
            if (p > last + 8) newHidden = true else if (p < last - 8) newHidden = false
            last = p
        }
    }
    val density = LocalDensity.current.density
    val introStore = nav.introStore ?: LocalIntroSeenStore.current
    var introOpen by remember { mutableStateOf(false) }

    fun reload() { scope.launch { store.load() } }
    fun open(p: LibraryProject, asksForFolder: Boolean = false) {
        page = when (p.mode) {
            ProjectMode.Code -> ProjectPage.Code(p, asksForFolder)
            ProjectMode.Design -> ProjectPage.Design(p)
            // Bot projects open the bot project page (wave E) when the shell can show it.
            ProjectMode.Bot -> nav.openBotProject?.let { openBot -> openBot(p.nativeId); return } ?: ProjectPage.Chat(p)
            else -> ProjectPage.Chat(p)
        }
    }

    LaunchedEffect(signedIn) { if (signedIn) store.load() }
    LaunchedEffect(signedIn, introStore) {
        val intro = introStore ?: return@LaunchedEffect
        if (signedIn) {
            intro.refresh()
            if (!intro.isSeen(IntroKeys.PROJECTS)) introOpen = true
        }
    }
    LaunchedEffect(store.projects) {
        store.projects.forEach { p -> if (!looks.containsKey(p.id)) looks[p.id] = AppearanceClient.get(p.mode, p.nativeId) }
        pendingOpenId?.let { id -> store.projects.firstOrNull { it.nativeId == id || it.id == id }?.let { open(it); pendingOpenId = null } }
    }

    when (val current = page) {
        is ProjectPage.Chat -> {
            ProjectDetailScreen(current.project, store, looks[current.project.id], nav, onLookSaved = { looks[current.project.id] = it }, onClose = { page = null; reload() })
            return
        }
        is ProjectPage.Code -> {
            CodeProjectDetailScreen(current.project, store, current.asksForFolder, nav, onClose = { page = null; reload() })
            return
        }
        is ProjectPage.Design -> {
            DesignProjectCanvasScreen(current.project, onClose = { page = null; reload() })
            return
        }
        null -> Unit
    }

    val visible = store.projects
        .filter { show == null || it.mode == show }
        .filter { showArchived || !it.isArchived }
        .filter { query.isBlank() || it.title.contains(query.trim(), ignoreCase = true) }
        .let { list ->
            when (sort) {
                ProjectSort.Recent -> list.sortedWith(compareByDescending<LibraryProject> { it.isFavorite }.thenByDescending { it.updatedAt })
                ProjectSort.Name -> list.sortedBy { it.title.lowercase() }
                ProjectSort.Created -> list.sortedByDescending { it.createdAt }
            }
        }
    val openCount = store.projects.count { !it.isArchived && !it.closed }
    val working = store.projects.sumOf { it.working }
    val needs = store.projects.sumOf { it.waiting }

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().statusBarsPadding().testTag("projectsList")) {
            Box(Modifier.height(52.dp))
            if (searching) LibrarySearchField(query, "Search projects", "projectsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { query = it }
            val full: LazyGridItemSpanScope.() -> GridItemSpan = { GridItemSpan(maxLineSpan) }
            val gutter = Modifier.padding(horizontal = 14.dp)
            val cardGutter = Modifier.padding(horizontal = if (layout == LibraryLayout.Grid) 6.dp else 14.dp)
            LazyVerticalGrid(
                state = gridState,
                columns = GridCells.Fixed(if (layout == LibraryLayout.Grid) 2 else 1),
                modifier = Modifier.weight(1f).fillMaxWidth(),
                contentPadding = PaddingValues(start = if (layout == LibraryLayout.Grid) 8.dp else 0.dp, end = if (layout == LibraryLayout.Grid) 8.dp else 0.dp, top = 0.dp, bottom = 96.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp),
                verticalArrangement = Arrangement.spacedBy(if (layout == LibraryLayout.List) 8.dp else 12.dp),
            ) {
                item(key = "page", span = full) {
                    // The block title is an overlay that docks into the top bar; this spacer keeps its room.
                    TabPage(
                        description = "Keep a job's chats, tasks and board together, and see what is working.",
                        title = { Box(Modifier.height((DockSpec.TOP - 52f + 34f).dp)) },
                    ) {}
                }
                notice?.let { n -> item(key = "notice", span = full) { Text(n, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = gutter.clickable { notice = null }) } }
                // iOS: small red inline lines, one per failed source, above the grey empty text.
                if (store.noComputer) item(key = "nocomputer", span = full) {
                    val cardShape = RoundedCornerShape(16.dp)
                    Row(
                        gutter.fillMaxWidth().clip(cardShape).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), cardShape).padding(14.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Text("No computer connected. Connect a computer or start Allternit Cloud to use this.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f).testTag("projectsNoComputer"))
                        nav.onConnect?.let { connect ->
                            GlassPill(onClick = connect, height = 34.dp, modifier = Modifier.testTag("projectsConnect")) {
                                Text("Connect", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                            }
                        }
                    }
                } else store.failures.forEachIndexed { i, f ->
                    item(key = "failure$i", span = full) {
                        Text(f, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = gutter.clickable { reload() }.then(if (i == 0) Modifier.testTag("projectsError") else Modifier))
                    }
                }
                if (signedIn) item(key = "label", span = full) {
                    Column(gutter) {
                        SectionLabel("Your projects \u00b7 $openCount open", Modifier.testTag("projectsCounts"))
                        if (working > 0 || needs > 0) {
                            Text("$working working \u00b7 $needs need${if (needs == 1) "s" else ""} you", color = c.textTertiary, fontSize = 12.5.sp, modifier = Modifier.padding(bottom = 6.dp))
                        }
                    }
                }
                when {
                    !signedIn -> item(key = "signedout", span = full) {
                        FriendlyState(FriendlyStyle.Empty, "Sign in to see your projects", "Projects are saved to your Allternit account.") {
                            Icon(painterResource(DesignR.drawable.deck_docs), null, tint = c.textSecondary)
                        }
                    }
                    store.loading && store.projects.isEmpty() -> item(key = "loading", span = full) { LibrarySpinner("Loading projects") }
                    store.loaded && store.projects.isEmpty() -> item(key = "empty", span = full) {
                        EmptyBox(
                            "No projects yet",
                            "Tap New project to start one. Its chats, tasks and files stay together.",
                            gutter.testTag("projectsEmpty"),
                            art = EmptyArtKind.Sheets,
                            actionLabel = "New project",
                            onAction = {
                                val prefs = context.getSharedPreferences("allternit_projects", Context.MODE_PRIVATE)
                                if (!prefs.getBoolean(EXPLAINER_KEY, false)) explainerOpen = true else newOpen = true
                            },
                        )
                    }
                    store.loaded && visible.isEmpty() && store.projects.isNotEmpty() -> item(key = "noresults", span = full) {
                        Text("No projects match your search.", color = c.textSecondary, fontSize = 15.sp, modifier = gutter.padding(vertical = 24.dp))
                    }
                    layout == LibraryLayout.Deck -> item(key = "deck", span = full) {
                        LazyRow(contentPadding = PaddingValues(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            items(visible, key = { it.id }) { p -> ProjectCard(p, looks[p.id], LibraryLayout.Deck, Modifier.width(300.dp), { open(p) }, { actionsFor = p }) }
                        }
                    }
                    else -> items(visible, key = { it.id }) { p -> ProjectCard(p, looks[p.id], layout, cardGutter, { open(p) }, { actionsFor = p }) }
                }
            }
        }
        // The block title docks into the bar as the page scrolls; the header sits on top of its band.
        val dockOffset = remember(gridState, density) {
            { if (gridState.firstVisibleItemIndex > 0) DockSpec.DOCK * 2f else gridState.firstVisibleItemScrollOffset / density }
        }
        DockingBlockTitle("Projects", dockOffset, modifier = Modifier.fillMaxSize().statusBarsPadding())
        Column(Modifier.statusBarsPadding()) {
            LibraryHeader(
                title = "", onBack = null, leading = leading, glass = true,
                trailing = {
                    LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search projects", "projectsSearch") { searching = !searching }
                    LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Filter and layout", "projectsFilter") { filterOpen = true }
                },
            )
        }
        // iOS ProjectsNewButton: the floating "New project" capsule.
        androidx.compose.animation.AnimatedVisibility(
            visible = !newHidden,
            modifier = Modifier.align(Alignment.BottomCenter),
            enter = androidx.compose.animation.fadeIn() + androidx.compose.animation.slideInVertically { it },
            exit = androidx.compose.animation.fadeOut() + androidx.compose.animation.slideOutVertically { it },
        ) {
        GlassPill(
            onClick = {
                val prefs = context.getSharedPreferences("allternit_projects", Context.MODE_PRIVATE)
                if (!prefs.getBoolean(EXPLAINER_KEY, false)) explainerOpen = true else newOpen = true
            },
            modifier = Modifier.navigationBarsPadding().padding(bottom = 16.dp).testTag("projectsNew"),
            height = 50.dp,
            contentPadding = PaddingValues(horizontal = 22.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
                Text("New project", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            }
        }
        }
        if (introOpen) IntroSheet(
            title = "Keep each job in one place.",
            line = "A project keeps its chats, tasks and board together. See what's working and what is waiting on you.",
            onContinue = { introOpen = false; scope.launch { introStore?.markSeen(IntroKeys.PROJECTS) } },
            eyebrow = { BlockText("PROJECTS", height = 10.dp, color = PolishGlass.Coral) },
            hero = { ProjectsIntroHero() },
        )
        if (explainerOpen) ProjectsExplainer {
            context.getSharedPreferences("allternit_projects", Context.MODE_PRIVATE).edit().putBoolean(EXPLAINER_KEY, true).apply()
            explainerOpen = false
            newOpen = true
        }
    }

    if (filterOpen) {
        LibrarySheet("Filter and layout", "projectsFilterSheet", onDismiss = { filterOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { filterOpen = false }) {
            Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState())) {
                FilterLabel("Layout")
                LibraryLayoutSwitch(layout) { layout = it; LibraryLayout.save(context, LAYOUT_KEY, it) }
                FilterLabel("Show")
                PillRow(listOf<ProjectMode?>(null) + ProjectMode.entries, show, { it?.label ?: "All Projects" }, { "projectsShow${it?.raw ?: "all"}" }) { show = it }
                FilterLabel("Sort by")
                PillRow(ProjectSort.entries, sort, { it.label }, { "projectsSort${it.name}" }) { sort = it }
                FilterLabel("Show archived ${store.projects.count { it.isArchived }}")
                PillRow(listOf(false, true), showArchived, { if (it) "On" else "Off" }, { "projectsArchived$it" }) { showArchived = it }
            }
        }
    }

    if (newOpen) {
        NewProjectSheet(onDismiss = { newOpen = false }) { mode, title, description ->
            when (mode) {
                ProjectMode.Chat -> store.createChat(title, description).map { open(it) }
                ProjectMode.Code, ProjectMode.Design -> store.createWorkspace(mode, title, description).map { open(it, asksForFolder = mode == ProjectMode.Code) }
                ProjectMode.Bot -> store.createAccountBot(title, description)
            }
        }
    }

    actionsFor?.let { p ->
        LibrarySheet(p.title, "projectActions", onDismiss = { actionsFor = null }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibraryRowButton("Open", DesignR.drawable.ph_arrow_square_out, "projectActionOpen") { actionsFor = null; open(p) }
                LibraryRowButton(if (p.isFavorite) "Unpin" else "Pin", DesignR.drawable.ph_seal_check, "projectActionPin") { actionsFor = null; scope.launch { store.setFavorite(p, !p.isFavorite) } }
                LibraryRowButton("Edit details", DesignR.drawable.ph_sliders_horizontal, "projectActionEdit") { actionsFor = null; editFor = p }
                LibraryRowButton(if (p.isArchived) "Unarchive" else "Archive", DesignR.drawable.ph_floppy_disk, "projectActionArchive") { actionsFor = null; scope.launch { store.setArchived(p, !p.isArchived) } }
                LibraryRowButton("Delete", DesignR.drawable.ph_trash, "projectActionDelete", destructive = true) { actionsFor = null; deleteFor = p }
            }
        }
    }
    editFor?.let { p -> ProjectDetailsSheet(p, looks[p.id], store, onDismiss = { editFor = null }) }
    deleteFor?.let { p ->
        AlertDialog(
            onDismissRequest = { deleteFor = null },
            title = { Text("Delete Project") },
            text = { Text("Delete \"${p.title}\"? This cannot be undone.") },
            confirmButton = {
                TextButton(onClick = { deleteFor = null; scope.launch { store.delete(p).onFailure { notice = it.message } } }, modifier = Modifier.testTag("projectDeleteConfirm")) {
                    Text("Delete", color = AllternitStatic.StatusError)
                }
            },
            dismissButton = { TextButton(onClick = { deleteFor = null }) { Text("Cancel") } },
        )
    }
}

private const val LAYOUT_KEY = "allternit-overview-layout:phone:projects"
private const val EXPLAINER_KEY = "allternit.nudge.projects-explainer"

/** iOS card tone: Archived, Closed, Blocked, "N waiting", "N working". */
private fun tone(p: LibraryProject): String? = when {
    p.isArchived -> "Archived"
    p.closed -> "Closed"
    p.state == "blocked" -> "Blocked"
    p.waiting > 0 -> "${p.waiting} waiting"
    p.working > 0 -> "${p.working} working"
    else -> null
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ProjectCard(p: LibraryProject, look: Appearance?, layout: LibraryLayout, modifier: Modifier, onOpen: () -> Unit, onMore: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val ring = when { p.waiting > 0 -> AllternitStatic.StatusWarning; p.working > 0 -> com.allternit.core.design.AllternitStatic.AccentChat; else -> c.borderSubtle }
    val t = tone(p)
    val preview = p.nextAction?.takeIf { it.isNotBlank() } ?: p.description?.takeIf { it.isNotBlank() } ?: p.countLine.replaceFirstChar { it.uppercase() }
    val label = "${p.title}, ${p.mode.label} project" + (t?.let { ", $it" } ?: "")
    val shape = RoundedCornerShape(if (layout == LibraryLayout.List) 12.dp else 14.dp)
    val base = modifier.fillMaxWidth().clip(shape).background(c.bgPanel).border(BorderStroke(1.dp, ring), shape)
        .alpha(if (p.isArchived || p.closed) 0.6f else 1f)
        .testTag("projectCard${p.id}")
        .combinedClickable(role = Role.Button, onLongClick = { Haptics.perform(view, Haptics.Kind.Tap); onMore() }, onClick = { Haptics.perform(view, Haptics.Kind.Tap); onOpen() })
        .semantics(mergeDescendants = true) { contentDescription = label }
    val note: @Composable () -> Unit = {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            if (p.isFavorite) Text("★", color = Color(0xFFEAB308), fontSize = 12.sp)
            Text("${p.mode.label} · ${p.countLine}", color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            when (p.state) {
                "at_risk" -> Text("At risk", color = AllternitStatic.StatusWarning, fontSize = 12.sp)
                "needs_decision" -> Text("Needs decision", color = AllternitStatic.StatusWarning, fontSize = 12.sp)
                "done" -> Text("Done", color = c.textTertiary, fontSize = 12.sp)
            }
        }
    }
    val more: @Composable () -> Unit = { LibraryIconButton(DesignR.drawable.ph_dots_three_vertical, "More for ${p.title}", "projectMore${p.id}", iconSize = 18, onClick = onMore) }
    if (layout == LibraryLayout.List) {
        Row(base.heightIn(min = 60.dp).padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            FolderCover(look, p.title, p.mode, width = 30.dp, flat = true)
            Column(Modifier.weight(1f)) {
                Text(p.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                note()
            }
            t?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 3.dp)) }
            more()
        }
    } else {
        val deck = layout == LibraryLayout.Deck
        Column(base.heightIn(min = if (deck) 360.dp else 250.dp).padding(if (deck) 16.dp else 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { FolderCover(look, p.title, p.mode, width = if (deck) 132.dp else 100.dp) }
            Text(p.title, color = c.textPrimary, fontSize = if (deck) 17.sp else 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            note()
            Text(preview, color = c.textSecondary, fontSize = 13.sp, maxLines = if (deck) 3 else 2, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(com.allternit.feature.artifacts.relativeTime(p.updatedAt), color = c.textTertiary, fontSize = 11.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
                t?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 3.dp)) }
                more()
            }
        }
    }
}

/** iOS ProjectsExplainerCard: once, before the first New project. */
@Composable
private fun ProjectsExplainer(onOk: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.28f)).clickable(onClick = onOk), contentAlignment = Alignment.Center) {
        Column(
            Modifier.padding(horizontal = 28.dp).clip(RoundedCornerShape(24.dp)).background(c.bgPrimary).padding(24.dp).testTag("projectsExplainer"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text("Every project is a folder your team works in", color = c.textPrimary, fontSize = 20.sp, fontFamily = AllternitSerif)
            Text("Put chats, code, designs and bots in one project. Everyone on it shares the same files, context and instructions.", color = c.textSecondary, fontSize = 15.sp)
            Text(
                "OK", color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.align(Alignment.End).clip(CircleShape).background(c.textPrimary).testTag("projectsExplainerOk").clickable(onClick = onOk).padding(horizontal = 22.dp, vertical = 10.dp),
            )
        }
    }
}

/**
 * iOS NewProjectSheet: ✕, "New project", ✓; the live cover; Chat / Code / Design / Bot; Name and
 * Description (Brief for a bot); Create project / Create bot. [create] runs the mode's call.
 */
@Composable
fun NewProjectSheet(onDismiss: () -> Unit, create: suspend (ProjectMode, String, String?) -> Result<Unit>) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var mode by remember { mutableStateOf(ProjectMode.Chat) }
    var name by remember { mutableStateOf("") }
    var description by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val valid = name.trim().isNotEmpty() && !busy
    fun submit() {
        if (!valid) return
        busy = true
        error = null
        scope.launch {
            val r = create(mode, name.trim(), description.trim().ifEmpty { null })
            busy = false
            r.onSuccess { onDismiss() }.onFailure { error = it.message ?: "Couldn't create the project. Try again." }
        }
    }
    LibrarySheet(
        "New project", "newProjectSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("✕", testTag = "newProjectClose", onClick = onDismiss),
        trailing = LibrarySheetAction("✓", enabled = valid, bold = true, testTag = "newProjectCreate", onClick = ::submit),
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("New project", color = c.textPrimary, fontSize = 22.sp, fontFamily = AllternitSerif, modifier = Modifier.testTag("newProjectTitle"))
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { FolderCover(null, name.ifBlank { "the new project" }, mode, width = 120.dp) }
            PillRow(ProjectMode.entries, mode, { it.label }, { "newProjectMode${it.raw}" }) { mode = it }
            LibraryField("Name", name, "Project name", "newProjectName") { name = it }
            LibraryField(
                if (mode == ProjectMode.Bot) "Brief" else "Description", description,
                if (mode == ProjectMode.Bot) "What should this bot do? (optional)" else "What is this project about? (optional)",
                "newProjectDescription", singleLine = false, minHeight = 88,
            ) { description = it }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            Box(
                Modifier.fillMaxWidth().height(48.dp).clip(RoundedCornerShape(14.dp)).background(if (valid) c.accentPrimary else c.fillNeutral)
                    .testTag("newProjectSubmit").clickable(enabled = valid, role = Role.Button, onClick = ::submit),
                contentAlignment = Alignment.Center,
            ) {
                Text(if (busy) "Creating…" else if (mode == ProjectMode.Bot) "Create bot" else "Create project", color = if (valid) c.onAccent else c.textTertiary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

/** iOS ProjectDetailSettingsSheet: the cover, Name, Description; ✓ saves through PATCH. */
@Composable
fun ProjectDetailsSheet(p: LibraryProject, look: Appearance?, store: ProjectsLibraryStore, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(p.title) }
    var description by remember { mutableStateOf(p.description ?: "") }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LibrarySheet(
        "Project settings", "projectDetailsSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("✕", testTag = "projectDetailsClose", onClick = onDismiss),
        trailing = LibrarySheetAction("✓", enabled = name.trim().isNotEmpty() && !saving, bold = true, testTag = "projectDetailsSave") {
            saving = true
            scope.launch {
                val r = store.saveDetails(p, name.trim(), description.trim().ifEmpty { null })
                saving = false
                r.onSuccess { onDismiss() }.onFailure { error = it.message }
            }
        },
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { FolderCover(look, name, p.mode, width = 110.dp) }
            LibraryField("Name", name, "Name this project", "projectDetailsName") { name = it }
            LibraryField("Description", description, "What is this project about?", "projectDetailsDescription", singleLine = false, minHeight = 88) { description = it }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
        }
    }
}
