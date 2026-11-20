package com.allternit.feature.bots

import com.allternit.core.design.AllternitSerif
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
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
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.chat.ChatAttachmentInput
import com.allternit.feature.chat.ChatCallbacks
import com.allternit.feature.chat.ChatScreen
import com.allternit.feature.chat.ChatSendRequest
import com.allternit.feature.chat.ChatViewModel
import com.allternit.feature.chat.SessionContext
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** One chat per bot screen (iOS @StateObject ChatViewModel), on the bot's agent context. */
@Composable
internal fun rememberBotChat(host: BotsHost, bot: BotRecord): ChatViewModel {
    val context = LocalContext.current
    val vm = remember(bot.id) {
        ChatViewModel(host.backend, context.contentResolver).also {
            it.sessionContext = SessionContext(originSurface = "chat", sessionMode = "agent", agentId = bot.id, agentName = bot.displayName)
        }
    }
    DisposableEffect(vm) { onDispose { vm.dispose() } }
    return vm
}

@Composable
private fun botCallbacks(host: BotsHost, vm: ChatViewModel, onPickModel: () -> Unit, onEdit: (String) -> Unit) = remember(vm) {
    ChatCallbacks(
        onChooseModel = onPickModel,
        currentModel = host.currentModel,
        onEdit = { m -> onEdit(ChatViewModel.strippedText(m.content)) },
        onOpenSettings = host.openConnections,
        onConnectConnector = { host.openConnections() },
        onOpenArtifact = host.openArtifact,
    )
}

private enum class DeckTab { Thread, Threads, Tasks, Bot }

/**
 * A bot chat (iOS BotChatSessionView): the thread top (back, the bot chip with its live line,
 * ⋯), the bot's StageBackdrop, then one of four decks: the thread (the chat), Threads, Tasks or
 * the bot page. A nil session opens the bot's main chat; a send docks the stage.
 */
