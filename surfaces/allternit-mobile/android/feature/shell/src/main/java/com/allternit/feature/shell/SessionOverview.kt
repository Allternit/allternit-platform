package com.allternit.feature.shell

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlin.math.roundToInt
import kotlinx.coroutines.launch

/** List · Grid · Deck (iOS OverviewLayout), kept across launches (`allternit-phone-overview-layout`). */
internal enum class OverviewLayout(val raw: String, val label: String, val icon: Int) {
    List("list", "List", R.drawable.shell_list),
    Grid("grid", "Grid", R.drawable.shell_grid),
    Deck("deck", "Deck", R.drawable.shell_stack),
}

/** All · Pinned · Archived (iOS OverviewView). */
internal enum class OverviewView(val label: String) { All("All"), Pinned("Pinned"), Archived("Archived") }

private val Coral = Color(0xFFD97757)

/** The session's surface glyph (iOS ShellSearchView.icon). */
internal fun sessionIcon(session: AgentSession): Int = when {
    session.agentId != null -> DesignR.drawable.ph_cpu
    session.originSurface == "code" -> DesignR.drawable.ph_terminal
    session.originSurface == "browser" -> DesignR.drawable.ph_globe
    else -> R.drawable.shell_chat
}

/** The session's home in words (iOS ShellSearchView.surfaceLabel). */
internal fun sessionSurfaceLabel(session: AgentSession): String = when {
    session.agentId != null -> "Bots"
    session.originSurface == "code" -> "Code"
    session.originSurface == "browser" -> "ACI"
    else -> "Home"
}

/**
 * The current mode's open sessions as a list, grid or deck of cards, full screen (iOS
 * SessionOverviewView, web PhoneOverviewOverlay). Opened from "View all ›" over Recents.
 * Header: back · title · ⋯ ("Recents options": layout, Select chats, All · Pinned · Archived);
 * modes without select (ACI) get the layout switch behind a sliders button. Select mode swaps to
 * checkbox rows and a bottom bar of Archive (Unarchive) · Pin (Unpin) · Delete. A floating
 * search field and the New pill sit at the bottom. [isWorking] marks a session streaming now.
 * The filter button (shown when there is more than one group) narrows to one project ("repos" in
 * Code). Each card shows where it runs (Local / Cloud, `teleports/placements`); Code cards'
 * ⋯ adds Teleport to cloud / Bring back to this Mac. "New reply" marks activity since last seen.
 */
