package com.allternit.feature.chat

import android.content.Context
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.scrollBy
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.gen.AllternitApi
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient

/** What the chat asks its host (the shell) to do. */
class ChatCallbacks(
    /** Opens the model picker (error card "Choose a model", a `no_brain` send). */
    val onChooseModel: () -> Unit,
    /** The current model + effort, read at retry time. */
    val currentModel: () -> Pair<String?, String?>,
    /** Edit on the last user bubble: load its text into the composer. */
    val onEdit: (MessageRecord) -> Unit,
    /** "Open Settings" on a settings-change card. */
    val onOpenSettings: () -> Unit,
    /** The Allternit Cloud model "Use Allternit credits for this message" re-sends on; null hides the button (iOS creditsFallbackModelId). */
    val creditsModelId: () -> String? = { null },
    /** Manage Claude organization / Link again on a refusal card: Settings › Connections (falls back to Settings). */
    val onOpenConnections: () -> Unit = onOpenSettings,
    /** "Connect" on a connector suggestion (the shell's Connectors page). */
    val onConnectConnector: (String) -> Unit,
    /** "Share link…" from a reply's Share menu (the host opens ThreadShareSheet). */
    val onShareLink: () -> Unit = {},
    /** Tapping an artifact card (wave D: the shell opens ArtifactDetailsScreen / the V2 viewer). */
    val onOpenArtifact: ((ArtifactRecord) -> Unit)? = null,
)

/** Builds the chat's backend on the shell's shared OkHttp client and API. */
object ChatBackends {
    fun create(context: Context, okHttp: OkHttpClient? = null, api: AllternitApi? = null): ChatBackend {
        val client = okHttp ?: ApiClients.okHttp(bearer = AllternitAuth.tokenProvider)
        val config = ApiConfig()
        return ChatBackend(client, api ?: AllternitApi(client, config), config, context.applicationContext)
    }
}

/**
 * The chat page body under the shell's header (iOS ChatContentView): the transient banner, the
 * feed (or the launch greeting while empty), the compute gate when a send found no computer,
 * the jump-to-latest button, the editing strip, then the composer the shell passes in.
 */