@Composable
internal fun BotChatSessionScreen(bot: BotRecord, explicitSessionId: String?, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val vm = rememberBotChat(host, bot)
    var sessionId by remember(bot.id, explicitSessionId) { mutableStateOf(explicitSessionId) }
    var resolveError by remember { mutableStateOf<String?>(null) }
    var deck by remember { mutableStateOf(DeckTab.Thread) }
    var docked by remember { mutableStateOf(false) }
    var menuOpen by remember { mutableStateOf(false) }
    var editOpen by remember { mutableStateOf(false) }
    var deleteOpen by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var acting by remember { mutableStateOf(false) }
    var prefill by remember { mutableStateOf<String?>(null) }
    var editingId by remember { mutableStateOf<String?>(null) }
    var pickerRequest by remember { mutableStateOf(0) }
    val isMainChat = explicitSessionId == null

    suspend fun resolve() {
        resolveError = null
        if (explicitSessionId != null) { sessionId = explicitSessionId; return }
        when (val r = store.mainChatSessionId(bot)) {
            is BotsResult.Ok -> sessionId = r.value
            is BotsResult.Failed -> resolveError = r.message
        }
    }
    LaunchedEffect(bot.id, explicitSessionId) { resolve() }
    LaunchedEffect(sessionId) { sessionId?.let { vm.loadSession(it); docked = false } }
    LaunchedEffect(vm.isStreaming) { if (vm.isStreaming) docked = true }
    LaunchedEffect(vm.transientError) {
        // A main chat that no longer loads: forget the pin so the next open makes a new one (iOS).
        if (isMainChat && vm.transientError?.startsWith("Couldn't load the conversation") == true) store.forgetMainChat(bot.id)
    }

    val thread = sessionId?.let { sid -> store.threads.firstOrNull { it.currentSessionId == sid } }
    val waitingHere = thread?.group == "waiting"
    val live: Triple<String, Boolean, Boolean> = when {
        waitingHere -> Triple(thread?.statusLine ?: "Needs you", true, true)
        thread?.group == "working" -> Triple(thread?.statusLine ?: "Working", false, true)
        vm.isStreaming -> Triple("Working", false, true)
        else -> Triple("Ready", false, false)
    }
    val onStage = deck == DeckTab.Thread && !docked && sessionId != null
    val placeholder = when (deck) {
        DeckTab.Threads -> "Start a new thread…"
        DeckTab.Tasks -> "Ask for a task in this thread…"
        else -> if (thread != null && !isMainChat) "Steer this thread…" else "Message ${bot.displayName}…"
    }

    fun act(t: BotThread, action: String) {
        acting = true
        scope.launch {
            if (store.act(t, action) is BotsResult.Failed) actionError = when (action) {
                "approve" -> "Couldn't start it. Try again."
                "decline" -> "Couldn't close the suggestion. Try again."
                else -> "Couldn't resolve the thread. Try again."
            }
            acting = false
        }
    }
    fun open(t: BotThread) {
        val id = t.currentSessionId ?: return nav.openThread(t)
        if (id == sessionId) deck = DeckTab.Thread else nav.open(BotRoute.Chat(bot.id, id))
    }
    fun startThread(text: String) {
        scope.launch {
            val id = store.newThreadSession(bot, BotModeStore.projectTitle(text))
            if (id is BotsResult.Ok && BotsApi.sendUserMessage(id.value, text) is BotsResult.Ok) {
                nav.open(BotRoute.Chat(bot.id, id.value))
                store.refreshThreads()
            } else { actionError = "${bot.displayName} couldn't start that. Try again."; prefill = text }
        }
    }

    val deckRow: @Composable () -> Unit = {
        val waiting = store.waitingCount(bot.id)
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
            DeckButton(if (isMainChat) "Main chat" else thread?.displayTitle?.takeIf { it.isNotEmpty() } ?: "Thread", deck == DeckTab.Thread, "botDeckThread") { deck = DeckTab.Thread }
            DeckButton("Threads", deck == DeckTab.Threads, "botDeckThreads", dot = waiting > 0) { deck = DeckTab.Threads }
            DeckButton("Tasks", deck == DeckTab.Tasks, "botDeckTasks", meta = thread?.progressLabel) { deck = DeckTab.Tasks }
            DeckButton("Bot", deck == DeckTab.Bot, "botDeckBot") { deck = DeckTab.Bot }
        }
    }
    val composer: @Composable (onSend: (ChatSendRequest) -> Unit) -> Unit = { send ->
        Column {
            deckRow()
            host.composer(BotComposerSpec(
                placeholder = placeholder, isStreaming = vm.isStreaming && deck == DeckTab.Thread, sendDisabled = vm.isCreatingSession,
                prefill = prefill, onPrefillConsumed = { prefill = null }, onSend = send, onStop = vm::stopStreaming, pickerRequest = pickerRequest,
            ))
        }
    }

    Box(Modifier.fillMaxSize().testTag("botChatScreen")) {
        host.backdrop(bot.id, bot.accentHex)
        Column(Modifier.fillMaxSize().statusBarsPadding()) {
            // The thread top (iOS threadTop): back, the bot chip, ⋯.
            Row(Modifier.fillMaxWidth().height(52.dp).padding(horizontal = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                LibraryIconButton(DesignR.drawable.ph_caret_left, "Back", "botsBack", onClick = nav::back)
                Spacer(Modifier.weight(1f))
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.clip(CircleShape).clickable(enabled = !onStage, onClickLabel = "Show ${bot.displayName}") { deck = DeckTab.Thread; docked = false }.testTag("botChipFace")) {
                        BotStatusAvatar(bot, 28.dp, working = live.third && !live.second, waiting = live.second)
                    }
                    Column(Modifier.clickable(onClickLabel = "Open the bot page") { deck = DeckTab.Bot }.testTag("botChip")) {
                        com.allternit.core.design.BotNameText(bot.displayName, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            if (live.third) Dot(if (live.second) AllternitStatic.StatusWarning else AllternitStatic.StatusSuccess, 6.dp)
                            Text(live.first, color = c.textSecondary, fontSize = 11.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp))
                        }
                    }
                }
                Spacer(Modifier.weight(1f))
                LibraryIconButton(com.allternit.core.design.R.drawable.ph_dots_three, "Thread options", "botThreadMenu", enabled = !acting) { menuOpen = true }
            }
            Box(Modifier.weight(1f).fillMaxWidth()) {
                when (deck) {
                    DeckTab.Thread -> {
                        val sid = sessionId
                        when {
                            sid != null -> Column(Modifier.fillMaxSize()) {
                                if (onStage) BotChatStage(
                                    bot, live.first.takeIf { live.third }, working = live.third && !live.second,
                                    needs = store.threads(bot.id).filter { it.group == "waiting" && it.currentSessionId != sid },
                                    suggestions = store.threads(bot.id).filter { it.status == "proposed" },
                                    onOpen = ::open, onApprove = { act(it, "approve") }, onDecline = { act(it, "decline") },
                                    onDock = { docked = true },
                                )
                                ChatScreen(
                                    viewModel = vm, backend = host.backend,
                                    callbacks = botCallbacks(host, vm, onPickModel = { pickerRequest += 1 }, onEdit = { t -> editingId = vm.lastUserMessageId; prefill = t }),
                                    editing = editingId != null, onCancelEdit = { editingId = null; prefill = "" },
                                    greeting = { BotGreeting(bot) { prefill = it } },
                                    composer = {
                                        composer { req ->
                                            val e = editingId
                                            if (e != null) { editingId = null; vm.resendEdited(e, req) } else vm.send(req)
                                        }
                                    },
                                    modifier = Modifier.weight(1f),
                                )
                            }
                            resolveError != null -> FriendlyState(FriendlyStyle.Error, "Couldn't open ${bot.displayName}", resolveError ?: "", actionTitle = "Try again", onAction = { scope.launch { resolve() } }) {
                                Icon(painterResource(com.allternit.core.design.R.drawable.ph_chat_circle), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp))
                            }
                            else -> Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically)) {
                                BotAvatar(bot, 72.dp, live = true)
                                CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = c.textSecondary)
                                Text("Opening ${bot.displayName}…", color = c.textSecondary, fontSize = 14.sp)
                            }
                        }
                    }
                    DeckTab.Threads -> Column(Modifier.fillMaxSize()) {
                        BotThreadsOverview(store, bot.id, modifier = Modifier.weight(1f)) { open(it) }
                        composer { startThread(it.text) }
                    }
                    DeckTab.Tasks -> Column(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f)) { BotThreadTasks(store, bot, sessionId) { open(it) } }
                        composer { req -> deck = DeckTab.Thread; vm.send(req) }
                    }
                    DeckTab.Bot -> Column(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f)) { BotPageView(bot, store, nav, onOpenThreads = { deck = DeckTab.Threads }, onOpenTasks = { deck = DeckTab.Tasks }, backend = host.backend) }
                        deckRow()
                    }
                }
            }
        }
    }

    if (menuOpen) LibrarySheet(title = bot.displayName, testTag = "botThreadMenuSheet", onDismiss = { menuOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { menuOpen = false }) {
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            NavRow("Computer", com.allternit.core.design.R.drawable.ph_desktop, null, "botMenuComputer", last = false) { menuOpen = false; nav.open(BotRoute.Computer(bot.id)) }
            NavRow("Live stage", com.allternit.core.design.R.drawable.ph_tray, null, "botMenuStage", last = false) { menuOpen = false; nav.open(BotRoute.Stage(bot.id)) }
            NavRow("Switch thread", com.allternit.core.design.R.drawable.ph_list, null, "botMenuThreads", last = false) { menuOpen = false; deck = DeckTab.Threads }
            NavRow("Show ${bot.displayName}", com.allternit.core.design.R.drawable.ph_eye, null, "botMenuShow", last = false) { menuOpen = false; deck = DeckTab.Thread; docked = false }
            thread?.let { t ->
                if (t.status == "proposed") {
                    NavRow("Go ahead", DesignR.drawable.ph_play_fill, null, "botMenuApprove", last = false) { menuOpen = false; act(t, "approve") }
                    NavRow("Not now", com.allternit.core.design.R.drawable.ph_x, null, "botMenuDecline", last = false) { menuOpen = false; act(t, "decline") }
                } else if (t.kind == "task" && t.group != "resolved") {
                    NavRow("Resolve", DesignR.drawable.ph_check_circle_fill, null, "botMenuResolve", last = false) { menuOpen = false; act(t, "resolve") }
                }
            }
            NavRow(if (store.isPinned(bot.id)) "Unpin" else "Pin", com.allternit.core.design.R.drawable.ph_star, null, "botMenuPin", last = false) { store.togglePin(bot.id); menuOpen = false }
            NavRow("Edit bot", com.allternit.core.design.R.drawable.ph_pencil_simple, null, "botMenuEdit", last = false) { menuOpen = false; editOpen = true }
            NavRow("Bot settings", com.allternit.core.design.R.drawable.ph_gear, null, "botMenuSettings", last = bot.isPrimaryTemplateBot) { menuOpen = false; nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Config)) }
            if (!bot.isPrimaryTemplateBot) NavRow("Delete bot", DesignR.drawable.ph_trash, null, "botMenuDelete", destructive = true) { menuOpen = false; deleteOpen = true }
        }
        Gap(12.dp)
    }
    if (editOpen) EditBotSheet(bot.id, onDismiss = { editOpen = false }) { editOpen = false; scope.launch { store.refreshAll() } }
    if (deleteOpen) ConfirmSheet(
        title = "Delete ${bot.displayName}?", message = "Its chats, routines and files go with it. This can't be undone.",
        action = "Delete bot", testTag = "botDeleteConfirm", onDismiss = { deleteOpen = false },
    ) {
        deleteOpen = false
        acting = true
        scope.launch {
            when (val r = BotsApi.deleteAgent(bot.id)) {
                is BotsResult.Ok -> { store.refreshAll(); nav.back() }
                is BotsResult.Failed -> actionError = r.message
            }
            acting = false
        }
    }
    actionError?.let { msg ->
        LibrarySheet(title = "Something went wrong", testTag = "botActionError", onDismiss = { actionError = null }, trailing = LibrarySheetAction("OK", bold = true) { actionError = null }) {
            Text(msg, color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(20.dp))
        }
    }
}