@Composable
internal fun SessionOverviewScreen(
    mode: ShellMode,
    store: ShellSessionsStore,
    isWorking: (String) -> Boolean,
    onBack: () -> Unit,
    onOpen: (AgentSession) -> Unit,
    onNew: () -> Unit,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val prefs = remember { context.getSharedPreferences("allternit_shell", android.content.Context.MODE_PRIVATE) }
    var layoutRaw by remember { mutableStateOf(prefs.getString(LAYOUT_KEY, OverviewLayout.Grid.raw) ?: OverviewLayout.Grid.raw) }
    var pins by remember { mutableStateOf(RailPinPrefs.pinned(context)) }
    var viewFilter by remember { mutableStateOf(OverviewView.All) }
    var selecting by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf<Set<String>>(emptySet()) }
    var optionsOpen by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var editingId by remember { mutableStateOf<String?>(null) }
    var draft by remember { mutableStateOf("") }
    var actionError by remember { mutableStateOf<String?>(null) }
    /** null = all projects. */
    var group by remember { mutableStateOf<String?>(null) }
    var filterOpen by remember { mutableStateOf(false) }
    var projects by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    androidx.compose.runtime.LaunchedEffect(Unit) {
        projects = SearchSources.list("cowork/projects", "projects", "title").associate { it.id to it.title }
    }
    fun groupLabel(s: AgentSession): String = s.projectId?.let { projects[it] } ?: "No project"
    val working: (String) -> Boolean = { id -> isWorking(id) || store.isStreaming(id) }

    val hasOptions = mode == ShellMode.Home || mode == ShellMode.Code
    fun selectable(s: AgentSession) = hasOptions && !s.isTerminal
    val selectLabel = if (mode == ShellMode.Code) "Select threads" else "Select chats"
    val title = when (mode) {
        ShellMode.Code -> "Code"
        ShellMode.Aci -> "Agent runs"
        else -> "Chats"
    }
    val newLabel = when (mode) {
        ShellMode.Code -> "New thread"
        ShellMode.Aci -> "New session"
        else -> "New chat"
    }
    val layout = if (selecting) OverviewLayout.List else OverviewLayout.entries.firstOrNull { it.raw == layoutRaw } ?: OverviewLayout.Grid
    val editable = mode != ShellMode.Aci && !selecting && viewFilter != OverviewView.Archived

    // Working sessions first, then the most recent (useModeOverview `order`).
    val items = if (viewFilter == OverviewView.Archived) {
        store.archivedOf(mode)
    } else {
        store.of(mode).filter { viewFilter != OverviewView.Pinned || it.id in pins }.sortedByDescending { working(it.id) }
    }
    val groups = items.groupingBy(::groupLabel).eachCount().let { counts -> items.map(::groupLabel).distinct().map { it to (counts[it] ?: 0) } }
    val needle = query.trim().lowercase()
    val shown = items.filter { s ->
        (group == null || groupLabel(s) == group) &&
            (needle.isEmpty() || s.title.lowercase().contains(needle) || s.description.orEmpty().lowercase().contains(needle))
    }
    // Ticks only cover what is on screen (reduceOverviewSelect .visible).
    if (selecting) {
        val visible = shown.map { it.id }.toSet()
        if (!visible.containsAll(selected)) selected = selected intersect visible
    }

    val emptyText = when (viewFilter) {
        OverviewView.Pinned -> "No pinned ${if (mode == ShellMode.Code) "threads" else "chats"} yet."
        OverviewView.Archived -> "Nothing archived."
        OverviewView.All -> when (mode) {
            ShellMode.Code -> "No code threads yet. Start one and it shows here."
            ShellMode.Aci -> "No agent runs yet."
            else -> "No open chats. Start one and it shows here."
        }
    }

    fun setLayout(l: OverviewLayout) {
        layoutRaw = l.raw
        prefs.edit().putString(LAYOUT_KEY, l.raw).apply()
    }

    fun open(s: AgentSession) {
        Haptics.perform(view, Haptics.Kind.Tap)
        onOpen(s)
    }

    fun tap(s: AgentSession) {
        if (editingId == s.id) return
        if (selecting) {
            if (!selectable(s)) return
            selected = if (s.id in selected) selected - s.id else selected + s.id
        } else {
            open(s)
        }
    }

    fun close(s: AgentSession) {
        Haptics.perform(view, Haptics.Kind.Tap)
        scope.launch { store.setArchived(s.id, true)?.let { actionError = it } }
    }

    fun commitRename(s: AgentSession) {
        if (editingId != s.id) return
        val next = draft.trim()
        editingId = null
        if (next.isEmpty() || next == s.name.orEmpty()) return
        scope.launch { store.rename(s.id, next)?.let { actionError = it } }
    }

    fun bulk(work: suspend (String) -> String?) {
        val ids = selected
        selecting = false
        selected = emptySet()
        scope.launch {
            var failure: String? = null
            for (id in ids) {
                val e = work(id)
                if (failure == null) failure = e
            }
            failure?.let { actionError = it }
        }
    }

    fun teleport(s: AgentSession, toCloud: Boolean) {
        Haptics.perform(view, Haptics.Kind.Tap)
        scope.launch { store.teleport(s.id, toCloud)?.let { actionError = it } }
    }

    val cardActions = CardActions(
        editable = editable,
        groupLabel = ::groupLabel,
        place = { store.cloudPlaces[it] },
        unread = { store.isUnread(it) },
        teleport = if (mode == ShellMode.Code) ({ s, toCloud -> teleport(s, toCloud) }) else null,
        editingId = editingId,
        draft = draft,
        onDraft = { draft = it.take(120) },
        onCommit = ::commitRename,
        onOpen = ::open,
        onRename = { s ->
            draft = s.name.orEmpty()
            editingId = s.id
        },
        onClose = ::close,
    )

    Column(Modifier.fillMaxSize().statusBarsPadding().background(c.bgPrimary).testTag("phoneOverview")) {
        if (selecting) {
            val ids = shown.filter(::selectable).map { it.id }
            val all = ids.isNotEmpty() && selected.containsAll(ids)
            Row(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                HeaderText("Cancel", Alignment.CenterStart) {
                    selecting = false
                    selected = emptySet()
                }
                Text(
                    if (selected.isEmpty()) selectLabel else "${selected.size} selected",
                    color = c.textPrimary,
                    fontSize = 17.sp,
                    fontWeight = FontWeight.SemiBold,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.weight(1f).semantics { heading() },
                )
                HeaderText(if (all) "Clear" else "Select all", Alignment.CenterEnd) {
                    selected = if (all) selected - ids.toSet() else selected + ids
                }
            }
        } else {
            PhoneHeader(
                title = title,
                wideSides = groups.size > 1,
                leading = { PhoneIconButton(DesignR.drawable.ph_caret_left, "Back", testTag = "shellBack", onClick = onBack) },
                trailing = {
                    if (groups.size > 1) {
                        Box {
                            PhoneIconButton(R.drawable.shell_list, "Filter", testTag = "overviewFilter", iconSize = 20) { filterOpen = true }
                            if (group != null) Box(Modifier.align(Alignment.TopEnd).padding(8.dp).size(7.dp).clip(CircleShape).background(c.accentPrimary))
                            DropdownMenu(
                                expanded = filterOpen,
                                onDismissRequest = { filterOpen = false },
                                shape = RoundedCornerShape(16.dp),
                                containerColor = c.bgPrimary,
                                modifier = Modifier.width(260.dp),
                            ) {
                                val noun = if (mode == ShellMode.Code) "repos" else "projects"
                                MenuRow(null, "All $noun (${items.size})", checked = group == null, testTag = "overviewGroupAll") {
                                    filterOpen = false
                                    group = null
                                }
                                groups.forEach { (label, count) ->
                                    MenuRow(null, "$label ($count)", checked = group == label) {
                                        filterOpen = false
                                        group = label
                                    }
                                }
                            }
                        }
                    }
                    Box {
                        if (hasOptions) {
                            PhoneIconButton(R.drawable.shell_dots, "Recents options", testTag = "recentsOptions", iconSize = 20) { optionsOpen = true }
                        } else {
                            PhoneIconButton(DesignR.drawable.ph_sliders_horizontal, "Layout", testTag = "overviewLayout", iconSize = 20) { optionsOpen = true }
                        }
                        DropdownMenu(
                            expanded = optionsOpen,
                            onDismissRequest = { optionsOpen = false },
                            shape = RoundedCornerShape(16.dp),
                            containerColor = c.bgPrimary,
                            modifier = Modifier.width(240.dp),
                        ) {
                            LayoutSwitch(layout) { setLayout(it) }
                            if (hasOptions) {
                                HorizontalDivider(color = c.borderSubtle, modifier = Modifier.padding(vertical = 4.dp))
                                MenuRow(DesignR.drawable.ph_check_circle_fill, selectLabel, testTag = "overviewSelect") {
                                    optionsOpen = false
                                    if (shown.isNotEmpty()) {
                                        selecting = true
                                        selected = emptySet()
                                    }
                                }
                                HorizontalDivider(color = c.borderSubtle, modifier = Modifier.padding(vertical = 4.dp))
                                OverviewView.entries.forEach { v ->
                                    MenuRow(null, v.label, checked = viewFilter == v, testTag = "overviewView${v.name}") {
                                        optionsOpen = false
                                        viewFilter = v
                                        selecting = false
                                        selected = emptySet()
                                    }
                                }
                            }
                        }
                    }
                },
            )
        }
        Text(
            "${shown.size} ${when (viewFilter) { OverviewView.Archived -> "ARCHIVED"; OverviewView.Pinned -> "PINNED"; else -> "OPEN" }}",
            color = c.textTertiary,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = 0.72.sp,
            modifier = Modifier.padding(start = 16.dp, bottom = 8.dp),
        )
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when {
                !store.hasLoaded && store.isLoading -> CircularProgressIndicator(Modifier.align(Alignment.Center).size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
                shown.isEmpty() -> Text(
                    if (query.isEmpty()) emptyText else "Nothing matches “$query”.",
                    color = c.textSecondary,
                    fontSize = 14.sp,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.align(Alignment.Center).padding(horizontal = 32.dp).testTag("overviewEmpty"),
                )
                layout == OverviewLayout.List -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(top = 4.dp, bottom = 96.dp)) {
                    items(shown, key = { it.id }) { s ->
                        OverviewListRow(
                            s,
                            working = working(s.id),
                            selecting = selecting,
                            selectable = selectable(s),
                            ticked = s.id in selected,
                            actions = cardActions,
                            onTap = { tap(s) },
                        )
                    }
                }
                layout == OverviewLayout.Grid -> LazyVerticalGrid(
                    columns = GridCells.Fixed(2),
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 96.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(shown, key = { it.id }) { s ->
                        OverviewCard(s, tall = false, working = working(s.id), actions = cardActions, onTap = { tap(s) })
                    }
                }
                else -> LazyRow(
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 96.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    items(shown, key = { it.id }) { s ->
                        DeckCard(s, working = working(s.id), actions = cardActions, onTap = { tap(s) }, onSwipeClose = { close(s) })
                    }
                }
            }
            if (selecting) {
                val none = selected.isEmpty()
                val archivedView = viewFilter == OverviewView.Archived
                val pinAll = selected.any { it !in pins }
                Row(
                    Modifier
                        .align(Alignment.BottomCenter)
                        .navigationBarsPadding()
                        .padding(horizontal = 16.dp, vertical = 12.dp)
                        .fillMaxWidth()
                        .shadow(8.dp, RoundedCornerShape(28.dp))
                        .clip(RoundedCornerShape(28.dp))
                        .background(c.bgPrimary)
                        .border(1.dp, c.borderSubtle, RoundedCornerShape(28.dp))
                        .padding(6.dp)
                        .testTag("overviewSelectBar"),
                ) {
                    BarButton(if (archivedView) "Unarchive" else "Archive", R.drawable.shell_tray, none) {
                        Haptics.perform(view, Haptics.Kind.Tap)
                        bulk { id -> store.setArchived(id, !archivedView) }
                    }
                    BarButton(if (pinAll) "Pin" else "Unpin", R.drawable.shell_star, none) {
                        pins = if (pinAll) pins + selected else pins - selected
                        RailPinPrefs.set(context, pins)
                        selecting = false
                        selected = emptySet()
                    }
                    BarButton("Delete", DesignR.drawable.ph_trash, none, destructive = true) { confirmDelete = true }
                }
            } else {
                OverviewFloatingBar(
                    query = query,
                    placeholder = "Search ${title.lowercase()}",
                    newLabel = newLabel,
                    onQuery = { query = it },
                    onNew = onNew,
                    modifier = Modifier.align(Alignment.BottomCenter),
                )
            }
        }
    }

    if (confirmDelete) {
        val n = selected.size
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            containerColor = c.bgPrimary,
            title = { Text("Delete $n ${if (n == 1) "item" else "items"}? This can't be undone.", color = c.textPrimary, fontSize = 17.sp) },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    Haptics.perform(view, Haptics.Kind.Warning)
                    val ids = selected
                    pins = pins - ids
                    RailPinPrefs.set(context, pins)
                    bulk { id -> store.delete(id) }
                }, modifier = Modifier.testTag("overviewDeleteConfirm")) { Text("Delete", color = AllternitStatic.StatusError) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel", color = c.textPrimary) } },
        )
    }
    actionError?.let { message ->
        AlertDialog(
            onDismissRequest = { actionError = null },
            containerColor = c.bgPrimary,
            title = { Text("Couldn't update the chat", color = c.textPrimary) },
            text = { Text(message, color = c.textSecondary) },
            confirmButton = { TextButton(onClick = { actionError = null }) { Text("OK", color = c.textPrimary) } },
        )
    }
}

