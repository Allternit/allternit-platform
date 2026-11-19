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
    // A notification tap while the workspace is up (iOS DeepLinkRouter): open what it points at.
    LaunchedEffect(launch.nonce) {
        if (launch.nonce == 0L) return@LaunchedEffect
        launch.mode?.let { mode = it }
        if (launch.tab == "settings") page = ShellPage.Settings
        if (launch.sheet == "inbox") inboxOpen = true
    }

    fun reload() {
        if (account != null) scope.launch { store.load() }
    }
    // A studio session's standing instructions ride on every turn (iOS ChatViewModel systemPrompt).
    chat.systemPromptFor = { sid -> com.allternit.feature.artifacts.ArtifactStudioPrompts.systemPrompt(context, sid) }
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

    // Wave E: Bots mode (iOS BotsModeRootView), with the shell's chat backend, composer and backdrop.
    val botsStore = remember { com.allternit.feature.bots.BotModeStore(context) }
    var botsLaunch by remember { mutableStateOf(com.allternit.feature.bots.BotsLaunch()) }
    // Wave F4a: the live view of a linked phone (iOS ACIComputersPane fullScreenCover PhoneLiveView).
    var phoneLive by remember { mutableStateOf<com.allternit.feature.computer.CloudPhone?>(null) }

    // Wave F1: Code mode (iOS CodeModeView). The thread is this shell's chat screen on a code session;
    // the Code home, the tools and the terminal come from :feature:code.
    val codeStore = remember { com.allternit.feature.code.CodeStore(chatBackend) }
    var codeRoute by remember { mutableStateOf<com.allternit.feature.code.CodeRoute?>(null) }
    val codeHost = com.allternit.feature.code.CodeHost(
        backend = chatBackend,
        sessions = {
            store.of(ShellMode.Code).map { sn ->
                com.allternit.feature.code.CodeSessionRow(
                    id = sn.id,
                    title = sn.title,
                    updatedMillis = sn.timestampMillis,
                    status = when {
                        sn.archived -> com.allternit.feature.code.CodeSessionStatus.Archived
                        sn.needsInput -> com.allternit.feature.code.CodeSessionStatus.NeedsInput
                        chat.isStreaming && chat.currentSessionId == sn.id -> com.allternit.feature.code.CodeSessionStatus.Working
                        sn.readyForReview -> com.allternit.feature.code.CodeSessionStatus.ReadyForReview
                        sn.completed -> com.allternit.feature.code.CodeSessionStatus.Done
                        else -> com.allternit.feature.code.CodeSessionStatus.Idle
                    },
                )
            } + store.archivedOf(ShellMode.Code).map { sn ->
                com.allternit.feature.code.CodeSessionRow(sn.id, sn.title, sn.timestampMillis, com.allternit.feature.code.CodeSessionStatus.Archived)
            }
        },
        reloadSessions = { if (account != null) store.load() },
        openSession = { id -> codeRoute = null; openSessionId(id) },
        currentSessionId = { chat.currentSessionId },
        openConnections = {
            codeRoute = null
            page = ShellPage.Section(RailRow("connectors", "Apps & Extensions", DesignR.drawable.ph_plugs_connected))
        },
    )

    // Wave D: what the Projects and Artifacts screens need from the shell.
    val shellMenuButton: @Composable () -> Unit = {
        PhoneMenuButton(mode = mode, hold = hold, holdEnabled = !drawerOpenNow(drawer), onOpen = { setDrawer(true) }, onPick = ::pickMode)
    }
    // Wave F3: ACI mode's host (iOS ACITabView): the relay for runs and workflows, ☰, sign-in.
    val aciHost = com.allternit.feature.computer.AciHost(
        signedIn = account != null,
        backend = chatBackend,
        menuButton = shellMenuButton,
        openSignIn = { page = ShellPage.Settings },
    )
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
            openBotProject = { id ->
                mode = ShellMode.Bots
                ShellModePrefs.save(context, ShellMode.Bots)
                botsLaunch = com.allternit.feature.bots.BotsLaunch(projectId = id)
                page = ShellPage.Home
            },
            runtimeGet = { path ->
                when (val r = chatBackend.runtimeJson("GET", path)) {
                    is com.allternit.feature.chat.RuntimeCall.Ok -> Result.success(r.body)
                    com.allternit.feature.chat.RuntimeCall.NoComputer -> Result.failure(IllegalStateException("No computer is connected."))
                    is com.allternit.feature.chat.RuntimeCall.Http -> Result.failure(IllegalStateException(r.message ?: "HTTP ${r.status}"))
                    is com.allternit.feature.chat.RuntimeCall.Failed -> Result.failure(IllegalStateException(r.message))
                }
            },
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

    remember { com.allternit.feature.office.OfficeFeature.install() }
    val botsHost = com.allternit.feature.bots.BotsHost(
        signedIn = account != null,
        backend = chatBackend,
        menuButton = shellMenuButton,
        composer = { spec ->
            Composer(
                models = models,
                noComputer = store.noComputer,
                phoneArmed = false,
                onClearPhone = {},
                onSend = { draft -> spec.onSend(draft.toChatRequest()) },
                onOpenSection = { row -> page = ShellPage.Section(row) },
                isStreaming = spec.isStreaming,
                sendDisabled = spec.sendDisabled,
                onStop = spec.onStop,
                prefill = spec.prefill,
                onPrefillConsumed = spec.onPrefillConsumed,
                pickerRequest = spec.pickerRequest,
                placeholderOverride = spec.placeholder,
            )
        },
        currentModel = { models.selectedId to models.selected?.let { models.level(it) } },
        backdrop = { botId, hex -> BotBackdrop(botId, hex) },
        openConnections = { page = ShellPage.Section(RailRow("connectors", "Apps & Extensions", DesignR.drawable.ph_plugs_connected)) },
        openSignIn = { page = ShellPage.Settings },
        openArtifact = { a -> page = ShellPage.Artifact(com.allternit.feature.artifacts.ArtifactRecordInput(a.id, a.kind, a.title, a.url, a.content), back = page) },
        openProjectInLibrary = { id -> page = ShellPage.Project(id) },
        openInbox = { inboxOpen = true },
        officeFloor = com.allternit.feature.office.OfficeFeature.floor,
    )

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
                        ShellPage.Home -> if (mode == ShellMode.Bots) {
                            com.allternit.feature.bots.BotsModeRoot(botsHost, botsStore, botsLaunch)
                        } else if (mode == ShellMode.Aci) {
                            // Wave F3: ACI mode (iOS ACITabView): the agent landing, browser, run, Computers.
                            com.allternit.feature.computer.AciRoot(aciHost)
                        } else ShellHome(
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
                            codeHome = if (mode == ShellMode.Code) ({
                                com.allternit.feature.code.CodeHome(codeHost, codeStore, onRoute = { codeRoute = it })
                            }) else null,
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
                            // Wave E: the drawer's "Your bots" and a bot from search open Bots mode's root.
                            "bots" -> com.allternit.feature.bots.BotsModeRoot(botsHost, botsStore, botsLaunch)
                            // Wave F3: the drawer's ACI rail opens that ACI view (iOS ACINavStore.open).
                            // The drawer's Teach row opens Teach over ACI (iOS ACINavStore.openTeach).
                            "site-apis" -> {
                                LaunchedEffect(Unit) { com.allternit.feature.computer.AciNav.open(com.allternit.feature.computer.AciTab.Agent); com.allternit.feature.computer.AciNav.openTeach() }
                                com.allternit.feature.computer.AciRoot(aciHost)
                            }
                            "mcp-apps", "browser-extensions", "computers" -> {
                                val aciTab = when (current.row.id) {
                                    "mcp-apps" -> com.allternit.feature.computer.AciTab.Apps
                                    "browser-extensions" -> com.allternit.feature.computer.AciTab.Extensions
                                    else -> com.allternit.feature.computer.AciTab.Computers
                                }
                                LaunchedEffect(aciTab) { com.allternit.feature.computer.AciNav.open(aciTab) }
                                androidx.compose.foundation.layout.Box(Modifier.fillMaxSize()) {
                                    com.allternit.feature.computer.AciRoot(aciHost.copy(openPhoneLive = { phoneLive = it }))
                                    phoneLive?.let { p ->
                                        androidx.compose.foundation.layout.Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary)) {
                                            com.allternit.feature.phones.PhoneLiveView(p, chatBackend, onClose = { phoneLive = null })
                                        }
                                    }
                                }
                            }
                            // Wave F2: Automation Tasks (both modes' rows) opens the hub: goals, routines, loops, heartbeats, the board.
                            "automation-tasks", "code-automations" -> com.allternit.feature.automation.AutomationRoot(
                                com.allternit.feature.automation.AutomationHost(
                                    signedIn = account != null,
                                    backend = chatBackend,
                                    menuButton = shellMenuButton,
                                    openSignIn = { page = ShellPage.Settings },
                                ),
                            )
                            // Wave F4a: Cowork (iOS CoworkWorkspaceView; the tasks sheet from the composer's plus), Models, Phones.
                            "cowork", "cowork-tasks" -> com.allternit.feature.cowork.CoworkRoot(
                                com.allternit.feature.cowork.CoworkHost(
                                    signedIn = account != null,
                                    backend = chatBackend,
                                    menuButton = shellMenuButton,
                                    openSignIn = { page = ShellPage.Settings },
                                    openChat = ::openSessionId,
                                ),
                                initialTasks = current.row.id == "cowork-tasks",
                            )
                            // Wave G2: Customize (iOS CustomizeScreen) and Connectors (iOS ConnectorsListView), from the drawer and the + sheet.
                            "customize" -> com.allternit.feature.settings.CustomizeRoot(
                                com.allternit.feature.settings.SettingsHost(
                                    account = account,
                                    backend = chatBackend,
                                    versionName = "",
                                    onClose = { page = ShellPage.Settings },
                                    connections = { _ -> },
                                    openPhoneControl = { page = ShellPage.Section(RailRow("phone-control", "Phone control", DesignR.drawable.ph_cpu)) },
                                    openModels = { page = ShellPage.Section(RailRow("models", "Models", DesignR.drawable.ph_cpu)) },
                                ),
                                onBack = { page = ShellPage.Home },
                            )
                            "connectors" -> com.allternit.feature.settings.ConnectorsRoot(onBack = { page = ShellPage.Home })
                            "models" -> com.allternit.feature.models.ModelsRoot(
                                com.allternit.feature.models.ModelsHost(
                                    signedIn = account != null,
                                    backend = chatBackend,
                                    menuButton = shellMenuButton,
                                    onRun = { id ->
                                        models.select(id)
                                        chat.startNewSession()
                                        page = ShellPage.Home
                                    },
                                ),
                            )
                            "phones", "phone-control", "android-screen", "vendor-connect" -> {
                                val phonesHost = com.allternit.feature.phones.PhonesHost(
                                    signedIn = account != null,
                                    backend = chatBackend,
                                    openSignIn = { page = ShellPage.Settings },
                                    openBotPhone = { botId ->
                                        mode = ShellMode.Bots
                                        ShellModePrefs.save(context, ShellMode.Bots)
                                        botsLaunch = com.allternit.feature.bots.BotsLaunch(openBotId = botId)
                                        page = ShellPage.Home
                                    },
                                    openInbox = { inboxOpen = true },
                                    openPhoneControl = { page = ShellPage.Section(RailRow("phone-control", "Phone control", DesignR.drawable.ph_cpu)) },
                                    openAndroidScreen = { page = ShellPage.Section(RailRow("android-screen", "Android phone", DesignR.drawable.ph_cpu)) },
                                    openVendorConnect = { page = ShellPage.Section(RailRow("vendor-connect", "Connect a subscription", DesignR.drawable.ph_plugs_connected)) },
                                )
                                when (current.row.id) {
                                    "phones" -> {
                                        androidx.compose.foundation.layout.Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary))
                                        com.allternit.feature.phones.PhonesSheet(phonesHost, onDismiss = { page = ShellPage.Home })
                                    }
                                    "phone-control" -> androidx.compose.foundation.layout.Box(Modifier.fillMaxSize()) {
                                        com.allternit.feature.phones.AppPhonesSettingsScreen(phonesHost, onBack = { page = ShellPage.Home }, onOpenLive = { phoneLive = it })
                                        phoneLive?.let { p ->
                                            androidx.compose.foundation.layout.Box(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary)) {
                                                com.allternit.feature.phones.PhoneLiveView(p, chatBackend, onClose = { phoneLive = null })
                                            }
                                        }
                                    }
                                    "android-screen" -> com.allternit.feature.phones.AndroidScreenPairingScreen(phonesHost, onBack = { page = ShellPage.Home })
                                    else -> com.allternit.feature.phones.VendorConnectScreen(phonesHost, onBack = { page = ShellPage.Home }, onReady = { })
                                }
                            }
                            // Wave H2: Meetings (iOS MeetingsView, opened from the drawer's Meetings button).
                            "meetings" -> com.allternit.feature.meetings.MeetingsScreen(
                                com.allternit.feature.meetings.MeetingsHost(
                                    signedIn = account != null,
                                    onClose = { page = ShellPage.Home },
                                    openComputers = { page = ShellPage.Section(RailRow("computers", "Computers", DesignR.drawable.ph_desktop)) },
                                    openMeetingsSettings = { page = ShellPage.Settings },
                                ),
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
                        // Wave G1: Settings part 1 (iOS SettingsView): the root, Advanced and the Account, Privacy, Notifications,
                        // Connections, Memory, Devices, Models, Subagents, MCP and Webhooks pages.
                        ShellPage.Settings -> com.allternit.feature.settings.SettingsRoot(
                            com.allternit.feature.settings.SettingsHost(
                                account = account,
                                backend = chatBackend,
                                versionName = remember { runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull().orEmpty() },
                                onClose = { page = ShellPage.Home },
                                connections = { onBack ->
                                    com.allternit.feature.shell.firstrun.ConnectionsSettingsScreen(
                                        onBack = onBack,
                                        openPhoneControl = { page = ShellPage.Section(RailRow("phone-control", "Phone control", DesignR.drawable.ph_cpu)) },
                                        openAndroidScreen = { page = ShellPage.Section(RailRow("android-screen", "Android phone", DesignR.drawable.ph_cpu)) },
                                    )
                                },
                                openPhoneControl = { page = ShellPage.Section(RailRow("phone-control", "Phone control", DesignR.drawable.ph_cpu)) },
                                openModels = { page = ShellPage.Section(RailRow("models", "Models", DesignR.drawable.ph_cpu)) },
                                // Files & devices → Open Cowork workspace (iOS sets the mode to Cowork; Android opens the Cowork section).
                                openCowork = { page = ShellPage.Section(RailRow("cowork", "Cowork", DesignR.drawable.ph_desktop)) },
                            ),
                        )
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
                    // Wave F1: the Code tools over a Code thread, and the full-screen Code tool screens.
                    if (mode == ShellMode.Code && page is ShellPage.Chat) {
                        com.allternit.feature.code.CodeThreadTools(
                            host = codeHost,
                            sessionId = chat.currentSessionId,
                            onRoute = { codeRoute = it },
                            modifier = Modifier.align(androidx.compose.ui.Alignment.TopEnd)
                                .padding(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 52.dp),
                        )
                    }
                    codeRoute?.let { r ->
                        Box(Modifier.fillMaxSize().testTag("codeRoute")) {
                            com.allternit.feature.code.CodeRouteScreen(codeHost, codeStore, r, onRoute = { codeRoute = it }, onClose = { codeRoute = null })
                        }
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
    /** Non-zero for a notification tap (wave I1): a new value re-applies the launch to a workspace already up. */
    val nonce: Long = 0L,
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

/** The StageBackdrop behind a bot chat (iOS BotChatSessionView: the bot's look, else the Bot chats default). */
@Composable
private fun BotBackdrop(botId: String, colorHex: String?) {
    val context = LocalContext.current
    val backdrops = remember(context) { StageBackdropStore.get(context) }
    val variant = backdrops.variantFor(BackdropSurface.Bot, BackdropScope.bot(botId))
    if (variant != "plain") StageBackdrop(variant = variant, modifier = Modifier.fillMaxSize(), colors = listOf(colorHex))
}
