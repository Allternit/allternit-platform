package com.allternit.feature.shell

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.AnchoredDraggableDefaults
import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.DraggableAnchors
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.anchoredDraggable
import androidx.compose.foundation.gestures.animateTo
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.auth.Account
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.Motion
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.chat.ChatViewModel
import com.allternit.feature.chat.SessionContext
import com.allternit.feature.artifacts.ArtifactDetailsScreen
import com.allternit.feature.artifacts.ArtifactsLibraryScreen
import com.allternit.feature.projects.ProjectsListScreen
import com.allternit.feature.projects.ProjectsNav
import com.allternit.feature.shell.backdrop.BackdropScope
import com.allternit.feature.shell.backdrop.BackdropSurface
import com.allternit.feature.shell.backdrop.StageBackdrop
import com.allternit.feature.shell.backdrop.StageBackdropStore
import kotlinx.coroutines.launch

internal enum class DrawerValue { Closed, Open }

/** What fills the page under the drawer. Chat, sections and settings are stubs until later steps. */
private sealed interface ShellPage {
    data object Home : ShellPage
    data class Chat(val session: AgentSession?, val draft: ComposerDraft) : ShellPage
    data class Section(val row: RailRow) : ShellPage
    data object Settings : ShellPage
    data object AllChats : ShellPage
    data object Search : ShellPage
    /** Wave D: a chat card's artifact (iOS ArtifactDetailsView sheet from ChatView). */
    data class Artifact(val record: com.allternit.feature.artifacts.ArtifactRecordInput, val back: ShellPage) : ShellPage
    /** Wave D: Projects opened on one project (search, a link). */
    data class Project(val projectId: String) : ShellPage
}

/**
 * The signed-in shell (iOS MainWorkspaceView, PWA PhoneShell): one page fills the screen, no
 * bottom bar. The drawer is min(80%, 360dp) wide (iOS sidebarWidth), springs in from the left (☰ tap or a pan from
 * the left edge), and the page pushes right with it, scaled to 0.94 with rounded corners. Holding
 * ☰ fans the modes out over everything; the fan and the drawer are never open together.
 *
 * [account] is null for the debug "Continue without signing in" path: no API calls then.
 * [openSettingsOnStart] opens Settings first (a first-run checklist fix).
 */
