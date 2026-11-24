package com.allternit.feature.bots

import com.allternit.core.design.AllternitSerif
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
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
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.glassSurface
import com.allternit.feature.chat.ArtifactRecord
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.ChatSendRequest
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import com.allternit.core.design.R as DesignR

/** What the composer slot gets from a Bots screen (iOS ComposerView with placeholderOverride / accentBorder). */
data class BotComposerSpec(
    val placeholder: String,
    val isStreaming: Boolean = false,
    val sendDisabled: Boolean = false,
    val prefill: String? = null,
    val onPrefillConsumed: () -> Unit = {},
    val onSend: (ChatSendRequest) -> Unit,
    val onStop: () -> Unit = {},
    /** Bumped to open the model picker (a `no_brain` send, "Choose a model"). */
    val pickerRequest: Int = 0,
    /** The composer's voice button (iOS ComposerView onVoiceMode in a bot's ChatView); null leaves it disabled. */
    val onVoiceMode: (() -> Unit)? = null,
    /** A bot chat's Runtime · Brain · Bot state: the composer shows the three agent chips in place of the model chip. */
    val agent: com.allternit.feature.bots.runtimes.BotAgentState? = null,
    val sessionOpen: Boolean = false,
    val onNewThread: (() -> Unit)? = null,
)

/**
 * What Bots mode needs from the shell: the shell's chat backend (sessions, relay), its menu button,
 * its composer (model picker, attachments, dictation), the StageBackdrop and a few cross-section links.
 */
class BotsHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val menuButton: @Composable () -> Unit,
    val composer: @Composable (BotComposerSpec) -> Unit,
    /** The composer's picked model and effort, for the chat's error cards (ChatCallbacks.currentModel). */
    val currentModel: () -> Pair<String?, String?>,
    /** The stage backdrop behind a bot chat (iOS StageBackdropView, bot scope); "plain" draws the washed page. */
    val backdrop: @Composable (botId: String, colorHex: String?) -> Unit,
    val openConnections: () -> Unit,
    val openSignIn: () -> Unit,
    val openArtifact: (ArtifactRecord) -> Unit,
    /** Opens a bot project in the Projects section's normal project detail (since #2629). */
    val openProjectInLibrary: (projectId: String) -> Unit,
    /** The Inbox phone-hold Review (wave C), reused by the decisions deck. */
    val openInbox: () -> Unit,
    /** The bots' Office floor (iOS OfficeFloorView), from :feature:office, which depends on this module: the store, the chat backend, open a bot's chat, back, and the Bots views its thread sheet reuses ([BotsOfficeKit]). Null hides the entry. */
    val officeFloor: (@Composable (store: BotModeStore, backend: ChatBackend, openBot: (String) -> Unit, onBack: () -> Unit, kit: BotsOfficeKit) -> Unit)? = null,
    /**
     * The shell's main-bot picker (first run's "Who leads your team?" step in a sheet), for the Office's "Pick your main
     * bot" ([BotsOfficeKit.MainBotSetup]). [onDone] runs after the choice was applied. Null falls back to [NewBotSheet].
     */
    val mainBotPicker: (@Composable (onDismiss: () -> Unit, onDone: () -> Unit) -> Unit)? = null,
)

/**
 * The Bots views the Office floor's thread sheet (step 26, iOS OfficeThreadSheet) embeds instead of rebuilding them:
 * the bot's main chat ([ChatScreen] on the bot's [ChatViewModel] with the shell composer, the same stack as
 * [BotChatSessionScreen], tool steps and ask cards included) and its computer ([BotComputerScreen] with Take over /
 * Hand back). Made by Bots mode, which owns the host and the navigator these need.
 */
class BotsOfficeKit internal constructor(private val host: BotsHost, private val store: BotModeStore, private val nav: BotNavigator) {
    val backend: ChatBackend get() = host.backend

    /** Pushes the bot's full chat screen over the floor. */
    fun openFullChat(botId: String) = nav.open(BotRoute.Chat(botId, null))

    /** The bot's settings (iOS AgentDetailView: Behavior, trust tier), the chief dock's Autopilot sheet links here. */
    fun openBotSettings(botId: String) = nav.open(BotRoute.Settings(botId))

    /** The shell's sign-in (Settings), for the chief dock's signed-out state. */
    fun openSignIn() = host.openSignIn()

    /** The shell's existing main-bot picker ([BotsHost.mainBotPicker]), or null when the shell didn't provide one. */
    val mainBotPicker: (@Composable (onDismiss: () -> Unit, onDone: () -> Unit) -> Unit)? get() = host.mainBotPicker

    /**
     * "Pick your main bot" (step 27, iOS MainBotSetupSheet). An account with bots chooses one in the shell's picker
     * ([mainBotPicker]: existing bots, A:// and its looks); an account with no bots (or no picker) gets Bots mode's
     * new-bot sheet ([NewBotSheet]), where the first bot made becomes the main bot ([BotFormData.needsMain]).
     * [onDone] runs after the roster is refreshed.
     */
    @Composable
    fun MainBotSetup(onDismiss: () -> Unit, onDone: () -> Unit) {
        val scope = rememberCoroutineScope()
        val picker = host.mainBotPicker
        if (picker != null && store.bots.isNotEmpty()) {
            picker(onDismiss) { scope.launch { store.refreshAll(); onDone() } }
        } else {
            NewBotSheet(
                store = store, host = host, onBrowseTemplates = null, onDismiss = onDismiss,
                onCreated = { scope.launch { store.refreshAll(); onDone() } },
            )
        }
    }