private const val LAYOUT_KEY = "allternit-phone-overview-layout"

/** What a card can do (open, rename in place, archive). */
private class CardActions(
    val editable: Boolean,
    val groupLabel: (AgentSession) -> String,
    /** The cloud computer's label when the session runs there; null = local. */
    val place: (String) -> String?,
    val unread: (AgentSession) -> Boolean,
    /** Code only: move the session to the cloud (true) or back to the Mac (false). */
    val teleport: ((AgentSession, Boolean) -> Unit)?,
    val editingId: String?,
    val draft: String,
    val onDraft: (String) -> Unit,
    val onCommit: (AgentSession) -> Unit,
    val onOpen: (AgentSession) -> Unit,
    val onRename: (AgentSession) -> Unit,
    val onClose: (AgentSession) -> Unit,
)

@Composable
private fun HeaderText(title: String, align: Alignment, onClick: () -> Unit) {
    Box(Modifier.width(88.dp).heightIn(min = 44.dp).clickable(role = Role.Button, onClick = onClick), contentAlignment = align) {
        Text(title, color = AllternitTheme.colors.textPrimary, fontSize = 16.sp)
    }
}

/** List · Grid · Deck segmented switch (iOS OverviewLayoutSwitch). */
@Composable
private fun LayoutSwitch(current: OverviewLayout, onPick: (OverviewLayout) -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.padding(horizontal = 12.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(3.dp),
        horizontalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        OverviewLayout.entries.forEach { l ->
            val on = l == current
            Row(
                Modifier
                    .weight(1f)
                    .clip(RoundedCornerShape(8.dp))
                    .background(if (on) c.bgPrimary else Color.Transparent)
                    .testTag("overviewLayout${l.raw}")
                    .clickable(role = Role.RadioButton) { onPick(l) }
                    .semantics { stateDescription = if (on) "Selected" else "Not selected" }
                    .heightIn(min = 36.dp),
                horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(painterResource(l.icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(14.dp))
                Text(l.label, color = c.textPrimary, fontSize = 13.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
            }
        }
    }
}

@Composable
private fun androidx.compose.foundation.layout.RowScope.BarButton(title: String, icon: Int, disabled: Boolean, destructive: Boolean = false, onClick: () -> Unit) {
    val tint = if (destructive) AllternitStatic.StatusError else AllternitTheme.colors.textPrimary
    Column(
        Modifier
            .weight(1f)
            .heightIn(min = 48.dp)
            .clip(RoundedCornerShape(22.dp))
            .clickable(enabled = !disabled, role = Role.Button, onClick = onClick)
            .alpha(if (disabled) 0.4f else 1f),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.Center,
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
        Text(title, color = tint, fontSize = 11.sp, fontWeight = FontWeight.Medium)
    }
}

/** OverviewStatusPill: "Working" with a pulsing coral dot. */
@Composable
private fun WorkingPill() {
    val pulse by rememberInfiniteTransition(label = "working").animateFloat(
        initialValue = 1f,
        targetValue = 0.35f,
        animationSpec = infiniteRepeatable(tween(900), RepeatMode.Reverse),
        label = "dot",
    )
    Row(
        Modifier.clip(CircleShape).background(Coral.copy(alpha = 0.13f)).padding(horizontal = 6.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Box(Modifier.size(6.dp).alpha(pulse).clip(CircleShape).background(Coral))
        Text("Working", color = Coral, fontSize = 10.5.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** "Working" while streaming, else "New reply" when there is unseen activity (OverviewStatusPill). */
@Composable
private fun StatusPill(s: AgentSession, working: Boolean, actions: CardActions) {
    if (working) {
        WorkingPill()
    } else if (actions.unread(s)) {
        Text("New reply", color = AllternitTheme.colors.textTertiary, fontSize = 10.5.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(vertical = 2.dp))
    }
}

/** Local / Cloud: where the session runs, as on the web session header. */
@Composable
private fun PlaceBadge(s: AgentSession, actions: CardActions) {
    val c = AllternitTheme.colors
    val label = actions.place(s.id)
    Row(
        Modifier.semantics { contentDescription = label?.let { "Running on $it" } ?: "Running on this Mac" },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Icon(painterResource(if (label == null) DesignR.drawable.ph_desktop else DesignR.drawable.ph_globe), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(10.dp))
        Text(if (label == null) "Local" else "Cloud", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
private fun TitleOrField(s: AgentSession, actions: CardActions, size: Float, lines: Int, modifier: Modifier) {
    val c = AllternitTheme.colors
    if (actions.editingId == s.id) {
        BasicTextField(
            value = actions.draft,
            onValueChange = actions.onDraft,
            singleLine = true,
            textStyle = TextStyle(color = c.textPrimary, fontSize = size.sp, fontWeight = FontWeight.SemiBold, fontFamily = ShellSans),
            cursorBrush = SolidColor(c.textPrimary),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            keyboardActions = KeyboardActions(onDone = { actions.onCommit(s) }),
            modifier = modifier
                .clip(RoundedCornerShape(6.dp))
                .background(c.bgPrimary)
                .border(1.dp, c.accentPrimary, RoundedCornerShape(6.dp))
                .padding(horizontal = 6.dp, vertical = 2.dp)
                .testTag("overviewRename")
                .semantics { contentDescription = "Rename" },
        )
    } else {
        Text(s.title, color = c.textPrimary, fontSize = size.sp, fontWeight = FontWeight.SemiBold, maxLines = lines, overflow = TextOverflow.Ellipsis, modifier = modifier)
    }
}

/** ⋯ (Open, Rename, Archive; a tap opens it) and the X (archive). */
@Composable
private fun CardControls(s: AgentSession, actions: CardActions, deck: Boolean) {
    val c = AllternitTheme.colors
    var menu by remember { mutableStateOf(false) }
    Box {
        Box(
            Modifier.size(28.dp).clickable(role = Role.Button, onClickLabel = "More") { menu = true }.semantics { contentDescription = "More for ${s.title}" },
            contentAlignment = Alignment.Center,
        ) { Icon(painterResource(R.drawable.shell_dots), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(if (deck) 15.dp else 13.dp)) }
        DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, shape = RoundedCornerShape(16.dp), containerColor = c.bgPrimary) {
            MenuRow(DesignR.drawable.ph_arrow_square_out, "Open") { menu = false; actions.onOpen(s) }
            MenuRow(R.drawable.shell_note_pencil, "Rename") { menu = false; actions.onRename(s) }
            actions.teleport?.let { move ->
                val inCloud = actions.place(s.id) != null
                MenuRow(if (inCloud) DesignR.drawable.ph_desktop else DesignR.drawable.ph_upload_simple, if (inCloud) "Bring back to this Mac" else "Teleport to cloud", testTag = "overviewTeleport") {
                    menu = false
                    move(s, !inCloud)
                }
            }
            MenuRow(R.drawable.shell_tray, "Archive") { menu = false; actions.onClose(s) }
        }
    }
    Box(
        Modifier.size(28.dp).clickable(role = Role.Button, onClickLabel = "Archive") { actions.onClose(s) }.semantics { contentDescription = "Archive ${s.title}" },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(R.drawable.shell_x), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(if (deck) 13.dp else 11.dp)) }
}

@Composable
private fun OverviewListRow(
    s: AgentSession,
    working: Boolean,
    selecting: Boolean,
    selectable: Boolean,
    ticked: Boolean,
    actions: CardActions,
    onTap: () -> Unit,
) {
    val c = AllternitTheme.colors
    Row(
        Modifier
            .fillMaxWidth()
            .testTag("overviewRow${s.id}")
            .clickable(role = Role.Button, onClick = onTap)
            .alpha(if (selecting && !selectable) 0.45f else 1f)
            .semantics {
                contentDescription = if (working) "${s.title}, Working" else if (actions.unread(s)) "${s.title}, New reply" else s.title
                if (selecting && selectable) stateDescription = if (ticked) "Selected" else "Not selected"
            }
            .heightIn(min = 60.dp)
            .padding(horizontal = 20.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        if (selecting) {
            Box(
                Modifier
                    .size(22.dp)
                    .alpha(if (selectable) 1f else 0f)
                    .clip(CircleShape)
                    .then(if (ticked) Modifier.background(c.accentPrimary) else Modifier.border(1.5.dp, c.textTertiary, CircleShape)),
                contentAlignment = Alignment.Center,
            ) { if (ticked) Icon(painterResource(DesignR.drawable.ph_check_bold), contentDescription = null, tint = c.onAccent, modifier = Modifier.size(12.dp)) }
        }
        Icon(painterResource(sessionIcon(s)), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                TitleOrField(s, actions, 16.5f, 1, Modifier.weight(1f))
                Text(relativeTime(s.timestampMillis), color = c.textSecondary, fontSize = 12.sp)
            }
            s.description?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            StatusPill(s, working, actions)
            PlaceBadge(s, actions)
        }
        if (actions.editable) Row { CardControls(s, actions, deck = false) }
    }
}

@Composable
private fun OverviewCard(s: AgentSession, tall: Boolean, working: Boolean, actions: CardActions, onTap: () -> Unit, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(16.dp)
    val count = (s.messageCount ?: 0L).toInt()
    Column(
        modifier
            .then(if (tall) Modifier.shadow(15.dp, shape, ambientColor = Color.Black.copy(alpha = 0.10f), spotColor = Color.Black.copy(alpha = 0.10f)) else Modifier)
            .then(if (working && !tall) Modifier.border(3.dp, Coral.copy(alpha = 0.14f), RoundedCornerShape(19.dp)).padding(1.5.dp) else Modifier)
            .clip(shape)
            .background(c.bgPanel)
            .border(1.dp, if (working) Coral else c.borderSubtle, shape)
            .testTag("overviewCard${s.id}")
            .clickable(role = Role.Button, onClick = onTap)
            .semantics { contentDescription = if (working) "${s.title}, Working" else if (actions.unread(s)) "${s.title}, New reply" else s.title }
            .then(if (tall) Modifier.fillMaxSize() else Modifier.fillMaxWidth().heightIn(min = 150.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(sessionIcon(s)), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(13.dp))
            Text(actions.groupLabel(s), color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium, maxLines = 1, modifier = Modifier.weight(1f))
            Text(relativeTime(s.timestampMillis), color = c.textSecondary, fontSize = 11.sp)
        }
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            TitleOrField(s, actions, if (tall) 15.5f else 15f, 2, Modifier.weight(1f))
            if (actions.editable) CardControls(s, actions, deck = tall)
        }
        StatusPill(s, working, actions)
        PlaceBadge(s, actions)
        s.description?.takeIf { it.isNotEmpty() }?.let {
            Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = if (tall) 12 else 4, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.weight(1f, fill = true))
        Text("$count message${if (count == 1) "" else "s"}", color = c.textTertiary, fontSize = 11.sp)
    }
}

/** A deck card (iOS OverviewDeck, 280 × 420): swipe up past 90dp to archive it. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DeckCard(s: AgentSession, working: Boolean, actions: CardActions, onTap: () -> Unit, onSwipeClose: () -> Unit) {
    var offset by remember { mutableFloatStateOf(0f) }
    val density = androidx.compose.ui.platform.LocalDensity.current
    val threshold = with(density) { 90.dp.toPx() }
    OverviewCard(
        s,
        tall = true,
        working = working,
        actions = actions,
        onTap = onTap,
        modifier = Modifier
            .width(280.dp)
            .height(420.dp)
            .offset { IntOffset(0, offset.roundToInt()) }
            .alpha((1f + offset / with(density) { 400.dp.toPx() }).coerceIn(0f, 1f))
            .then(
                if (actions.editable && actions.editingId == null) Modifier.pointerInput(s.id) {
                    detectVerticalDragGestures(
                        onDragEnd = {
                            if (offset < -threshold) onSwipeClose()
                            offset = 0f
                        },
                        onDragCancel = { offset = 0f },
                    ) { _, dy ->
                        val next = offset + dy
                        offset = if (next < 0f) next else next * 0.05f
                    }
                } else Modifier,
            ),
    )
}

/** The floating search field and New pill at the bottom (iOS PhoneFloatingBar). */
@Composable
private fun OverviewFloatingBar(query: String, placeholder: String, newLabel: String, onQuery: (String) -> Unit, onNew: () -> Unit, modifier: Modifier) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(
            Modifier
                .weight(1f)
                .height(44.dp)
                .shadow(8.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.10f), spotColor = Color.Black.copy(alpha = 0.10f))
                .clip(CircleShape)
                .background(c.bgPrimary)
                .border(1.dp, c.textPrimary.copy(alpha = 0.08f), CircleShape)
                .padding(horizontal = 14.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            Box(Modifier.weight(1f)) {
                if (query.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 15.sp, maxLines = 1)
                BasicTextField(
                    value = query,
                    onValueChange = onQuery,
                    singleLine = true,
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp, fontFamily = ShellSans),
                    cursorBrush = SolidColor(c.textPrimary),
                    modifier = Modifier.fillMaxWidth().testTag("overviewSearch"),
                )
            }
        }
        Row(
            Modifier
                .height(44.dp)
                .shadow(8.dp, RoundedCornerShape(22.dp), ambientColor = Color.Black.copy(alpha = 0.10f), spotColor = Color.Black.copy(alpha = 0.10f))
                .clip(RoundedCornerShape(22.dp))
                .background(c.bgPrimary)
                .border(1.dp, c.textPrimary.copy(alpha = 0.08f), RoundedCornerShape(22.dp))
                .testTag("overviewNew")
                .clickable(role = Role.Button) {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onNew()
                }
                .padding(horizontal = 16.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(painterResource(R.drawable.shell_plus), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
            Text(newLabel, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
        }
    }
}