@Composable
internal fun WorkspaceShell(account: Account?, openSettingsOnStart: Boolean = false, launch: ShellLaunch = ShellLaunch()) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val density = LocalDensity.current
    val scope = rememberCoroutineScope()

    var mode by remember { mutableStateOf(launch.mode ?: ShellModePrefs.load(context)) }
    // A first-run checklist fix opens Settings once the workspace is up (iOS DeepLinkRouter);
    // a debug `open-tab` extra opens a section (iOS AppModeStore `-open-tab`).
    var page by remember {
        mutableStateOf(
            when {
                openSettingsOnStart || launch.tab == "settings" -> ShellPage.Settings
                launch.tab == "search" -> ShellPage.Search
                launch.tab == "chats" || launch.tab == "overview" -> ShellPage.AllChats
                launch.tab != null -> launchSection(launch.tab)?.let { ShellPage.Section(it) } ?: ShellPage.Home
                else -> ShellPage.Home
            },
        )
    }
    val store = remember {
        ShellSessionsStore().also { it.seenPrefs = context.getSharedPreferences("allternit_shell", android.content.Context.MODE_PRIVATE) }
    }
    val models = remember { ModelStore(context) }
    // One chat for the shell (iOS keeps one ChatViewModel): a running reply survives a trip
    // to the home and back. It shares the shell's OkHttp client and API.
    val chatBackend = remember { ChatBackends.create(context, ShellApi.okHttp, ShellApi.api) }
    val chat = remember { ChatViewModel(chatBackend, context.contentResolver) }
    DisposableEffect(chat) { onDispose { chat.dispose() } }
    val hold = remember { MenuHoldState() }
    val drawer = remember { AnchoredDraggableState(if (launch.sidebar) DrawerValue.Open else DrawerValue.Closed) }
    // Wave C sheets: Inbox (drawer tray), Quick task and Continue CLI session (New pill hold).
    val inbox = remember { InboxStore(chatBackend) }
    val quickTasks = remember { QuickTaskRunner(chatBackend, context.contentResolver) }
    DisposableEffect(quickTasks) { onDispose { quickTasks.dispose() } }
    var inboxOpen by remember { mutableStateOf(launch.sheet == "inbox") }
    var quickTaskOpen by remember { mutableStateOf(launch.sheet == "quick-task") }
    var cliPickerSurface by remember { mutableStateOf(if (launch.sheet == "continue-cli") (if (mode == ShellMode.Code) "code" else "chat") else null) }

    fun reload() {
        if (account != null) scope.launch { store.load() }
    }
    // A new or deleted session shows in Recents right away.
    chat.onSessionsChanged = ::reload
    quickTasks.onSessionsChanged = ::reload
    chat.sessionContext = SessionContext(
        originSurface = when (mode) {
            ShellMode.Code -> "code"
            ShellMode.Aci -> "browser"
            else -> "chat"
        },
    )

    fun openSession(session: AgentSession?) {
        if (session == null) chat.startNewSession() else chat.loadSession(session.id)
        page = ShellPage.Chat(session = session, draft = ComposerDraft())
    }
    LaunchedEffect(account?.userId) {
        if (account != null) {
            store.load()
            // The drawer's Inbox count (iOS HistorySidebarView task: inbox.refresh()).
            inbox.refresh()
        }
    }


    fun setDrawer(open: Boolean) {
        scope.launch { drawer.animateTo(if (open) DrawerValue.Open else DrawerValue.Closed, Motion.standardSpring()) }
    }

    // One light haptic per open (any way of opening), the fan closes, and Recents refresh
    // each time the drawer is revealed (iOS onChange(isSidebarOpen)).
    LaunchedEffect(drawer.settledValue) {
        if (drawer.settledValue == DrawerValue.Open) {
            Haptics.perform(view, Haptics.Kind.Tap)
            hold.closeFan()
            if (account != null) store.load()
        }
    }

    fun pickMode(m: ShellMode) {
        hold.closeFan()
        Haptics.perform(view, Haptics.Kind.Tap)
        if (m != mode) {
            mode = m
            ShellModePrefs.save(context, m)
        }
        page = ShellPage.Home
    }

    fun openPage(p: ShellPage) {
        page = p
        setDrawer(false)
    }

    /** A session from search, the overview, a quick task or a CLI pick-up: open it in its mode. */
    fun openInItsMode(session: AgentSession) {
        val owner = ShellMode.entries.firstOrNull { session.belongs(it) && it != ShellMode.Bots }
        if (owner != null && owner != mode && session.agentId == null) {
            mode = owner
            ShellModePrefs.save(context, owner)
        }
        openSession(session)
    }

    /** Opens a session by id once the list has it (a quick task, a picked-up CLI session). */
    fun openSessionId(id: String) {
        scope.launch {
            if (account != null) store.load()
            val s = store.sessions.firstOrNull { it.id == id } ?: AgentSession(id = id)
            openInItsMode(s)
            setDrawer(false)
        }
    }

    // Wave D: what the Projects and Artifacts screens need from the shell.
    val shellMenuButton: @Composable () -> Unit = {
        PhoneMenuButton(mode = mode, hold = hold, holdEnabled = !drawerOpenNow(drawer), onOpen = { setDrawer(true) }, onPick = ::pickMode)
    }
    val projectsNav = remember {
        ProjectsNav(
            openSession = { id, sessionMode ->
                if (sessionMode == "code" && mode != ShellMode.Code) {
                    mode = ShellMode.Code
                    ShellModePrefs.save(context, ShellMode.Code)
                }
                openSessionId(id)
            },
            backdrop = { scopeId -> ProjectBackdrop(scopeId) },
        )
    }
    // The studio's Chat pane: the shell's one chat, on the studio's session (iOS ArtifactStudioView).
    val studioChat: @Composable (String) -> Unit = { sid ->
        LaunchedEffect(sid) { chat.loadSession(sid) }
        ChatPage(
            chat = chat,
            backend = chatBackend,
            store = store,
            models = models,
            menuButton = {},
            onNewChat = { chat.startNewSession(); page = ShellPage.Home },
            onDeleted = { page = ShellPage.Home },
            onOpenSection = { row -> page = ShellPage.Section(row) },
            onOpenSettings = { page = ShellPage.Settings },
        )
    }

    /** iOS newOptions: press and hold the New pill. */
    fun newOptions(): List<NewOption> {
        val quick = NewOption("quick-task", "Quick task", R.drawable.shell_bolt) { quickTaskOpen = true }
        val scheduled = NewOption("scheduled", "New scheduled task", R.drawable.shell_clock) {
            openPage(ShellPage.Section(RailRow(if (mode == ShellMode.Code) "code-automations" else "automation-tasks", "Automation Tasks", R.drawable.shell_clock)))
        }
        fun continueCli(surface: String) = NewOption("continue-cli", "Continue CLI session", DesignR.drawable.ph_terminal) { cliPickerSurface = surface }
        return when (mode) {
            ShellMode.Home -> listOf(continueCli("chat"), scheduled, quick)
            ShellMode.Code -> listOf(continueCli("code"), scheduled, quick)
            ShellMode.Aci, ShellMode.Bots -> listOf(quick)
        }
    }

    // The overview's Working / New reply (iOS ChatView → ShellSessionsStore.setActive).
    val chatOnScreen = page is ShellPage.Chat
    LaunchedEffect(chatOnScreen, chat.currentSessionId, chat.isStreaming) {
        store.setActive(if (chatOnScreen) chat.currentSessionId else null, chat.isStreaming)
        if (!chatOnScreen) store.setStreaming(chat.currentSessionId, chat.isStreaming)
    }

    val drawerOpen = drawer.settledValue == DrawerValue.Open || drawer.targetValue == DrawerValue.Open
    BackHandler(enabled = hold.fanOpen || drawerOpen || page != ShellPage.Home) {
        when {
            hold.fanOpen -> hold.closeFan()
            drawerOpen -> setDrawer(false)
            else -> page = ShellPage.Home
        }
    }

    // Every Text in the shell uses the bundled sans (iOS SF Pro sizes); the headline sets Serif itself.
    CompositionLocalProvider(LocalTextStyle provides LocalTextStyle.current.copy(fontFamily = ShellSans)) {
    BoxWithConstraints(Modifier.fillMaxSize().background(c.bgPrimary)) {
        val widthDp = drawerWidth(maxWidth)
        val widthPx = with(density) { widthDp.toPx() }
        val gapPx = with(density) { 24.dp.toPx() }
        LaunchedEffect(widthPx) {
            drawer.updateAnchors(
                DraggableAnchors {
                    DrawerValue.Closed at 0f
                    DrawerValue.Open at widthPx
                },
                drawer.targetValue,
            )
        }
        val fling = AnchoredDraggableDefaults.flingBehavior(drawer, { distance: Float -> distance * 0.25f }, Motion.standardSpring())
        val offset = drawer.offset.let { if (it.isNaN()) 0f else it }
        val p = if (widthPx > 0f) (offset / widthPx).coerceIn(0f, 1f) else 0f

        Box(
            Modifier
                .fillMaxSize()
                // Open: drag anywhere (page strip or drawer) to close it.
                .anchoredDraggable(
                    state = drawer,
                    orientation = Orientation.Horizontal,
                    enabled = drawer.settledValue == DrawerValue.Open,
                    flingBehavior = fling,
                ),
        ) {
            // The page: pushed right, scaled from its left edge, corners rounding as it moves.
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer {
                        translationX = p * widthPx
                        val s = 1f - 0.06f * p
                        scaleX = s
                        scaleY = s
                        transformOrigin = TransformOrigin(0f, 0.5f)
                        shape = if (p > 0f) RoundedCornerShape((36 * p).dp) else RectangleShape
                        clip = p > 0f
                    }
                    .background(c.bgPrimary),
            ) {
                Box(Modifier.fillMaxSize().then(if (drawerOpen) Modifier.clearAndSetSemantics {} else Modifier)) {
                    when (val current = page) {
                        ShellPage.Home -> ShellHome(
                            mode = mode,
                            store = store,
                            models = models,
                            menuButton = {
                                PhoneMenuButton(
                                    mode = mode,
                                    hold = hold,
                                    holdEnabled = !drawerOpen,
                                    onOpen = { setDrawer(true) },
                                    onPick = ::pickMode,
                                )
                            },
                            onNewChat = { draft ->
                                // The home composer's send starts a new chat and streams it.
                                chat.startNewSession()
                                chat.send(draft.toChatRequest())
                                page = ShellPage.Chat(session = null, draft = draft)
                            },
                            onNew = { page = ShellPage.Home },
                            onOpenSection = { row -> page = ShellPage.Section(row) },
                        )
                        is ShellPage.Chat -> ChatPage(
                            chat = chat,
                            backend = chatBackend,
                            store = store,
                            models = models,
                            menuButton = {
                                PhoneMenuButton(
                                    mode = mode,
                                    hold = hold,
                                    holdEnabled = !drawerOpen,
                                    onOpen = { setDrawer(true) },
                                    onPick = ::pickMode,
                                )
                            },
                            // New chat: the home with an empty composer (iOS startNewChat).
                            onNewChat = {
                                chat.startNewSession()
                                page = ShellPage.Home
                            },
                            onDeleted = { page = ShellPage.Home },
                            onOpenSection = { row -> page = ShellPage.Section(row) },
                            onOpenSettings = { page = ShellPage.Settings },
                            onOpenArtifact = { a ->
                                page = ShellPage.Artifact(com.allternit.feature.artifacts.ArtifactRecordInput(a.id, a.kind, a.title, a.url, a.content), back = current)
                            },
                        )
                        is ShellPage.Section -> when (current.row.id) {
                            // Wave D: the Projects and Artifacts sections (iOS WorkspaceShell .projects / .artifacts).
                            "projects" -> ProjectsListScreen(
                                signedIn = account != null,
                                leading = shellMenuButton,
                                nav = projectsNav,
                            )
                            "artifacts-library" -> ArtifactsLibraryScreen(
                                signedIn = account != null,
                                leading = shellMenuButton,
                                studioChat = studioChat,
                                onOpenChat = ::openSessionId,
                                onConnectors = { page = ShellPage.Section(RailRow("connectors", "Apps & Extensions", DesignR.drawable.ph_plugs_connected)) },
                                onSignIn = { page = ShellPage.Settings },
                            )
                            else -> SectionStub(current.row, onBack = { page = ShellPage.Home })
                        }
                        is ShellPage.Project -> ProjectsListScreen(
                            signedIn = account != null,
                            leading = shellMenuButton,
                            nav = projectsNav,
                            initialProjectId = current.projectId,
                        )
                        is ShellPage.Artifact -> ArtifactDetailsScreen(
                            record = current.record,
                            onClose = { page = current.back },
                            onOpenChat = ::openSessionId,
                            onSignIn = { page = ShellPage.Settings },
                        )
                        ShellPage.Settings -> SettingsScreen(account, onBack = { page = ShellPage.Home })
                        ShellPage.AllChats -> SessionOverviewScreen(
                            mode = mode,
                            store = store,
                            isWorking = { id ->
                                (chat.isStreaming && chat.currentSessionId == id) ||
                                    quickTasks.runs.any { it.sessionId == id && it.status == QuickTaskRunner.Status.Running }
                            },
                            onBack = { page = ShellPage.Home },
                            onOpen = { openInItsMode(it) },
                            onNew = {
                                chat.startNewSession()
                                page = ShellPage.Home
                            },
                        )
                        ShellPage.Search -> ShellSearchScreen(
                            store = store,
                            signedIn = account != null,
                            onBack = { page = ShellPage.Home },
                            onOpenSession = { openInItsMode(it) },
                            onOpenProject = { entry -> page = ShellPage.Project(entry.id) },
                            onOpenBot = { page = ShellPage.Section(RailRow("bots", "Your bots", DesignR.drawable.ph_cpu)) },
                        )
                    }
                }
                if (p > 0f) {
                    // The visible page strip: tap closes the drawer.
                    Box(
                        Modifier
                            .fillMaxSize()
                            .background(Color.Black.copy(alpha = 0.16f * p))
                            .testTag("drawerScrim")
                            .clickable(
                                interactionSource = remember { MutableInteractionSource() },
                                indication = null,
                                onClickLabel = "Close menu",
                            ) { setDrawer(false) }
                            .semantics { contentDescription = "Close menu" },
                    )
                }
            }

            // Left-edge pan opens the drawer. It starts under the header so ☰ keeps its taps,
            // and sits outside the moving page so drag deltas are not measured in a moving frame.
            if (drawer.settledValue == DrawerValue.Closed && !hold.fanOpen) {
                val top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 52.dp
                Box(
                    Modifier
                        .padding(top = top)
                        .width(24.dp)
                        .fillMaxHeight()
                        .testTag("drawerEdge")
                        .anchoredDraggable(
                            state = drawer,
                            orientation = Orientation.Horizontal,
                            enabled = true,
                            flingBehavior = fling,
                        ),
                )
            }

            // The drawer panel: rounded trailing corners, slides in from the edge.
            Box(
                Modifier
                    .width(widthDp)
                    .fillMaxHeight()
                    .graphicsLayer {
                        translationX = (p - 1f) * (widthPx + gapPx)
                        shadowElevation = if (p > 0f) 16.dp.toPx() else 0f
                        shape = RoundedCornerShape(topEnd = 26.dp, bottomEnd = 26.dp)
                        clip = true
                    }
                    .background(c.bgPrimary)
                    .testTag("shellDrawer")
                    .then(if (p == 0f) Modifier.clearAndSetSemantics {} else Modifier.semantics { contentDescription = "${mode.label} menu" }),
            ) {
                ShellDrawer(
                    mode = mode,
                    account = account,
                    store = store,
                    onModeDial = {
                        // D4: close the drawer, then fan the modes out of ☰.
                        scope.launch {
                            drawer.animateTo(DrawerValue.Closed, Motion.standardSpring())
                            hold.openFan()
                        }
                    },
                    onRow = { row -> openPage(ShellPage.Section(row)) },
                    onOpenSession = {
                        openSession(it)
                        setDrawer(false)
                    },
                    // New chat: the home with an empty composer (iOS newItem on Home).
                    onNew = {
                        chat.startNewSession()
                        openPage(ShellPage.Home)
                    },
                    onSettings = { openPage(ShellPage.Settings) },
                    onRetry = ::reload,
                    onSearch = { openPage(ShellPage.Search) },
                    onViewAll = { openPage(ShellPage.AllChats) },
                    onInbox = { inboxOpen = true },
                    inboxCount = inbox.items.size,
                    newOptions = newOptions(),
                )
            }
        }

        ModeFanOverlay(hold = hold, current = mode, onPick = ::pickMode)

        if (inboxOpen) {
            InboxSheet(
                store = inbox,
                focusedId = launch.inboxItem,
                onOpenBot = { openPage(ShellPage.Section(RailRow("bots", "Your bots", DesignR.drawable.ph_cpu))) },
                onOpenSettings = { openPage(ShellPage.Settings) },
                onDismiss = { inboxOpen = false },
            )
        }
        if (quickTaskOpen) {
            QuickTaskSheet(
                runner = quickTasks,
                models = models,
                onOpenChat = ::openSessionId,
                onDismiss = { quickTaskOpen = false },
            )
        }
        cliPickerSurface?.let { surface ->
            NativeSessionPickerSheet(
                surface = surface,
                signedIn = account != null,
                onPicked = ::openSessionId,
                onDismiss = { cliPickerSurface = null },
            )
        }
    }
    }
}

