package com.allternit.feature.shell

import com.allternit.core.design.GizziCodeWordmark
import com.allternit.core.design.PixelWordmark
import com.allternit.core.design.MatrixLogo
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.GlassCircleButton
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.PolishHaptic
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.ui.graphics.graphicsLayer
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR

/**
 * The drawer (iOS HistorySidebarView, phone Arc drawer). Top line: the mode orb, the pixel
 * wordmark, Inbox and Search. Then the mode's rail rows and More, Recents with "View all ›" and
 * the filter, and a floating footer: the account avatar (Settings), the Phones button and the
 * mode's New pill.
 */
@Composable
internal fun ShellDrawer(
    mode: ShellMode,
    account: Account?,
    store: ShellSessionsStore,
    /** A mode picked in the drawer's mode orb (tap-only; Shelves is the composer orb's hold). */
    onPickMode: (ShellMode) -> Unit,
    /** Bots' waiting count on the mode orb (iOS drawer orb badge). */
    botsWaiting: Int = 0,
    onRow: (RailRow) -> Unit,
    onOpenSession: (AgentSession) -> Unit,
    onNew: () -> Unit,
    onSettings: () -> Unit,
    onRetry: () -> Unit,
    onSearch: () -> Unit,
    onViewAll: () -> Unit,
    onInbox: () -> Unit = {},
    inboxCount: Int = 0,
    newOptions: List<NewOption> = emptyList(),
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var hidden by remember(mode) { mutableStateOf(RailTabPrefs.hidden(context, mode)) }
    var status by remember { mutableStateOf(RecentsStatusFilter.All) }
    var date by remember { mutableStateOf(RecentsDateFilter.All) }
    val filtered = status != RecentsStatusFilter.All || date != RecentsDateFilter.All
    val now = System.currentTimeMillis()
    val recents = store.sessions
        .filter { it.belongs(mode) }
        .filter {
            when (status) {
                RecentsStatusFilter.All, RecentsStatusFilter.Active -> !it.archived
                RecentsStatusFilter.Completed, RecentsStatusFilter.Archived -> it.archived
            }
        }
        .filter { s -> date.days?.let { d -> now - s.timestampMillis <= d * 86_400_000L } ?: true }
    val rows = railRows(mode)
    Box(modifier.fillMaxSize().statusBarsPadding()) {
        Column(Modifier.fillMaxSize()) {
            DrawerHeader(mode = mode, onPickMode = onPickMode, botsWaiting = botsWaiting, onInbox = onInbox, inboxCount = inboxCount, onSearch = onSearch)
            LazyColumn(modifier = Modifier.fillMaxWidth().weight(1f)) {
                items(rows.filter { it.id !in hidden }, key = { "rail-${it.id}" }) { row -> DrawerNavRow(row.label, row.icon, "drawerRow${row.id}") { onRow(row) } }
                item(key = "rail-more") {
                    MoreRow(rows = rows, hidden = hidden, onToggle = { id -> hidden = RailTabPrefs.toggle(context, mode, id) }, onApps = {
                        onRow(RailRow("connectors", "Apps & Extensions", DesignR.drawable.ph_plugs_connected))
                    }, onMiniApps = {
                        onRow(RailRow("mini-apps", "Mini Apps", DesignR.drawable.ph_puzzle_piece))
                    })
                }
                item(key = "recents-label") {
                    RecentsLabel(
                        filtered = filtered,
                        status = status,
                        date = date,
                        onStatus = { status = it },
                        onDate = { date = it },
                        onViewAll = onViewAll,
                    )
                }
                when {
                    account == null -> item(key = "recents-signed-out") {
                        Text(
                            "Sign in to see your chats here.",
                            color = c.textSecondary,
                            fontSize = 14.sp,
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp),
                        )
                    }
                    store.isLoading && !store.hasLoaded -> item(key = "recents-loading") {
                        Box(Modifier.fillMaxWidth().padding(vertical = 24.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(
                                modifier = Modifier.size(22.dp).semantics { contentDescription = "Loading chats" },
                                strokeWidth = 2.dp,
                                color = c.textSecondary,
                            )
                        }
                    }
                    store.loadError != null && store.sessions.isEmpty() -> item(key = "recents-error") {
                        InlineError(
                            title = "Couldn't load conversations",
                            message = store.loadError.orEmpty(),
                            onRetry = onRetry,
                        )
                    }
                    recents.isEmpty() -> item(key = "recents-empty") {
                        if (filtered) EmptyStateDrawer(
                            title = "Nothing matches",
                            message = "Try another filter.",
                            art = com.allternit.core.design.EmptyArtKind.Search,
                            newLabel = mode.newLabel,
                            onNew = onNew,
                        ) else EmptyStateDrawer(
                            title = mode.emptyTitle,
                            message = mode.emptyMessage,
                            art = com.allternit.core.design.EmptyArtKind.Envelope,
                            newLabel = mode.newLabel,
                            onNew = onNew,
                        )
                    }
                    else -> items(recents, key = { "s-${it.id}" }) { session ->
                        RecentRow(session) { onOpenSession(session) }
                    }
                }
                // Room for the floating footer.
                item(key = "foot-space") { Spacer(Modifier.height(92.dp)) }
            }
        }
        DrawerFooter(
            mode = mode,
            account = account,
            onSettings = onSettings,
            onPhones = { onRow(RailRow("phones", "Phones", com.allternit.core.design.R.drawable.ph_device_mobile)) },
            onMeetings = { onRow(RailRow("meetings", "Meetings", com.allternit.core.design.R.drawable.ph_video_camera)) },
            onNew = onNew,
            newOptions = newOptions,
            modifier = Modifier.align(Alignment.BottomCenter),
        )
    }
}