    /**
     * The bot's main chat, embedded. [refreshKey] re-reads the session's permission asks (bump it after an approval
     * from the sheet's card so the chat's own card closes too).
     */
    @Composable
    fun Thread(botId: String, refreshKey: Int = 0, modifier: Modifier = Modifier) {
        val bot = store.bot(botId) ?: return
        val scope = rememberCoroutineScope()
        val vm = rememberBotChat(host, bot)
        var sessionId by remember(bot.id) { mutableStateOf<String?>(null) }
        var error by remember(bot.id) { mutableStateOf<String?>(null) }
        var prefill by remember { mutableStateOf<String?>(null) }
        var editingId by remember { mutableStateOf<String?>(null) }
        var pickerRequest by remember { mutableStateOf(0) }
        suspend fun resolve() {
            error = null
            when (val r = store.mainChatSessionId(bot)) {
                is BotsResult.Ok -> sessionId = r.value
                is BotsResult.Failed -> error = r.message
            }
        }
        LaunchedEffect(bot.id) { resolve() }
        LaunchedEffect(sessionId) { sessionId?.let { vm.loadSession(it) } }
        LaunchedEffect(refreshKey) { if (refreshKey > 0) vm.refreshPermissions() }
        val callbacks = remember(vm) {
            com.allternit.feature.chat.ChatCallbacks(
                onChooseModel = { pickerRequest += 1 },
                currentModel = host.currentModel,
                onEdit = { m -> editingId = vm.lastUserMessageId; prefill = com.allternit.feature.chat.ChatViewModel.strippedText(m.content) },
                onOpenSettings = host.openConnections,
                onConnectConnector = { host.openConnections() },
                onOpenArtifact = host.openArtifact,
            )
        }
        Box(modifier.testTag("officeSheetThread")) {
            when {
                sessionId != null -> com.allternit.feature.chat.ChatScreen(
                    viewModel = vm, backend = host.backend, callbacks = callbacks,
                    editing = editingId != null, onCancelEdit = { editingId = null; prefill = "" },
                    greeting = {},
                    composer = {
                        host.composer(BotComposerSpec(
                            sessionOpen = true, onNewThread = { nav.open(BotRoute.Chat(bot.id, null, newThread = true)) },
                            placeholder = "Message ${bot.displayName}…", isStreaming = vm.isStreaming, sendDisabled = vm.isCreatingSession,
                            prefill = prefill, onPrefillConsumed = { prefill = null }, pickerRequest = pickerRequest, onStop = vm::stopStreaming,
                            onSend = { req -> val e = editingId; if (e != null) { editingId = null; vm.resendEdited(e, req) } else vm.send(req) },
                        ))
                    },
                    modifier = Modifier.fillMaxSize(),
                )
                error != null -> FriendlyState(FriendlyStyle.Error, "Couldn't open ${bot.displayName}", error.orEmpty(), actionTitle = "Try again", onAction = { scope.launch { resolve() } }) {
                    Icon(painterResource(com.allternit.core.design.R.drawable.ph_chat_circle), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp))
                }
                else -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LibrarySpinnerBox() }
            }
        }
    }

    /** The bot's computer: its desktop (View desktop, Take over / Hand back) and assigned computers. */
    @Composable
    fun Screen(botId: String, modifier: Modifier = Modifier) {
        val bot = store.bot(botId) ?: return
        Box(modifier) { BotComputerScreen(bot, nav, embedded = true) }
    }
}

/** Every screen Bots mode can push (iOS BotRoute, cut to the wave E screens). */
sealed interface BotRoute {
    data class Chat(val botId: String, val sessionId: String?, val newThread: Boolean = false) : BotRoute
    data class Page(val botId: String) : BotRoute
    data class Threads(val botId: String?) : BotRoute
    data class Project(val project: BotProjectSummary) : BotRoute
    data class Tasks(val botId: String) : BotRoute
    data class Channels(val botId: String) : BotRoute
    data class Computer(val botId: String) : BotRoute
    data class Stage(val botId: String) : BotRoute
    /** The full Bot Home dashboard (iOS BotHomeView). */
    data class Home(val botId: String) : BotRoute
    /** The agent editor (iOS AgentDetailView). */
    data class Settings(val botId: String) : BotRoute
    data class BotHome(val botId: String, val tab: BotHomeTab) : BotRoute
    data class Group(val groupId: String) : BotRoute
    /** Group chats (iOS GroupsListView). */
    data object Groups : BotRoute
    data class ProjectThread(val thread: BotThread) : BotRoute
    data object Hub : BotRoute
    data object Marketplace : BotRoute
    data class Listing(val id: String) : BotRoute
    data class FileEditor(val botId: String, val file: WorkspaceFileInfo) : BotRoute
    data object Activity : BotRoute
    data class ActivityDetail(val thread: AgentActivityThread) : BotRoute
    data object Decisions : BotRoute
    /** The bots' Office floor (iOS BotRoute.office). */
    data object Office : BotRoute
}

/** The bot a pushed route is about, for the mode orb's face; null for team-level routes. */
internal fun BotRoute.focusedBotId(): String? = when (this) {
    is BotRoute.Chat -> botId
    is BotRoute.Page -> botId
    is BotRoute.Threads -> botId
    is BotRoute.Tasks -> botId
    is BotRoute.Channels -> botId
    is BotRoute.Computer -> botId
    is BotRoute.Stage -> botId
    is BotRoute.Home -> botId
    is BotRoute.Settings -> botId
    is BotRoute.BotHome -> botId
    is BotRoute.FileEditor -> botId
    else -> null
}

