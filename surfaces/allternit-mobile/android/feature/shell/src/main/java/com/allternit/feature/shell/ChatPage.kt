package com.allternit.feature.shell

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatAttachmentInput
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.ChatCallbacks
import com.allternit.feature.chat.ChatScreen
import com.allternit.feature.chat.ChatSendRequest
import com.allternit.feature.chat.ChatViewModel
import com.allternit.feature.chat.ThreadShareSheet
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import com.allternit.core.design.GlassPill

/** A composer send → the chat's send request (iOS ChatView onSend: model, effort, files, tools). */
internal fun ComposerDraft.toChatRequest(): ChatSendRequest = ChatSendRequest(
    text = text,
    imagePrompt = if (agentModeId == "image" || agentModeId == "artifact") text.trim().take(16000).takeIf { it.isNotEmpty() } else null,
    personTurn = true,
    attachments = attachments.map { ChatAttachmentInput(it.name, it.uri, it.kind, it.bytes, it.mediaType) },
    runtimeModelId = modelId,
    effort = effort,
    agentModeId = agentModeId,
    webSearch = webSearch,
    research = research,
    skillId = skillId,
    skillArguments = skillArguments,
)

/**
 * The open chat (iOS ChatView): the header (glass ☰, the chat's name and the glass pill with ⋯), then [ChatScreen] with the shell's composer. Edit on the last user message loads its
 * text into the composer and the next send replaces everything after it.
 */