/** `phone-drawer-top-modes`: the mode dial, the wordmark, Inbox and Search (iOS header). */
@Composable
private fun DrawerHeader(mode: ShellMode, onPickMode: (ShellMode) -> Unit, botsWaiting: Int, onInbox: () -> Unit, inboxCount: Int, onSearch: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 14.dp, top = 6.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        // The regular four-mode switcher; Shelves opens only from the composer (or ACI) orb's hold.
        DrawerModeOrb(mode = mode, botsWaiting = botsWaiting, onPickMode = onPickMode)
        // ShellWordmark: A://TERNIT on Home; Bots and ACI show their word alone (Eoj 2026-11-24).
        Box(
            modifier = Modifier.padding(start = 6.dp).weight(1f).heightIn(min = 44.dp).testTag("shellWordmark${mode.raw}"),
            contentAlignment = Alignment.CenterStart,
        ) {
            when {
                mode == ShellMode.Code -> GizziCodeWordmark(height = 15.dp)
                mode.wordmarkSuffix.isNotEmpty() -> PixelWordmark(height = 16.dp, suffix = mode.wordmarkSuffix, wordOnly = true)
                else -> PixelWordmark(height = 16.dp)
            }
        }
        Box {
            RoundButton(com.allternit.core.design.R.drawable.ph_tray, if (inboxCount > 0) "Needs you, $inboxCount waiting" else "Needs you", "drawerInbox", onInbox)
            if (inboxCount > 0) {
                // The waiting count (iOS inbox badge on the drawer's Inbox button).
                Text(
                    if (inboxCount > 99) "99+" else "$inboxCount",
                    color = Color.White,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .clip(CircleShape)
                        .background(AllternitStatic.AccentChat)
                        .padding(horizontal = 5.dp, vertical = 1.dp)
                        .testTag("drawerInboxCount"),
                )
            }
        }
        RoundButton(DesignR.drawable.ph_magnifying_glass, "Search chats, projects and bots", "drawerSearch", onSearch)
    }
}

/** 40dp glass circle (`phone-round-btn`): Inbox and Search. */
@Composable
private fun RoundButton(icon: Int, label: String, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    GlassCircleButton(onClick = onClick, contentDescription = label, modifier = Modifier.testTag(testTag)) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
    }
}

/** A row's press highlight: the neutral fill while a finger is down, with the .98 row press scale. */
@Composable
private fun Modifier.rowPress(source: MutableInteractionSource): Modifier {
    val pressed by source.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.98f else 1f, spring(dampingRatio = 0.7f, stiffness = 600f), label = "rowPress")
    val fill = AllternitTheme.colors.fillNeutral
    return this.graphicsLayer { scaleX = scale; scaleY = scale }.background(if (pressed) fill else Color.Transparent)
}