/** Bots mode's back stack (iOS BotNavigator). */
class BotNavigator {
    val path = mutableStateListOf<BotRoute>()
    fun open(route: BotRoute) { path.add(route) }
    fun back() { if (path.isNotEmpty()) path.removeAt(path.lastIndex) }
    fun reset(route: BotRoute? = null) { path.clear(); route?.let { path.add(it) } }
    /** iOS openThread: its chat, or its waiting page while the thread has no session. */
    fun openThread(thread: BotThread) {
        val sid = thread.currentSessionId
        if (sid != null) open(BotRoute.Chat(thread.botId, sid)) else open(BotRoute.ProjectThread(thread))
    }
}

/** Bots mode pause (iOS BotPauseModel): `GET/PUT /api/v1/bots/pause` on the computer. */
internal class BotPauseModel(private val backend: ChatBackend) {
    var paused by mutableStateOf(false); private set

    suspend fun refresh() {
        (backend.runtimeResult("GET", "/api/v1/bots/pause", doing = "check pause").orNull() as? kotlinx.serialization.json.JsonObject)
            ?.get("paused")?.let { paused = (it as? JsonPrimitive)?.booleanOrNull ?: paused }
    }

    suspend fun toggle() {
        val body = "{\"paused\":${!paused},\"by\":\"android\"}"
        (backend.runtimeResult("PUT", "/api/v1/bots/pause", body, "pause your bots").orNull() as? kotlinx.serialization.json.JsonObject)
            ?.get("paused")?.let { paused = (it as? JsonPrimitive)?.booleanOrNull ?: paused }
    }
}

/** The roster layouts (iOS OverviewLayout list / grid / deck). */
enum class RosterLayout(val raw: String, val label: String) { List("list", "List"), Grid("grid", "Grid"), Deck("deck", "Deck") }

/** Where the shell asks Bots mode to go on open (iOS BotModeStore.pendingOpenBotId / pendingRoute). */
class BotsLaunch(val openBotId: String? = null, val route: BotRoute? = null, val projectId: String? = null, val openMain: Boolean = false, val openSessionId: String? = null, val resetRoot: Boolean = false)

/**
 * Bots mode root (iOS BotsModeRootView): header "Bots" with search and the options sheet, the
 * paused banner, then the launchpad; every pushed screen draws over it from [BotNavigator].
 */