@Composable
private fun DeckButton(label: String, on: Boolean, testTag: String, meta: String? = null, dot: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.height(32.dp).clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral.copy(alpha = c.fillNeutral.alpha * 0.7f)).testTag(testTag)
            .clickable(role = Role.Tab) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }.padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(label, color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 150.dp))
        meta?.let { Text(it, color = if (on) c.bgPrimary.copy(alpha = 0.7f) else c.textTertiary, fontSize = 11.5.sp) }
        if (dot) Dot(AllternitStatic.StatusWarning, 6.dp)
    }
}

/** The empty chat's greeting: the bot's face, its welcome and starter prompts. */
@Composable
private fun BotGreeting(bot: BotRecord, onPrompt: (String) -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        BotAvatar(bot, 64.dp, live = true)
        Text(bot.botGreeting, color = c.textPrimary, fontSize = 20.sp, fontFamily = AllternitSerif, textAlign = TextAlign.Center)
        (bot.starterPrompts.ifEmpty { bot.suggestedPrompts }).take(4).forEach { p ->
            Text(
                p, color = c.textSecondary, fontSize = 13.5.sp,
                modifier = Modifier.clip(RoundedCornerShape(12.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).clickable { onPrompt(p) }.padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
    }
}

/**
 * The stage over a fresh thread (iOS BotChatStage): the big face, the live line, what else needs
 * you, the bot's suggestions with Go ahead / Not now, and a tap to dock it.
 */
@Composable
private fun BotChatStage(
    bot: BotRecord, statusLine: String?, working: Boolean, needs: List<BotThread>, suggestions: List<BotThread>,
    onOpen: (BotThread) -> Unit, onApprove: (BotThread) -> Unit, onDecline: (BotThread) -> Unit, onDock: () -> Unit,
) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().heightIn(max = 360.dp).padding(horizontal = 16.dp, vertical = 8.dp).testTag("botChatStage"), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.clip(CircleShape).clickable(onClickLabel = "Dock ${bot.displayName}", onClick = onDock)) { BotAvatar(bot, 96.dp, live = true, working = working) }
        statusLine?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 2, textAlign = TextAlign.Center) }
        needs.take(2).forEach { t ->
            StageRow(t.displayTitle, "Needs you", AllternitStatic.StatusWarning, "botStageNeed${t.id}") { onOpen(t) }
        }
        suggestions.take(2).forEach { t ->
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel.copy(alpha = 0.85f)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(t.displayTitle, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                t.summary?.let { Text(it, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 3) }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Pill("Not now", "botStageDecline${t.id}") { onDecline(t) }
                    Pill("Go ahead", "botStageApprove${t.id}", primary = true) { onApprove(t) }
                }
            }
        }
    }
}