/** `phone-nav-row`: 50dp, 24dp glyph column, 17sp label. */
@Composable
private fun DrawerNavRow(label: String, icon: Int, testTag: String, dot: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val source = remember { MutableInteractionSource() }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(AllternitRadii.MD))
            .testTag(testTag)
            .rowPress(source)
            .clickable(interactionSource = source, indication = null, role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onClick()
            }
            .height(50.dp)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
            Icon(painter = painterResource(icon), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(22.dp))
            if (dot) Box(Modifier.align(Alignment.TopEnd).size(6.dp).clip(CircleShape).background(c.textPrimary))
        }
        Text(label, color = c.textPrimary, fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** More: choose which rows show (shared with desktop), and Apps & Extensions (iOS moreRow). */
@Composable
private fun MoreRow(rows: List<RailRow>, hidden: Set<String>, onToggle: (String) -> Unit, onApps: () -> Unit, onMiniApps: () -> Unit) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Box {
        DrawerNavRow("More", com.allternit.core.design.R.drawable.ph_dots_three, "drawerRowmore", dot = rows.any { it.id in hidden }) { open = true }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
            Text("Show in rail", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
            rows.forEach { row ->
                MenuRow(
                    icon = row.icon,
                    title = if (row.id in hidden) "${row.label} · Hidden" else row.label,
                    checked = row.id !in hidden,
                    testTag = "railTab${row.id}",
                ) { onToggle(row.id) }
            }
            MenuRow(icon = DesignR.drawable.ph_plugs_connected, title = "Apps & Extensions", testTag = "railAppsExtensions") {
                open = false
                onApps()
            }
            MenuRow(icon = DesignR.drawable.ph_puzzle_piece, title = "Mini Apps", testTag = "railMiniApps") {
                open = false
                onMiniApps()
            }
        }
    }
}

/** Recents · View all › · filter (iOS recentsLabel). */
@Composable
private fun RecentsLabel(
    filtered: Boolean,
    status: RecentsStatusFilter,
    date: RecentsDateFilter,
    onStatus: (RecentsStatusFilter) -> Unit,
    onDate: (RecentsDateFilter) -> Unit,
    onViewAll: () -> Unit,
) {
    val c = AllternitTheme.colors
    var menu by remember { mutableStateOf(false) }
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = 34.dp).padding(start = 20.dp, end = 14.dp, top = 12.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text("Recents", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f).semantics { heading() })
        Row(
            modifier = Modifier
                .heightIn(min = 32.dp)
                .clip(RoundedCornerShape(8.dp))
                .testTag("recentsSeeAll")
                .clickable(role = Role.Button, onClickLabel = "Shows all your chats", onClick = onViewAll)
                .padding(horizontal = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            Text("View all", color = c.textSecondary, fontSize = 14.sp)
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(11.dp))
        }
        Box {
            Box(
                modifier = Modifier
                    .size(36.dp)
                    .clip(CircleShape)
                    .testTag("recentsFilter")
                    .clickable(role = Role.Button, onClickLabel = if (filtered) "Filter recents (filtered)" else "Filter recents") { menu = true }
                    .semantics { contentDescription = if (filtered) "Filter recents (filtered)" else "Filter recents" },
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(DesignR.drawable.ph_sliders_horizontal), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(17.dp))
                if (filtered) Box(Modifier.align(Alignment.TopEnd).padding(top = 6.dp, end = 6.dp).size(6.dp).clip(CircleShape).background(c.textPrimary))
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
                Text("Status", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                RecentsStatusFilter.entries.forEach { f -> MenuRow(icon = null, title = f.label, checked = f == status) { onStatus(f) } }
                Text("Last activity", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                RecentsDateFilter.entries.forEach { f -> MenuRow(icon = null, title = f.label, checked = f == date) { onDate(f) } }
            }
        }
    }
}

/** iOS FriendlyInlineStateView (offline): amber tile, title and line, then a small Retry pill. */
@Composable
private fun InlineError(title: String, message: String, onRetry: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 16.dp).testTag("drawerRecentsError")) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
            Box(
                Modifier.size(36.dp).clip(RoundedCornerShape(10.dp)).background(AllternitStatic.StatusWarning.copy(alpha = 0.16f)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_wifi_slash), contentDescription = null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(18.dp))
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text(message, color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp)
            }
        }
        Text(
            "Retry",
            color = c.textPrimary,
            fontSize = 14.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier
                .padding(top = 10.dp)
                .shadow(3.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.08f), spotColor = Color.Black.copy(alpha = 0.08f))
                .clip(CircleShape)
                .background(c.bgPrimary)
                .border(1.dp, c.borderSubtle, CircleShape)
                .testTag("drawerRecentsRetry")
                .clickable(role = Role.Button, onClick = onRetry)
                .padding(horizontal = 16.dp, vertical = 8.dp),
        )
    }
}