@Composable
fun BotsModeRoot(host: BotsHost, store: BotModeStore, launch: BotsLaunch = BotsLaunch()) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val nav = remember { BotNavigator() }
    val pause = remember { BotPauseModel(host.backend) }
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var optionsOpen by remember { mutableStateOf(false) }
    var threadsOpen by remember { mutableStateOf(false) }

    LaunchedEffect(launch) {
        if (launch.resetRoot) nav.reset()
        launch.openBotId?.let { nav.reset(BotRoute.Chat(it, launch.openSessionId)) }
        launch.route?.let { nav.reset(it) }
        // allternit://call/main (iOS .callMain): the main bot's chat, or the bots list when there is none.
        if (launch.openMain) {
            store.refreshAgents()
            store.agents.firstOrNull { it.isMainBot }?.let { nav.reset(BotRoute.Chat(it.id, null)) }
        }
        // A bot project from Projects or search (iOS pendingRoute .project).
        launch.projectId?.let { id ->
            store.refreshProjects()
            store.projects.firstOrNull { it.id == id }?.let { nav.reset(BotRoute.Project(it)) }
        }
    }
    LaunchedEffect(host.signedIn) { if (host.signedIn) pause.refresh() }
    BackHandler(enabled = nav.path.isNotEmpty()) { nav.back() }

    val top = nav.path.lastOrNull()
    // The shell's mode orb shows this bot's face; the team page (and leaving Bots mode) clears it.
    val focusId = top?.focusedBotId()
    LaunchedEffect(focusId) { store.focusedBotId = focusId }
    DisposableEffect(store) { onDispose { store.focusedBotId = null } }
    if (top != null) {
        Box(Modifier.fillMaxSize().background(c.bgPrimary)) { BotRouteDestination(top, host, store, nav) }
        return
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("botsModeRoot")) {
        LibraryHeader(
            title = "Bots", onBack = null, leading = host.menuButton,
            trailing = {
                // iOS ProjectsPill: one glass capsule with Search bots and Bots options (Office, Threads,
                // Pause and the rest live in the options sheet, as in iOS BotsOptionsPopover).
                Row(Modifier.glassSurface(CircleShape).padding(horizontal = 2.dp)) {
                    LibraryIconButton(DesignR.drawable.ph_magnifying_glass, "Search bots", "botsSearch", iconSize = 19) { searching = !searching }
                    Box {
                        LibraryIconButton(
                            DesignR.drawable.ph_sliders_horizontal,
                            if (store.waitingCount > 0) "Bots options · ${store.waitingCount} waiting on you" else "Bots options", "botsOptions",
                            iconSize = 19,
                        ) { optionsOpen = true }
                        if (store.waitingCount > 0) Box(Modifier.align(Alignment.TopEnd).padding(top = 10.dp, end = 10.dp)) { Dot(AllternitStatic.StatusWarning, 7.dp) }
                    }
                }
            },
        )
        if (pause.paused) PausedBanner()
        if (searching || query.isNotEmpty()) LibrarySearchField(query, "Search bots", "botsSearchField", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { query = it }
        if (!host.signedIn) {
            FriendlyState(
                FriendlyStyle.Empty, "Sign in to meet your bots", "Your bots, their threads and group chats live in your account.",
                actionTitle = "Sign in", onAction = host.openSignIn,
            ) { Icon(painterResource(com.allternit.core.design.R.drawable.ph_house), null, tint = c.accentPrimary, modifier = Modifier.size(28.dp)) }
        } else {
            BotLaunchpad(host, store, nav, query, Modifier.weight(1f))
        }
    }

    if (optionsOpen) BotsOptionsSheet(
        waiting = store.waitingCount, paused = pause.paused,
        onDismiss = { optionsOpen = false },
        onThreads = { optionsOpen = false; threadsOpen = true },
        onOffice = if (host.officeFloor != null) ({ optionsOpen = false; nav.open(BotRoute.Office) }) else null,
        onHub = { optionsOpen = false; nav.open(BotRoute.Hub) },
        onDecisions = { optionsOpen = false; nav.open(BotRoute.Decisions) },
        onActivity = { optionsOpen = false; nav.open(BotRoute.Activity) },
        onGroups = { optionsOpen = false; nav.open(BotRoute.Groups) },
        onTogglePause = { scope.launch { pause.toggle() } },
        onConnections = { optionsOpen = false; host.openConnections() },
    )
    if (threadsOpen) LibrarySheet(title = "Threads", testTag = "botsThreadsSheet", onDismiss = { threadsOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { threadsOpen = false }) {
        BotThreadsOverview(store, botId = null, title = "Welcome back.", modifier = Modifier.heightIn(max = 620.dp)) { t -> threadsOpen = false; nav.openThread(t) }
    }
}

/** iOS PausedBanner. */
@Composable
private fun PausedBanner() {
    Text(
        "All bots are paused. Routines and automatic replies are on hold. Your own messages still run.",
        color = AllternitTheme.colors.textPrimary, fontSize = 13.sp,
        modifier = Modifier.fillMaxWidth().background(AllternitStatic.StatusWarning.copy(alpha = 0.16f)).padding(horizontal = 16.dp, vertical = 10.dp).testTag("botsPausedBanner"),
    )
}

/** iOS BotsOptionsPopover as a sheet (no menus): layout, Threads, Agent hub, Decisions, Activity, Pause, Connections. */
@Composable
private fun BotsOptionsSheet(
    waiting: Int, paused: Boolean, onDismiss: () -> Unit, onThreads: () -> Unit, onOffice: (() -> Unit)?, onHub: () -> Unit, onDecisions: () -> Unit,
    onActivity: () -> Unit, onGroups: () -> Unit, onTogglePause: () -> Unit, onConnections: () -> Unit,
) {
    val context = LocalContext.current
    var layout by remember { mutableStateOf(rosterLayout(context)) }
    LibrarySheet(title = "Bots options", testTag = "botsOptionsSheet", onDismiss = onDismiss, trailing = LibrarySheetAction("Done", bold = true, onClick = onDismiss)) {
        PillRow(RosterLayout.entries, layout, { it.label }, { "botsLayout${it.label}" }, Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
            layout = it
            saveRosterLayout(context, it)
        }
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            NavRow("Threads", com.allternit.core.design.R.drawable.ph_list, if (waiting > 0) "$waiting waiting" else null, "botsOptionsThreads", last = false, onClick = onThreads)
            if (onOffice != null) NavRow("Office", com.allternit.core.design.R.drawable.ph_house, null, "botsOptionsOffice", last = false, onClick = onOffice)
            NavRow("Agent hub", com.allternit.core.design.R.drawable.ph_squares_four, null, "botsOptionsHub", last = false, onClick = onHub)
            NavRow("Decisions", com.allternit.core.design.R.drawable.ph_question, null, "botsOptionsDecisions", last = false, onClick = onDecisions)
            NavRow("Agent activity", com.allternit.core.design.R.drawable.ph_chat_circle, null, "botsOptionsActivity", last = false, onClick = onActivity)
            NavRow("Group chats", com.allternit.core.design.R.drawable.ph_chat_circle, null, "botsOptionsGroups", last = false, onClick = onGroups)
            NavRow(if (paused) "Resume all bots" else "Pause all bots", if (paused) DesignR.drawable.ph_play_fill else com.allternit.core.design.R.drawable.ph_stop_fill, null, "botsOptionsPause", last = false, onClick = onTogglePause)
            NavRow("Connections", DesignR.drawable.ph_plugs_connected, null, "botsOptionsConnections", onClick = onConnections)
        }
        Gap(16.dp)
    }
}

internal fun rosterLayout(context: android.content.Context): RosterLayout {
    val raw = context.getSharedPreferences("allternit_bots", android.content.Context.MODE_PRIVATE).getString("bots-home-roster", "list")
    return RosterLayout.entries.firstOrNull { it.raw == raw } ?: RosterLayout.List
}

internal fun saveRosterLayout(context: android.content.Context, l: RosterLayout) =
    context.getSharedPreferences("allternit_bots", android.content.Context.MODE_PRIVATE).edit().putString("bots-home-roster", l.raw).apply()

/** A roster entry (iOS RosterEntry): a bot or a group chat. */
private sealed interface RosterEntry {
    val key: String
    val name: String
    val status: RosterStatus

    data class Bot(val bot: BotRecord, val main: Boolean, val pinned: Boolean, override val status: RosterStatus) : RosterEntry {
        override val key get() = "bot:${bot.id}"
        override val name get() = bot.displayName
    }

    data class Group(val group: BotGroupChat, override val status: RosterStatus) : RosterEntry {
        override val key get() = "group:${group.id}"
        override val name get() = group.name
    }
}

/**
 * The bots launchpad (iOS BotLaunchpadView): YOUR TEAM (list, grid or deck), the "What should
 * the team work on?" header, and the launch composer with its To: line. A send to one bot starts a
 * thread for it; a send to the team starts a bot project the main bot coordinates.
 */