@Composable
private fun StageRow(title: String, status: String, tone: Color, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel.copy(alpha = 0.85f)).testTag(testTag).clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Dot(tone, 7.dp)
        Text(title, color = c.textPrimary, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Text(status, color = tone, fontSize = 12.sp)
    }
}

/** A destructive confirm as a sheet (iOS confirmationDialog). */
@Composable
internal fun ConfirmSheet(title: String, message: String, action: String, testTag: String, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val c = AllternitTheme.colors
    LibrarySheet(title = title, testTag = testTag, onDismiss = onDismiss, leading = LibrarySheetAction("Cancel", onClick = onDismiss)) {
        Text(message, color = c.textSecondary, fontSize = 15.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 12.dp))
        Box(Modifier.padding(16.dp)) { com.allternit.feature.artifacts.LibraryRowButton(action, DesignR.drawable.ph_trash, "${testTag}Action", destructive = true, onClick = onConfirm) }
    }
}

/** A picked file for a hand-off (iOS StagedAttachment). */
private data class Staged(val name: String, val uri: Uri, val mediaType: String, val bytes: ByteArray?)

/**
 * The live stage (iOS BotLiveStageView): the bot's face and live line, "Hand it something" opens
 * the tray (files, photos, links, a note) and Plan first / Do it / Ask me first hands it over in
 * the main chat, which runs below.
 */