/** The Arc empty state: icon tile, title, line, and the mode's New button (iOS emptyState). */
@Composable
private fun EmptyStateDrawer(title: String, message: String, newLabel: String, onNew: () -> Unit, art: com.allternit.core.design.EmptyArtKind) {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        com.allternit.core.design.EmptyArt(art)
        Text(title, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 12.dp))
        Text(message, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(top = 8.dp))
        Row(
            modifier = Modifier
                .padding(top = 20.dp)
                .clip(RoundedCornerShape(8.dp))
                .background(c.accentPrimary)
                .testTag("drawerEmptyNew")
                .clickable(role = Role.Button, onClick = onNew)
                .padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_note_pencil), contentDescription = null, tint = c.onAccent, modifier = Modifier.size(13.dp))
            Text(newLabel, color = c.onAccent, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        }
    }
}

@Composable
internal fun RecentRow(session: AgentSession, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    com.allternit.core.design.QuietSessionRow(
        title = session.title,
        time = relativeTime(session.timestampMillis),
        testTag = "recentRow${session.id}",
        onClick = onClick,
        leading = { com.allternit.core.design.QuietSessionGlyph(if (session.isTerminal || session.originSurface == "code") sessionIcon(session) else com.allternit.core.design.R.drawable.ph_chat_circle) },
        status = {
            if (session.isTerminal) {
                Text(
                    text = "TERMINAL",
                    color = c.textSecondary,
                    fontSize = 10.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .clip(RoundedCornerShape(4.dp))
                        .background(c.borderStrong.copy(alpha = c.borderStrong.alpha * 0.5f))
                        .padding(horizontal = 5.dp, vertical = 1.dp),
                )
            }
        },
    )
}