@Composable
private fun BotLaunchpad(host: BotsHost, store: BotModeStore, nav: BotNavigator, query: String, modifier: Modifier) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var selectedBotId by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var hint by remember { mutableStateOf<String?>(null) }
    var newBotOpen by remember { mutableStateOf(false) }
    // Harness & Bots 2.0: the empty state opens the form on the Runtime · Brain · Bot picker.
    var newBotPicker by remember { mutableStateOf(false) }
    var templatesOpen by remember { mutableStateOf(false) }
    var toPickerOpen by remember { mutableStateOf(false) }
    var templateError by remember { mutableStateOf<String?>(null) }
    var prefill by remember { mutableStateOf<String?>(null) }
    val layout = rosterLayout(context)

    val bots = store.bots
    val mainBot = bots.firstOrNull { it.isMainBot }
    val mainName = mainBot?.displayName ?: "A://"
    val selected = selectedBotId?.let { id -> bots.firstOrNull { it.id == id } }

    fun statusOf(b: BotRecord): RosterStatus = when {
        store.waitingCount(b.id) > 0 -> RosterStatus.Waiting
        store.workingCount(b.id) > 0 -> RosterStatus.Working
        else -> RosterStatus.Idle
    }

    LaunchedEffect(Unit) {
        store.refreshAll()
        // iOS polls threads and projects every 20 s while the launchpad is up.
        while (true) {
            delay(20_000)
            store.refreshThreads()
            store.refreshProjects()
        }
    }

    val entries = buildList<RosterEntry> {
        bots.sortedByDescending { it.isMainBot }.forEach { add(RosterEntry.Bot(it, it.isMainBot, store.isPinned(it.id), statusOf(it))) }
        store.groups.forEach { g ->
            val members = g.memberIds.mapNotNull(store::bot).map(::statusOf)
            add(RosterEntry.Group(g, when {
                RosterStatus.Waiting in members -> RosterStatus.Waiting
                members.any { it == RosterStatus.Working || it == RosterStatus.Talking } -> RosterStatus.Working
                else -> RosterStatus.Idle
            }))
        }
    }.filter { query.isBlank() || it.name.lowercase().contains(query.trim().lowercase()) }

    fun open(e: RosterEntry) = when (e) {
        is RosterEntry.Bot -> nav.open(BotRoute.Chat(e.bot.id, null))
        is RosterEntry.Group -> nav.open(BotRoute.Group(e.group.id))
    }
    fun toggleTo(b: BotRecord) { hint = null; selectedBotId = if (selectedBotId == b.id || b.isMainBot) null else b.id }

    fun send(req: ChatSendRequest) {
        val text = req.text.trim()
        if (text.isEmpty() || busy) return
        hint = null
        busy = true
        scope.launch {
            val bot = selected
            if (bot != null) {
                val sid = store.newThreadSession(bot, BotModeStore.projectTitle(text))
                val sent = (sid as? BotsResult.Ok)?.let { BotsApi.sendUserMessage(it.value, text) }
                if (sid is BotsResult.Ok && sent is BotsResult.Ok) {
                    nav.open(BotRoute.Chat(bot.id, sid.value))
                    store.refreshThreads()
                } else {
                    hint = "${bot.displayName} couldn't start that. Try again."
                    prefill = text
                }
            } else {
                val pid = BotsApi.createBotProject(BotModeStore.projectTitle(text))
                val sent = (pid as? BotsResult.Ok)?.let { BotsApi.sendToCoordinator(it.value, text) }
                if (pid is BotsResult.Ok && sent is BotsResult.Ok) {
                    store.refreshProjects()
                    store.projects.firstOrNull { it.id == pid.value }?.let { nav.open(BotRoute.Project(it)) }
                } else {
                    hint = "$mainName couldn't start that project. Try again."
                    prefill = text
                }
            }
            busy = false
        }
    }

    Column(modifier.fillMaxWidth()) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(top = 8.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(24.dp)) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("YOUR TEAM", color = c.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.8.sp, modifier = Modifier.padding(horizontal = 20.dp))
                when (layout) {
                    RosterLayout.List -> Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        entries.forEach { e -> RosterTile(e, selected = (e as? RosterEntry.Bot)?.bot?.id == selectedBotId, onOpen = { open(e) }, onAt = { (e as? RosterEntry.Bot)?.let { toggleTo(it.bot) } }) }
                        NewBotTile { newBotOpen = true }
                    }
                    RosterLayout.Grid -> Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        (entries.map<RosterEntry, RosterEntry?> { it } + null).chunked(2).forEach { row ->
                            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                row.forEach { e ->
                                    Box(Modifier.weight(1f)) {
                                        if (e == null) NewBotCard { newBotOpen = true }
                                        else RosterCard(e, store, deck = false, selected = (e as? RosterEntry.Bot)?.bot?.id == selectedBotId, onOpen = { open(e) }, onAt = { (e as? RosterEntry.Bot)?.let { toggleTo(it.bot) } })
                                    }
                                }
                                if (row.size == 1) Spacer(Modifier.weight(1f))
                            }
                        }
                    }
                    RosterLayout.Deck -> Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        entries.forEach { e -> Box(Modifier.width(240.dp)) { RosterCard(e, store, deck = true, selected = (e as? RosterEntry.Bot)?.bot?.id == selectedBotId, onOpen = { open(e) }, onAt = { (e as? RosterEntry.Bot)?.let { toggleTo(it.bot) } }) } }
                        Box(Modifier.width(160.dp)) { NewBotCard { newBotOpen = true } }
                    }
                }
                // iOS AccountBotsStrip: the hub error or the no-bots prompt.
                when {
                    store.agentsError != null && bots.isEmpty() -> ErrorLine(store.agentsError ?: "")
                    bots.isEmpty() && !store.agentsLoading -> Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("No bots yet.", color = c.textSecondary, fontSize = 13.sp)
                        val view = androidx.compose.ui.platform.LocalView.current
                        Row(
                            Modifier.heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp)).background(c.fillNeutral)
                                .clickable(role = androidx.compose.ui.semantics.Role.Button) {
                                    com.allternit.core.design.Haptics.perform(view, com.allternit.core.design.Haptics.Kind.Tap)
                                    newBotPicker = true
                                    newBotOpen = true
                                }
                                .testTag("botsEmptyPickHow").padding(horizontal = 16.dp, vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) { Text("Pick how your bot works", color = c.textPrimary, fontSize = 14.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold) }
                    }
                }
            }
            Column(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(
                    selected?.let { "What should ${it.displayName} work on?" } ?: "What should the team work on?",
                    color = c.textPrimary, fontSize = 30.sp, fontFamily = AllternitSerif, lineHeight = 34.sp, modifier = Modifier.testTag("botsLaunchHeadline"),
                )
                Text(
                    selected?.let { "${it.displayName} starts a thread for it and keeps going when you close the app." }
                        ?: "Describe it once. $mainName splits it into threads, picks the bots, and keeps going when you close the app.",
                    color = c.textSecondary, fontSize = 16.sp,
                )
            }
        }
        // The launch composer (iOS BotLaunchComposer): the To: line, the card, the hint.
        Column(Modifier.fillMaxWidth().imePadding().navigationBarsPadding(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(Modifier.padding(horizontal = 18.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("To:", color = c.textTertiary, fontSize = 12.5.sp)
                val view = LocalView.current
                Row(
                    Modifier.clip(CircleShape).testTag("botsLaunchTo").clickable(role = Role.Button, onClickLabel = "Choose who to send to") { Haptics.perform(view, Haptics.Kind.Tap); toPickerOpen = true }
                        .padding(horizontal = 6.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    if (selected != null) {
                        BotAvatar(selected, 18.dp)
                        com.allternit.core.design.BotNameText(selected.displayName, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 1)
                        Text("· ${statusOf(selected).word}", color = statusOf(selected).tone(), fontSize = 12.5.sp)
                    } else com.allternit.core.design.BotNameText("$mainName · new project", color = c.textSecondary, fontSize = 12.5.sp)
                    Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_down), null, tint = c.textTertiary, modifier = Modifier.size(12.dp))
                }
                if (selected != null) Text("×", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.clip(CircleShape).testTag("botsLaunchToClear").clickable { selectedBotId = null }.padding(4.dp))
            }
            host.composer(
                BotComposerSpec(
                    placeholder = selected?.let { "Ask ${it.displayName} to…" } ?: "e.g. Get the pricing page ready for Monday and check we still beat Lambda on H100s…",
                    sendDisabled = busy, prefill = prefill, onPrefillConsumed = { prefill = null }, onSend = ::send,
                ),
            )
            hint?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.5.sp, modifier = Modifier.padding(horizontal = 18.dp).testTag("botsLaunchHint")) }
        }
    }

    if (toPickerOpen) LibrarySheet(title = "Send to", testTag = "botsToSheet", onDismiss = { toPickerOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { toPickerOpen = false }) {
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            NavRow("$mainName · new project", null, if (selected == null) "✓" else null, "botsToTeam", last = bots.isEmpty()) { selectedBotId = null; toPickerOpen = false }
        }
        if (bots.isNotEmpty()) {
            SectionHeader("Send it to one bot")
            RowGroup {
                bots.sortedByDescending { it.isMainBot }.forEachIndexed { i, b ->
                    NavRow(b.displayName, null, if (selected?.id == b.id) "✓" else null, "botsTo${b.id}", last = i == bots.lastIndex) {
                        selectedBotId = if (b.isMainBot) null else b.id
                        toPickerOpen = false
                    }
                }
            }
        }
        RowGroup(Modifier.padding(vertical = 12.dp)) {
            NavRow("Add a Connected bot…", DesignR.drawable.ph_plugs_connected, null, "botsToVendor") { toPickerOpen = false; newBotOpen = true }
        }
    }
    if (newBotOpen) NewBotSheet(
        store = store,
        host = host,
        onBrowseTemplates = { newBotOpen = false; templatesOpen = true },
        onDismiss = { newBotOpen = false; newBotPicker = false },
        openPicker = newBotPicker,
        onCreated = { ids -> newBotOpen = false; newBotPicker = false; scope.launch { store.refreshAll(); ids.firstOrNull()?.let { nav.open(BotRoute.Chat(it, null)) } } },
    )
    if (templatesOpen) AgentTemplateSheet(onDismiss = { templatesOpen = false }) { t ->
        templatesOpen = false
        scope.launch {
            when (val r = BotsApi.createFromTemplate(t.id)) {
                is BotsResult.Ok -> store.refreshAgents()
                is BotsResult.Failed -> templateError = r.message
            }
        }
    }
    templateError?.let { msg ->
        LibrarySheet(title = "Couldn't create the bot", testTag = "botsTemplateError", onDismiss = { templateError = null }, trailing = LibrarySheetAction("OK", bold = true) { templateError = null }) {
            Text(msg, color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(20.dp))
        }
    }
}