@Composable
internal fun BotLiveStageScreen(bot: BotRecord, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val vm = rememberBotChat(host, bot)
    var sessionId by remember { mutableStateOf<String?>(null) }
    var resolveError by remember { mutableStateOf<String?>(null) }
    var trayOpen by remember { mutableStateOf(false) }
    val files = remember { mutableStateListOf<Staged>() }
    val links = remember { mutableStateListOf<String>() }
    var linkDraft by remember { mutableStateOf("") }
    var note by remember { mutableStateOf("") }
    var pickError by remember { mutableStateOf<String?>(null) }

    suspend fun resolve() {
        resolveError = null
        when (val r = store.mainChatSessionId(bot)) {
            is BotsResult.Ok -> { sessionId = r.value; vm.loadSession(r.value) }
            is BotsResult.Failed -> resolveError = r.message
        }
    }
    LaunchedEffect(bot.id) { resolve() }

    fun stage(uris: List<Uri>, photo: Boolean) {
        uris.forEachIndexed { i, uri ->
            val cr = context.contentResolver
            val type = cr.getType(uri) ?: if (photo) "image/jpeg" else "application/octet-stream"
            val name = cr.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cur -> if (cur.moveToFirst()) cur.getString(0) else null }
                ?: if (photo) "photo-${i + 1}.${type.substringAfter('/')}" else "file-${i + 1}"
            val bytes = runCatching { cr.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            if (bytes == null) pickError = "Couldn't read $name." else files.add(Staged(name, uri, type, bytes))
        }
    }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { stage(it, false) }
    val pickPhotos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(6)) { stage(it, true) }

    fun addLinks() {
        val found = linkDraft.split(Regex("\\s+")).filter { it.lowercase().startsWith("http://") || it.lowercase().startsWith("https://") }
        if (found.isEmpty()) { pickError = "That doesn't look like a link (it should start with https://)."; return }
        pickError = null
        found.forEach { if (it !in links) links.add(it) }
        linkDraft = ""
    }
    fun handOff(choice: String) {
        val names = files.map { it.name } + links
        if (names.isEmpty()) return
        val what = if (names.size == 1) names[0] else "these ${names.size} items"
        var text = when (choice) {
            "plan" -> "Plan how you'll handle $what before doing anything. Lay out the steps and wait for my go-ahead."
            "ask" -> "Before you start on $what, ask me the questions you need answered."
            else -> "Work on $what."
        }
        if (links.isNotEmpty()) text += "\n\n" + links.joinToString("\n")
        note.trim().takeIf { it.isNotEmpty() }?.let { text += "\n\n$it" }
        vm.send(ChatSendRequest(text = text, attachments = files.map { ChatAttachmentInput(it.name, it.uri.toString(), if (it.mediaType.startsWith("image/")) "image" else "file", it.bytes, it.mediaType) }))
        files.clear(); links.clear(); note = ""; trayOpen = false
    }

    val thread = store.threads.firstOrNull { it.currentSessionId == sessionId }
    val line = thread?.statusLine ?: if (vm.isStreaming) "Working…" else "Ready when you are"
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("botLiveStage")) {
        LibraryHeader(title = bot.displayName, onBack = nav::back, backTag = "botsBack", trailing = {
            LibraryIconButton(com.allternit.core.design.R.drawable.ph_desktop, "Computer", "botStageComputer") { nav.open(BotRoute.Computer(bot.id)) }
        })
        Column(
            Modifier.fillMaxWidth().background(Brush.verticalGradient(listOf(c.accentPrimary.copy(alpha = 0.12f), Color.Transparent))).padding(vertical = 14.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            BotAvatar(bot, 96.dp, live = true, working = vm.isStreaming)
            Text(line, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(horizontal = 24.dp))
            Pill(if (trayOpen) "Close" else "Hand it something", "botStageTray", icon = if (trayOpen) com.allternit.core.design.R.drawable.ph_x else com.allternit.core.design.R.drawable.ph_tray) { trayOpen = !trayOpen }
        }
        if (trayOpen) Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Pill("Files", "botStageFiles", icon = com.allternit.core.design.R.drawable.ph_file_plus) { pickFiles.launch(arrayOf("*/*")) }
                Pill("Photos", "botStagePhotos", icon = com.allternit.core.design.R.drawable.ph_image) { pickPhotos.launch(androidx.activity.result.PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.weight(1f).height(40.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 12.dp), contentAlignment = Alignment.CenterStart) {
                    if (linkDraft.isEmpty()) Text("Paste a link", color = c.textTertiary, fontSize = 15.sp)
                    BasicTextField(linkDraft, { linkDraft = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("botStageLink"))
                }
                Pill("Add", "botStageLinkAdd", enabled = linkDraft.isNotEmpty(), onClick = ::addLinks)
            }
            if (files.isNotEmpty() || links.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    files.toList().forEach { f -> Chip(f.name) { files.remove(f) } }
                    links.toList().forEach { l -> Chip(l) { links.remove(l) } }
                }
                Box(Modifier.fillMaxWidth().heightIn(min = 40.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp)) {
                    if (note.isEmpty()) Text("Anything to add? (optional)", color = c.textTertiary, fontSize = 15.sp)
                    BasicTextField(note, { note = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("botStageNote"))
                }
                val ready = sessionId != null && !vm.isStreaming
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Pill("Plan first", "botStagePlan", enabled = ready) { handOff("plan") }
                    Pill("Do it", "botStageDo", primary = true, enabled = ready) { handOff("do") }
                    Pill("Ask me first", "botStageAsk", enabled = ready) { handOff("ask") }
                }
            }
            pickError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val sid = sessionId
            when {
                sid != null -> ChatScreen(
                    viewModel = vm, backend = host.backend,
                    callbacks = botCallbacks(host, vm, onPickModel = {}, onEdit = {}),
                    editing = false, onCancelEdit = {}, greeting = {},
                    composer = { host.composer(BotComposerSpec(placeholder = "Message ${bot.displayName}…", isStreaming = vm.isStreaming, onSend = vm::send, onStop = vm::stopStreaming)) },
                )
                resolveError != null -> FriendlyState(FriendlyStyle.Error, "Couldn't open the stage", resolveError ?: "", actionTitle = "Try again", onAction = { scope.launch { resolve() } }) {
                    Icon(painterResource(com.allternit.core.design.R.drawable.ph_chat_circle), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(28.dp))
                }
                else -> LibrarySpinnerBox()
            }
        }
    }
}

@Composable
private fun Chip(title: String, onRemove: () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.clip(CircleShape).background(c.fillNeutral).padding(start = 10.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, color = c.textPrimary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 160.dp))
        LibraryIconButton(com.allternit.core.design.R.drawable.ph_x, "Remove $title", iconSize = 12, onClick = onRemove)
    }
}