/**
 * `phone-rail-foot` (iOS footer): the account avatar in its ring (Settings), the Phones button
 * (the pixel A in ink, iOS PhonesDrawerButton) and the mode's floating New pill.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DrawerFooter(
    mode: ShellMode,
    account: Account?,
    onSettings: () -> Unit,
    onPhones: () -> Unit,
    /** Wave H2: iOS MeetingsDrawerButton, shown with the account's `meetings` flag. */
    onMeetings: () -> Unit,
    onNew: () -> Unit,
    newOptions: List<NewOption>,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var newMenu by remember { mutableStateOf(false) }
    // The New pill is a glass pill (press scale .94) that keeps its press-and-hold menu.
    val pillSource = remember { MutableInteractionSource() }
    val pillPressed by pillSource.collectIsPressedAsState()
    val pillScale by animateFloatAsState(if (pillPressed) 0.94f else 1f, spring(dampingRatio = 0.49f, stiffness = 600f), label = "newPillPress")
    val name = account?.displayName ?: "Not signed in"
    Row(
        modifier = modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(0f to c.bgPrimary.copy(alpha = 0f), 0.42f to c.bgPrimary))
            .navigationBarsPadding()
            .padding(start = 14.dp, end = 14.dp, top = 28.dp, bottom = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        GlassCircleButton(
            onClick = onSettings,
            contentDescription = "$name. Settings",
            modifier = Modifier.testTag("drawerSettings"),
            size = 44.dp,
        ) {
            AccountAvatar(account, size = 30)
        }
        Box(
            modifier = Modifier
                .size(40.dp)
                .clip(CircleShape)
                .background(c.fillNeutral)
                .testTag("drawerPhones")
                .clickable(role = Role.Button, onClickLabel = "Phones") {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    onPhones()
                }
                .semantics { contentDescription = if (com.allternit.feature.phones.PhonesUnread.total > 0) "Phones, ${com.allternit.feature.phones.PhonesUnread.total} unread" else "Phones" },
            contentAlignment = Alignment.Center,
        ) {
            MatrixLogo(size = 22.dp, ink = c.textPrimary)
            val unread = com.allternit.feature.phones.PhonesUnread.total
            if (unread > 0) {
                Text(
                    if (unread > 99) "99+" else "$unread",
                    color = androidx.compose.ui.graphics.Color.White,
                    fontSize = 10.sp,
                    fontWeight = androidx.compose.ui.text.font.FontWeight.Bold,
                    modifier = Modifier
                        .align(Alignment.TopEnd)
                        .offset(x = 4.dp, y = (-4).dp)
                        .clip(CircleShape)
                        .background(com.allternit.core.design.AllternitStatic.StatusError)
                        .padding(horizontal = 4.dp)
                        .defaultMinSize(minWidth = 16.dp, minHeight = 16.dp)
                        .testTag("drawerPhonesBadge"),
                )
            }
        }
        androidx.compose.runtime.LaunchedEffect(account) {
            if (account == null) { com.allternit.feature.phones.PhonesUnread.clear(); return@LaunchedEffect }
            while (true) { com.allternit.feature.phones.PhonesUnread.refresh(); kotlinx.coroutines.delay(60_000) }
        }
        val meetingsContext = androidx.compose.ui.platform.LocalContext.current
        androidx.compose.runtime.LaunchedEffect(account) { com.allternit.feature.meetings.MeetingsStore.refreshFlag(meetingsContext, signedIn = account != null) }
        // iOS keeps the agenda watched app-wide: banners post with the Meetings screen closed.
        androidx.compose.runtime.LaunchedEffect(account) { com.allternit.feature.meetings.MeetingsStore.watch(meetingsContext, signedIn = account != null) }
        if (com.allternit.feature.meetings.MeetingsStore.isEnabled) {
            val inCall = com.allternit.feature.meetings.MeetingsStore.live.isNotEmpty()
            Box(
                modifier = Modifier
                    .size(40.dp)
                    .clip(CircleShape)
                    .background(c.fillNeutral)
                    .testTag("drawerMeetings")
                    .clickable(role = Role.Button, onClickLabel = "Meetings") {
                        Haptics.perform(view, Haptics.Kind.Tap)
                        onMeetings()
                    }
                    .semantics { contentDescription = if (inCall) "Meetings, Gizzi is in a call" else "Meetings" },
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_video_camera), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
                if (inCall) Box(Modifier.align(Alignment.TopEnd).padding(6.dp).size(8.dp).clip(CircleShape).background(Color(0xFF16A34A)))
            }
        }
        Spacer(Modifier.weight(1f))
        Box {
        Row(
            modifier = Modifier
                .graphicsLayer { scaleX = pillScale; scaleY = pillScale }
                .shadow(6.dp, CircleShape, ambientColor = Color.Black.copy(alpha = 0.08f), spotColor = Color.Black.copy(alpha = 0.12f))
                .clip(CircleShape)
                .background(PolishGlass.fill())
                .border(1.dp, PolishGlass.border(), CircleShape)
                .testTag("drawerNewPill")
                // Tap starts the mode's new item; press and hold opens more ways to start (iOS newPill Menu).
                .combinedClickable(
                    interactionSource = pillSource,
                    indication = null,
                    role = Role.Button,
                    onLongClickLabel = "More ways to start",
                    onLongClick = if (newOptions.isEmpty()) null else ({
                        Haptics.perform(view, Haptics.Kind.Tap)
                        newMenu = true
                    }),
                ) {
                    PolishHaptic.perform(view, PolishHaptic.Kind.Light)
                    onNew()
                }
                .semantics { contentDescription = "${mode.newLabel}. Press and hold for more ways to start" }
                .height(44.dp)
                .padding(horizontal = 18.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_plus), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(18.dp))
            Text(mode.newLabel, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium)
        }
        DropdownMenu(
            expanded = newMenu,
            onDismissRequest = { newMenu = false },
            shape = RoundedCornerShape(16.dp),
            containerColor = c.bgPrimary,
        ) {
            newOptions.forEach { option ->
                MenuRow(option.icon, option.label, testTag = "drawerNew${option.id}") {
                    newMenu = false
                    option.action()
                }
            }
        }
        }
    }
}

/** A press-and-hold way to start from the New pill (iOS HistorySidebarView NewOption). */
internal data class NewOption(val id: String, val label: String, val icon: Int, val action: () -> Unit)

/** The account initial in a neutral circle (iOS avatarInitial on BorderStrong). */
@Composable
internal fun AccountAvatar(account: Account?, size: Int) {
    val c = AllternitTheme.colors
    val initial = account?.displayName?.firstOrNull()?.uppercaseChar()?.toString() ?: "?"
    Box(
        modifier = Modifier.size(size.dp).clip(CircleShape).background(c.borderStrong),
        contentAlignment = Alignment.Center,
    ) {
        Text(initial, color = c.textPrimary, fontSize = (size * 0.43f).sp, fontWeight = FontWeight.SemiBold)
    }
}

/** Width helper so the shell and the drawer agree: min(80% of the screen, 360dp), iOS WorkspaceShell `sidebarWidth`. */
internal fun drawerWidth(screenWidth: androidx.compose.ui.unit.Dp): androidx.compose.ui.unit.Dp =
    minOf(screenWidth * 0.80f, 360.dp)