/** iOS RosterFace: the face in a soft circle, a ring while working, a dot when it needs you, a star when pinned. */
@Composable
private fun RosterFace(e: RosterEntry, size: androidx.compose.ui.unit.Dp) {
    val c = AllternitTheme.colors
    Box(Modifier.size(size)) {
        Box(Modifier.size(size).clip(CircleShape).background(c.bgSecondary), contentAlignment = Alignment.Center) {
            when (e) {
                is RosterEntry.Bot -> BotAvatar(e.bot, size * 0.84f)
                is RosterEntry.Group -> Text(e.group.name.take(1).uppercase(), color = c.textPrimary, fontSize = (size.value * 0.4f).sp, fontWeight = FontWeight.SemiBold)
            }
        }
        if (e.status == RosterStatus.Working) Box(Modifier.size(size).border(2.dp, c.textPrimary.copy(alpha = 0.75f), CircleShape))
        if (e.status == RosterStatus.Waiting) Box(Modifier.align(Alignment.TopEnd)) { Dot(AllternitStatic.StatusWarning, size * 0.22f) }
        if ((e as? RosterEntry.Bot)?.pinned == true) Icon(painterResource(com.allternit.core.design.R.drawable.ph_star_fill), "Pinned", tint = AllternitStatic.StatusWarning, modifier = Modifier.align(Alignment.BottomStart).size(size * 0.24f))
    }
}