@Composable
fun ChatScreen(
    viewModel: ChatViewModel,
    backend: ChatBackend,
    callbacks: ChatCallbacks,
    editing: Boolean,
    onCancelEdit: () -> Unit,
    greeting: @Composable () -> Unit,
    composer: @Composable () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val list = rememberLazyListState()
    // iOS ChatView .scrollDismissesKeyboard(.interactively): dragging the feed hides the keyboard.
    val feedFocus = LocalFocusManager.current
    // Only a finger drag counts: auto-scroll while a reply streams must not hide the keyboard.
    LaunchedEffect(list) {
        list.interactionSource.interactions.collect { interaction ->
            if (interaction is androidx.compose.foundation.interaction.DragInteraction.Start) feedFocus.clearFocus()
        }
    }
    androidx.compose.runtime.DisposableEffect(viewModel) {
        viewModel.imageChatVisible = true
        onDispose { viewModel.imageChatVisible = false }
    }
    val messages = viewModel.messages

    val hasSession = viewModel.currentSessionId != null
    val actions = remember(viewModel, callbacks, hasSession) {
        RowActions(
            onChooseModel = callbacks.onChooseModel,
            onRetry = { id ->
                val (model, effort) = callbacks.currentModel()
                viewModel.retryFailed(id, model, effort)
            },
            availableErrorActions = buildSet {
                add(ChatErrorAction.ManageClaudeOrg)
                add(ChatErrorAction.LinkAgain)
                if (callbacks.creditsModelId() != null) add(ChatErrorAction.UseCredits)
            },
            onErrorAction = { id, action ->
                when (action) {
                    // Only on tap: re-send this one message on Allternit credits (iOS resendOnCloudModel).
                    ChatErrorAction.UseCredits -> callbacks.creditsModelId()?.let { cloud -> viewModel.retryFailed(id, cloud, callbacks.currentModel().second) }
                    // Settings › Connections, where the Claude organization row lives (iOS opens Connections too).
                    ChatErrorAction.LinkAgain, ChatErrorAction.ManageClaudeOrg -> callbacks.onOpenConnections()
                }
            },
            onRegenerate = {
                val (model, effort) = callbacks.currentModel()
                viewModel.regenerate(model, effort)
            },
            onEdit = callbacks.onEdit,
            onReplyPermission = { ask, reply, err -> viewModel.replyPermission(ask, reply, err) },
            onAnswer = { q, a, err -> viewModel.answerQuestion(q, a, err) },
            onSkip = { q, err -> viewModel.skipQuestion(q, err) },
            onOpenSettings = callbacks.onOpenSettings,
            onConnect = callbacks.onConnectConnector,
            onShareLink = if (hasSession) callbacks.onShareLink else null,
            onOpenArtifact = callbacks.onOpenArtifact,
            onOpenConnections = callbacks.onOpenConnections,
            onOpenSession = { sid -> viewModel.loadSession(sid) },
        )
    }

    // Chat wait, design pick B (iOS AnswerRoom): the sent message anchors at the top of the view and the answer grows
    // into the room kept below it; status stays in the running strip above the composer.
    var anchorId by remember { mutableStateOf<String?>(null) }
    var wasStreaming by remember { mutableStateOf(viewModel.isStreaming) }
    LaunchedEffect(viewModel.isStreaming) {
        AnswerRoom.anchorId(wasStreaming, viewModel.isStreaming, viewModel.messages)?.let { anchorId = it }
        if (!viewModel.isStreaming) anchorId = null
        wasStreaming = viewModel.isStreaming
    }
    val anchorIndex = anchorId?.let { id -> messages.indexOfFirst { it.id == id } } ?: -1
    LaunchedEffect(anchorId) {
        if (anchorIndex >= 0) list.scrollToItem(anchorIndex)
    }
    val spacingPx = with(androidx.compose.ui.platform.LocalDensity.current) { 20.dp.toPx() }
    val roomPx by remember(anchorIndex) {
        derivedStateOf {
            if (anchorIndex < 0) 0f else {
                val info = list.layoutInfo
                val turn = info.visibleItemsInfo.filter { it.index >= anchorIndex && it.key != ANSWER_ROOM_KEY }
                val reachesEnd = turn.lastOrNull()?.index == viewModel.messages.lastIndex
                if (!reachesEnd) 0f else AnswerRoom.room(
                    viewport = (info.viewportEndOffset - info.viewportStartOffset).toFloat(),
                    turnHeight = turn.sumOf { it.size }.toFloat(), topGap = 0f, rowSpacing = spacingPx,
                )
            }
        }
    }

    LaunchedEffect(viewModel.needsBrainPick) {
        if (viewModel.needsBrainPick) {
            viewModel.needsBrainPick = false
            callbacks.onChooseModel()
        }
    }

    // Follow the reply: new rows and streamed text keep the end in view (iOS onChange(messages.last)).
    val last = messages.lastOrNull()
    // iOS ChatView: a reply refused for Cloud credits (402 credits_required / credits_exhausted) opens Cloud credits,
    // once per reply. Packs and plans are bought there through Google Play when the server has Google on.
    val creditsReason = last?.error?.let { e -> com.allternit.feature.billing.CloudCreditsCopy.reason(listOfNotNull(e.name, e.message, e.raw).joinToString(" ")) }
    var creditsShownFor by remember { mutableStateOf<Any?>(null) }
    if (creditsReason != null && last != null && creditsShownFor != last.id) {
        com.allternit.feature.billing.CloudCreditsSheet(com.allternit.feature.billing.CloudCreditsRequest(creditsReason)) { creditsShownFor = last.id }
    }
    LaunchedEffect(messages.size, last?.content?.length, last?.toolCalls?.size, viewModel.computeGate) {
        val count = list.layoutInfo.totalItemsCount
        // An anchored turn that is still running must not drag the view to the bottom (jump button instead).
        if (count > 0 && AnswerRoom.followsBottom(anchorIndex >= 0, viewModel.isStreaming)) {
            if (viewModel.isStreaming) {
                list.scrollToItem(count - 1)
                list.scrollBy(100_000f)
            } else {
                list.animateScrollToItem(count - 1)
            }
        }
    }
    val far by remember {
        derivedStateOf {
            val info = list.layoutInfo
            val lastVisible = info.visibleItemsInfo.lastOrNull()?.index ?: 0
            info.totalItemsCount > 0 && lastVisible < info.totalItemsCount - 2
        }
    }

    Column(modifier.fillMaxSize()) {
        viewModel.transientError?.let { TransientBanner(it) { viewModel.transientError = null } }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val gate = viewModel.computeGate
            if (messages.isEmpty() && gate == null) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    if (viewModel.isLoadingHistory) {
                        CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    } else {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            greeting()
                            NotificationsCard()
                        }
                    }
                }
            } else {
                LazyColumn(
                    state = list,
                    modifier = Modifier.fillMaxSize().testTag("chatFeed"),
                    contentPadding = PaddingValues(vertical = 16.dp),
                    verticalArrangement = Arrangement.spacedBy(20.dp),
                ) {
                    items(messages, key = { it.id }) { m ->
                        val summary = m.voiceSummary
                        if (summary != null) {
                            com.allternit.feature.chat.voice.VoiceSummaryCard(summary)
                        } else androidx.compose.runtime.CompositionLocalProvider(LocalRunTreeSession provides viewModel.currentSessionId) {
                        MessageRow(
                            message = m,
                            isLastAssistant = m.id == viewModel.lastAssistantMessageId,
                            isLastUser = m.id == viewModel.lastUserMessageId,
                            editedVersions = viewModel.editedVersions[m.id].orEmpty(),
                            backend = backend,
                            actions = actions,
                        )
                        }
                    }
                    if (anchorIndex >= 0) {
                        item(key = ANSWER_ROOM_KEY) { Spacer(Modifier.height(with(androidx.compose.ui.platform.LocalDensity.current) { roomPx.toDp() })) }
                    }
                    if (gate != null) {
                        item(key = "computeGate") {
                            ComputeGateCard(
                                reason = gate,
                                backend = backend,
                                onConnected = { viewModel.computeGate = null },
                                onDismiss = { viewModel.computeGate = null },
                            )
                        }
                    }
                }
            }
            // Inside a Box nested in a Column: the plain overload, not ColumnScope's.
            androidx.compose.animation.AnimatedVisibility(
                visible = far && messages.isNotEmpty(),
                enter = fadeIn() + scaleIn(initialScale = 0.85f),
                exit = fadeOut() + scaleOut(targetScale = 0.85f),
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 12.dp),
            ) {
                Box(
                    Modifier
                        .size(40.dp)
                        .shadow(4.dp, CircleShape)
                        .clip(CircleShape)
                        .background(c.bgPanel)
                        .border(1.dp, c.borderSubtle, CircleShape)
                        .testTag("chatJumpToBottom")
                        .clickable(role = Role.Button) {
                            Haptics.perform(view, Haptics.Kind.Tap)
                            scope.launch {
                                val count = list.layoutInfo.totalItemsCount
                                if (count > 0) list.animateScrollToItem(count - 1)
                            }
                        }
                        .semantics { contentDescription = "Scroll to latest message" },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(painterResource(DesignR.drawable.ph_arrow_down), null, tint = c.textPrimary, modifier = Modifier.size(15.dp))
                }
            }
        }
        if (editing) {
            Row(
                Modifier.fillMaxWidth().background(c.bgPrimary).padding(start = 16.dp, end = 8.dp).testTag("chatEditingBanner"),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_pencil_simple), null, tint = c.textSecondary, modifier = Modifier.size(13.dp))
                Text("Editing message — sending replaces everything after it", color = c.textSecondary, fontSize = 13.sp, maxLines = 1, modifier = Modifier.weight(1f))
                Box(
                    Modifier.size(32.dp).clickable(role = Role.Button, onClick = onCancelEdit).semantics { contentDescription = "Cancel edit" },
                    contentAlignment = Alignment.Center,
                ) { Icon(painterResource(DesignR.drawable.ph_x), null, tint = c.textSecondary, modifier = Modifier.size(12.dp)) }
            }
        }
        // Running generations: one chip each; tap opens the full view, press and hold peeks.
        RunningStrip()
        BrowserImageControls(viewModel)
        composer()
    }
}

private const val ANSWER_ROOM_KEY = "answerRoom"