/**
 * Debug launch extras for screenshots and Maestro (iOS `-open-tab`, `-sidebar`, `-shell-mode`,
 * `-open-inbox` …), parsed by MainActivity in debug builds only. Release always passes the default.
 */
internal data class ShellLaunch(
    /** A section or page: a rail id (`projects`, `artifacts-library`, …), `settings`, `search`, `chats`. */
    val tab: String? = null,
    /** Start with the drawer open (iOS `-sidebar`). */
    val sidebar: Boolean = false,
    /** Start in this mode (iOS `-shell-mode`). */
    val mode: ShellMode? = null,
    /** Open a sheet on launch: `inbox`, `quick-task`, `continue-cli`. */
    val sheet: String? = null,
    /** The inbox item a link asked for (iOS `allternit://inbox/<id>`). */
    val inboxItem: String? = null,
)

/** iOS ModeBarItem raw values and the web rail ids → the drawer row they open. */
private fun launchSection(tab: String): RailRow? {
    val id = when (tab) {
        "artifacts" -> "artifacts-library"
        "automation", "tasks" -> "automation-tasks"
        "agents", "bots" -> "bots"
        else -> tab
    }
    return ShellMode.entries.flatMap { railRows(it) }.firstOrNull { it.id == id }
        ?: when (id) {
            "models" -> RailRow("models", "Models", DesignR.drawable.ph_cpu)
            "connectors" -> RailRow("connectors", "Apps & Extensions", DesignR.drawable.ph_plugs_connected)
            else -> null
        }
}

private fun drawerOpenNow(drawer: AnchoredDraggableState<DrawerValue>): Boolean = drawer.settledValue == DrawerValue.Open

/**
 * The StageBackdrop behind a project screen (iOS ProjectDetailView: the project's own look, else
 * the Settings default for bot project homes; "plain" draws nothing over the page colour).
 */
@Composable
private fun ProjectBackdrop(scopeId: String) {
    val context = LocalContext.current
    val backdrops = remember(context) { StageBackdropStore.get(context) }
    val variant = backdrops.variantFor(BackdropSurface.Project, BackdropScope.project(scopeId))
    if (variant != "plain") StageBackdrop(variant = variant, modifier = Modifier.fillMaxSize())
}