@Composable
private fun RosterTile(e: RosterEntry, selected: Boolean, onOpen: () -> Unit, onAt: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Column(Modifier.width(76.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Box(Modifier.clip(CircleShape).testTag("botsRoster${e.key}").clickable(role = Role.Button, onClickLabel = "Open ${e.name}, ${e.status.word}") { Haptics.perform(view, Haptics.Kind.Tap); onOpen() }.padding(4.dp)) {
            RosterFace(e, 52.dp)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                e.name, color = if ((e as? RosterEntry.Bot)?.main == true) c.textPrimary else c.textSecondary, fontSize = 12.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                textDecoration = if (selected) TextDecoration.Underline else null, modifier = Modifier.weight(1f, fill = false),
            )
            if (e is RosterEntry.Bot) AtButton(selected, e.name, onAt)
        }
        Text(e.status.word, color = e.status.tone(), fontSize = 11.sp)
    }
}

/** iOS AtButton: send the next message to this bot. */
@Composable
private fun AtButton(selected: Boolean, name: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.size(22.dp).clip(CircleShape).background(if (selected) c.textPrimary else Color.Transparent)
            .clickable(role = Role.Button, onClickLabel = if (selected) "Stop sending to $name" else "Send to $name") { onClick() }
            .semantics { contentDescription = if (selected) "Sending to $name" else "Send to $name" },
        contentAlignment = Alignment.Center,
    ) { Text("@", color = if (selected) c.bgPrimary else c.textTertiary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
private fun NewBotTile(onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.width(76.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Box(
            Modifier.padding(4.dp).size(52.dp).clip(CircleShape).border(BorderStroke(1.dp, c.borderStrong), CircleShape).testTag("botsNewBot")
                .clickable(role = Role.Button, onClickLabel = "New bot", onClick = onClick),
            contentAlignment = Alignment.Center,
        ) { Icon(painterResource(DesignR.drawable.ph_plus), "New bot", tint = c.textSecondary, modifier = Modifier.size(20.dp)) }
        Text("New bot", color = c.textSecondary, fontSize = 12.5.sp)
    }
}

@Composable
private fun NewBotCard(onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().heightIn(min = 120.dp).clip(RoundedCornerShape(16.dp)).border(BorderStroke(1.dp, c.borderStrong), RoundedCornerShape(16.dp))
            .testTag("botsNewBotCard").clickable(role = Role.Button, onClick = onClick),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically),
    ) {
        Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
        Text("New bot", color = c.textSecondary, fontSize = 12.5.sp)
    }
}

/** iOS rosterCard: the grid and deck card with the latest line, the time and (deck) "Now: …". */
@Composable
private fun RosterCard(e: RosterEntry, store: BotModeStore, deck: Boolean, selected: Boolean, onOpen: () -> Unit, onAt: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val (line, whenText, task) = when (e) {
        is RosterEntry.Bot -> {
            val threads = store.threads(e.bot.id)
            val latest = threads.maxByOrNull { it.lastActivityAt }
            val live = threads.filter { it.group == "working" || it.group == "waiting" }.maxByOrNull { it.lastActivityAt }
            Triple(latest?.let { it.statusLine ?: it.displayTitle } ?: e.bot.tagline, latest?.let { botTimeShort(it.lastActivityAt) }.orEmpty(), live?.let { it.displayTitle.ifEmpty { it.statusLine.orEmpty() } })
        }
        is RosterEntry.Group -> Triple(e.group.lastLine ?: "${e.group.memberIds.size} bots", botTimeShort(e.group.updatedAt), "${e.group.memberIds.size} bots · ${e.group.memberNames.take(3).joinToString(", ")}")
    }
    val tint = if (deck) (e as? RosterEntry.Bot)?.bot?.accentHex?.let { hexColor(it) } else null
    Box(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp))
            .background(Brush.verticalGradient(listOf((tint ?: Color.Transparent).copy(alpha = 0.12f), Color.Transparent)))
            .background(c.bgPanel.copy(alpha = 0.6f))
            .border(BorderStroke(if (selected) 2.dp else 1.dp, if (selected) c.textPrimary else c.borderSubtle), RoundedCornerShape(16.dp))
            .testTag("botsCard${e.key}")
            .clickable(role = Role.Button, onClickLabel = "Open ${e.name}, ${e.status.word}") { Haptics.perform(view, Haptics.Kind.Tap); onOpen() }
            .padding(12.dp),
    ) {
        Column(Modifier.fillMaxWidth().heightIn(min = if (deck) 220.dp else 110.dp), horizontalAlignment = if (deck) Alignment.CenterHorizontally else Alignment.Start) {
            if (deck) {
                RosterFace(e, if (e is RosterEntry.Group) 64.dp else 72.dp)
                Gap(8.dp)
                Text(e.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                Text(e.status.word, color = e.status.tone(), fontSize = 12.sp)
                if (task != null) Text("Now: $task", color = c.textPrimary, fontSize = 12.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 8.dp))
            } else Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                RosterFace(e, 40.dp)
                Column(Modifier.weight(1f)) {
                    Text(e.name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(e.status.word, color = e.status.tone(), fontSize = 11.sp)
                }
                Spacer(Modifier.width(22.dp))
            }
            Text(line, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 6.dp))
            Spacer(Modifier.weight(1f, fill = false).height(6.dp))
            Text(whenText, color = c.textTertiary, fontSize = 11.sp)
        }
        if (e is RosterEntry.Bot) Box(Modifier.align(Alignment.TopEnd)) { AtButton(selected, e.name, onAt) }
    }
}