@Composable
internal fun ChatPage(
    chat: ChatViewModel,
    backend: ChatBackend,
    store: ShellSessionsStore,
    models: ModelStore,
    menuButton: @Composable () -> Unit,
    onNewChat: () -> Unit,
    onDeleted: () -> Unit,
    onOpenSection: (RailRow) -> Unit,
    onOpenSettings: () -> Unit,
    /** Settings › Connections (the Claude organization row), from a refusal card. */
    onOpenConnections: () -> Unit = onOpenSettings,
    /** Wave D: an artifact card opens the artifact screens. */
    onOpenArtifact: ((com.allternit.feature.chat.ArtifactRecord) -> Unit)? = null,
    /** Opened from the home composer's voice button: start voice mode once, then [onVoiceStarted]. */
    startVoice: Boolean = false,
    onVoiceStarted: () -> Unit = {},
    /** A Code thread: the waiting games' Play button shows from the start (iOS Code threads pass `anytime: true`). */
    playGamesAnytime: Boolean = false,
    /** Artifact mode + Send inside a chat starts a new artifact session, never converts this thread (D5). */
    onStartArtifact: ((DeckMode, ComposerDraft, Map<String, Boolean>) -> Unit)? = null,
    onPickPdf: (() -> Unit)? = null,
    onOpenArtifactId: ((String) -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    var editingId by remember { mutableStateOf<String?>(null) }
    var prefill by remember { mutableStateOf<String?>(null) }
    var pickerRequest by remember { mutableIntStateOf(0) }
    var menuOpen by remember { mutableStateOf(false) }
    var shareOpen by remember { mutableStateOf(false) }

    // A send that never reached the computer hands its text back (iOS draftToRestore).
    LaunchedEffect(chat.draftToRestore) {
        chat.draftToRestore?.let {
            prefill = it
            chat.draftToRestore = null
        }
    }

    val callbacks = remember(chat, models) {
        ChatCallbacks(
            onChooseModel = { pickerRequest += 1 },
            currentModel = { models.selectedId to models.selected?.let { models.level(it) } },
            onEdit = { m ->
                editingId = m.id
                prefill = ChatViewModel.strippedText(m.content)
            },
            onOpenSettings = onOpenSettings,
            onOpenConnections = onOpenConnections,
            creditsModelId = { models.cloudModels.let { cloud -> cloud.firstOrNull { it.id == models.selectedId } ?: cloud.firstOrNull() }?.id },
            onConnectConnector = { onOpenSection(RailRow("connectors", "Connectors", com.allternit.core.design.R.drawable.ph_lightning)) },
            onShareLink = { shareOpen = true },
            onOpenArtifact = onOpenArtifact,
        )
    }

    val title = store.sessions.firstOrNull { it.id == chat.currentSessionId }?.title.orEmpty()
    // Wave H2: voice mode (iOS ChatView voiceHost): the bar over the composer, expanding to full screen. Like iOS,
    // the composer's voice button (and the home's, which opens this chat) starts it as the bar.
    val voiceContext = androidx.compose.ui.platform.LocalContext.current
    val voiceHost = remember(chat) { com.allternit.feature.chat.voice.VoiceHost(voiceContext) }
    com.allternit.feature.chat.voice.VoiceHostWiring(voiceHost, chat)
    LaunchedEffect(startVoice) {
        if (startVoice) {
            voiceHost.start(chat, models.selectedId, models.selected?.let { models.level(it) })
            onVoiceStarted()
        }
    }
    val endVoice: (Int) -> Unit = { seconds -> chat.appendVoiceSummary(seconds) }
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("chatScreen")) {
        PhoneHeader(
            title = title,
            wideSides = true,
            leading = menuButton,
            trailing = {
                com.allternit.feature.chat.BrowserImageLibrary(chat)
                Box {
                    GlassPill(
                        onClick = { menuOpen = true },
                        modifier = Modifier.testTag("chatOptions").semantics { contentDescription = "Chat options" },
                        contentPadding = PaddingValues(horizontal = 12.dp),
                    ) {
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_dots_three), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
                    }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
                        if (chat.currentSessionId == null) {
                            Text("Send a message to start this chat.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 10.dp))
                        } else {
                            MenuRow(icon = com.allternit.core.design.R.drawable.ph_link, title = "Share link", testTag = "chatMenushareLink") {
                                menuOpen = false
                                shareOpen = true
                            }
                            MenuRow(icon = DesignR.drawable.ph_trash, title = "Delete chat", testTag = "chatMenuDelete") {
                                menuOpen = false
                                chat.deleteCurrentSession(onDeleted)
                            }
                        }
                    }
                }
            },
        )
        ChatScreen(
            viewModel = chat,
            backend = backend,
            callbacks = callbacks,
            editing = editingId != null,
            onCancelEdit = {
                editingId = null
                prefill = ""
            },
            // T7: no dock inside the chat thread; its empty state keeps the plain mark greeting.
            greeting = { LaunchGreeting(showMark = true) },
            composer = {
                com.allternit.feature.chat.voice.VoiceBarSlot(voiceHost, onEnd = endVoice)
                Composer(
                    watchSessionId = chat.currentSessionId,
                    models = models,
                    noComputer = store.noComputer,
                    phoneArmed = false,
                    onClearPhone = {},
                    onSend = { draft ->
                        val editing = editingId
                        if (editing != null) {
                            editingId = null
                            chat.resendEdited(editing, draft.toChatRequest())
                        } else {
                            chat.send(draft.toChatRequest())
                        }
                    },
                    onOpenSection = onOpenSection,
                    isStreaming = chat.isStreaming,
                    deck = ComposerDeck.Compact,
                    sendDisabled = chat.isCreatingSession || chat.hasBrowserImageJob,
                    onStop = chat::stopStreaming,
                    prefill = prefill,
                    onPrefillConsumed = { prefill = null },
                    pickerRequest = pickerRequest,
                    onVoiceMode = {
                        voiceHost.start(chat, models.selectedId, models.selected?.let { models.level(it) })
                    },
                    playGamesAnytime = playGamesAnytime,
                    sessionOpen = true,
                    onNewItem = onNewChat,
                    onStartArtifact = onStartArtifact,
                    onPickPdf = onPickPdf,
                    onOpenArtifactId = onOpenArtifactId,
                )
            },
            modifier = Modifier.weight(1f),
        )
    }
    com.allternit.feature.chat.voice.VoiceFullSlot(voiceHost, onEnd = endVoice)
    }
    if (shareOpen) {
        ThreadShareSheet(backend = backend, sessionId = chat.currentSessionId, title = title.ifEmpty { "Allternit" }, messages = chat.messages, onDismiss = { shareOpen = false })
    }
}