/** A pushed screen's frame: a back header, then the content (iOS navigation bar on pushed screens). */
@Composable
internal fun BotScreen(title: String, nav: BotNavigator, subtitle: String? = null, testTag: String, trailing: @Composable () -> Unit = {}, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag(testTag)) {
        LibraryHeader(title = title, subtitle = subtitle, onBack = nav::back, backTag = "botsBack", trailing = trailing)
        Box(Modifier.weight(1f).fillMaxWidth()) { content() }
    }
}

/** iOS BotRouteDestination: each route's screen; a bot that is gone says so. */
@Composable
private fun BotRouteDestination(route: BotRoute, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    @Composable
    fun withBot(id: String, content: @Composable (BotRecord) -> Unit) {
        val bot = store.bot(id)
        LaunchedEffect(id) { if (bot == null && store.agents.isEmpty()) store.refreshAgents() }
        when {
            bot != null -> content(bot)
            store.agentsLoading -> BotScreen("", nav, testTag = "botsLoading") { LibrarySpinnerBox() }
            else -> BotScreen("", nav, testTag = "botsNotFound") {
                FriendlyState(FriendlyStyle.Empty, "Bot not found", "This bot was removed or isn't shared with you.") {
                    Icon(painterResource(com.allternit.core.design.R.drawable.ph_question), null, tint = AllternitTheme.colors.accentPrimary, modifier = Modifier.size(28.dp))
                }
            }
        }
    }
    when (route) {
        is BotRoute.Chat -> withBot(route.botId) { BotChatSessionScreen(it, route.sessionId, host, store, nav, newThread = route.newThread) }
        is BotRoute.Page -> withBot(route.botId) { b -> BotScreen(b.displayName, nav, testTag = "botPageScreen") { BotPageView(b, store, nav, backend = host.backend) } }
        is BotRoute.Threads -> BotScreen(route.botId?.let { store.bot(it)?.displayName } ?: "Threads", nav, testTag = "botThreadsScreen") {
            BotThreadsOverview(store, route.botId) { nav.openThread(it) }
        }
        is BotRoute.Project -> BotProjectPage(route.project, host, store, nav)
        is BotRoute.ProjectThread -> BotProjectThreadPage(route.thread, store, nav)
        is BotRoute.Tasks -> withBot(route.botId) { b -> BotScreen("Tasks", nav, testTag = "botTasksScreen") { BotThreadTasks(store, b, null) { nav.openThread(it) } } }
        is BotRoute.Channels -> withBot(route.botId) { b -> BotScreen("Channels", nav, testTag = "botChannelsScreen") { BotChannelsView(b) } }
        is BotRoute.Computer -> withBot(route.botId) { b -> BotComputerScreen(b, nav) }
        is BotRoute.Stage -> withBot(route.botId) { BotLiveStageScreen(it, host, store, nav) }
        is BotRoute.Home -> withBot(route.botId) { BotHomeScreen(it, host, store, nav) }
        is BotRoute.Settings -> withBot(route.botId) { b -> BotScreen(b.name, nav, testTag = "agentDetailScreen") { AgentDetailView(b, store, nav) } }
        is BotRoute.BotHome -> withBot(route.botId) { BotHomeTabsScreen(it, route.tab, host, store, nav) }
        is BotRoute.Group -> GroupRoomScreen(route.groupId, host, store, nav)
        BotRoute.Groups -> GroupsListScreen(store, nav)
        BotRoute.Office -> host.officeFloor?.let { floor ->
            Column(Modifier.fillMaxSize().statusBarsPadding().testTag("officeFloorScreen")) {
                LibraryHeader(title = "Office", onBack = nav::back, backTag = "botsBack")
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    val kit = remember(host, store, nav) { BotsOfficeKit(host, store, nav) }
                    floor(store, host.backend, { id -> nav.open(BotRoute.Chat(id, null)) }, nav::back, kit)
                }
            }
        }
        BotRoute.Hub -> AgentHubScreen(host, store, nav)
        BotRoute.Marketplace -> MarketplaceScreen(nav)
        is BotRoute.Listing -> MarketplaceListingScreen(route.id, store, nav)
        is BotRoute.FileEditor -> withBot(route.botId) { WorkspaceFileEditorScreen(it, route.file, host.backend, nav) }
        BotRoute.Activity -> AgentActivityListScreen(host.backend, nav)
        is BotRoute.ActivityDetail -> AgentActivityDetailScreen(route.thread, host.backend, nav)
        BotRoute.Decisions -> DecisionsScreen(host, store, nav)
    }
}

@Composable
internal fun LibrarySpinnerBox() = com.allternit.feature.artifacts.LibrarySpinner("Loading")
