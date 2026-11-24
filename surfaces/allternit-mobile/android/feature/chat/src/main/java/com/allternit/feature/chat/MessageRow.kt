package com.allternit.feature.chat

import android.content.Context
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState

/** What a row can ask the screen to do (iOS MessageRow callbacks). */
internal class RowActions(
    val onChooseModel: () -> Unit,
    val onRetry: (String) -> Unit,
    val availableErrorActions: Set<ChatErrorAction> = emptySet(),
    val onErrorAction: (String, ChatErrorAction) -> Unit = { _, _ -> },
    val onRegenerate: () -> Unit,
    val onEdit: (MessageRecord) -> Unit,
    val onReplyPermission: (PermissionAsk, String, (String) -> Unit) -> Unit,
    val onAnswer: (QuestionAsk, List<List<String>>, (String) -> Unit) -> Unit,
    val onSkip: (QuestionAsk, (String) -> Unit) -> Unit,
    val onOpenSettings: () -> Unit,
    val onConnect: (String) -> Unit,
    /** "Share link…" (ThreadShareSheet); null before the chat has a session. */
    val onShareLink: (() -> Unit)? = null,
    /** Opens an artifact card on the artifact screens (wave D); null keeps the open-the-URL fallback. */
    val onOpenArtifact: ((ArtifactRecord) -> Unit)? = null,
)

/**
 * One feed row (iOS MessageRow): user bubbles trail at up to 80% width; assistant replies are
 * full width with no bubble. Order: thinking, tool calls (or the status line), the tool cards
 * (run_code files, lookup answer cards, connector suggestion or settings change), the reply,
 * the Edited chip, questions and permission asks, the error card, artifacts, then the action
 * bar and the perf line once the reply is done.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun MessageRow(
    message: MessageRecord,
    isLastAssistant: Boolean,
    isLastUser: Boolean,
    editedVersions: List<MessageBranch>,
    backend: ChatBackend,
    actions: RowActions,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val isUser = message.role == "user"
    BoxWithConstraints(Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
        val maxBubble = maxWidth * 0.8f
        Row(Modifier.fillMaxWidth()) {
            if (isUser) Spacer(Modifier.weight(1f))
            Column(
                modifier = if (isUser) Modifier.widthIn(max = maxBubble) else Modifier.fillMaxWidth(),
                horizontalAlignment = if (isUser) Alignment.End else Alignment.Start,
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                if (message.reasoning.isNotEmpty()) ThinkingBlock(message.reasoning, streaming = message.isStreaming && message.content.isEmpty())
                if (message.toolCalls.isNotEmpty()) {
                    ToolCallsRail(message.toolCalls, message.isStreaming, waiting = message.permissions.any { it.answered == null }, backend = backend)
                } else {
                    message.toolStatus?.let { ToolStatusLine(it) }
                }
                if (!isUser) {
                    message.toolCalls.forEach { call ->
                        val n = ToolDescriber.normalized(call.toolName)
                        if (call.toolName == "run_code" && (call.files != null || call.run != null)) RunCodeFilesView(call, backend)
                        if (n == "lookup") call.metadata?.obj("card")?.let { AnswerCard(it, call.output, backend) }
                        val suggestion = ConnectorSuggestions.parse(call)
                        if (suggestion != null) {
                            ConnectSuggestCard(suggestion, actions.onConnect)
                        } else {
                            SettingsChanges.parse(call).takeIf { it.isNotEmpty() }?.let { SettingsChangeCard(it, actions.onOpenSettings) }
                        }
                    }
                }

                // The bubble / reply.
                if (!(message.content.isEmpty() && message.error != null)) {
                    if (isUser) {
                        var menu by remember { mutableStateOf(false) }
                        Box {
                            Text(
                                ChatViewModel.strippedText(message.content),
                                color = c.textPrimary,
                                fontSize = 15.sp,
                                lineHeight = 21.sp,
                                modifier = Modifier
                                    .clip(RoundedCornerShape(14.dp))
                                    .background(c.fillNeutral)
                                    .combinedClickable(onClick = {}, onLongClick = { menu = true }, onLongClickLabel = "Message options")
                                    .padding(horizontal = 16.dp, vertical = 10.dp),
                            )
                            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.bgPrimary) {
                                if (isLastUser) DropdownMenuItem(
                                    text = { Text("Edit", color = c.textPrimary) },
                                    leadingIcon = { Icon(painterResource(com.allternit.core.design.R.drawable.ph_pencil_simple), null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) },
                                    onClick = {
                                        menu = false
                                        actions.onEdit(message)
                                    },
                                )
                                DropdownMenuItem(
                                    text = { Text("Copy Text", color = c.textPrimary) },
                                    leadingIcon = { Icon(painterResource(DesignR.drawable.ph_copy), null, tint = c.textPrimary, modifier = Modifier.size(16.dp)) },
                                    onClick = {
                                        menu = false
                                        copyToClipboard(context, message.content)
                                    },
                                )
                            }
                        }
                        if (message.attachmentNames.isNotEmpty()) {
                            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                message.attachmentNames.forEach { name ->
                                    Row(
                                        Modifier.clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(horizontal = 10.dp, vertical = 6.dp),
                                        verticalAlignment = Alignment.CenterVertically,
                                        horizontalArrangement = Arrangement.spacedBy(5.dp),
                                    ) {
                                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_file), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                                        Text(name, color = c.textPrimary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 160.dp))
                                    }
                                }
                            }
                        }
                    } else if (message.isStreaming && message.content.isEmpty() && message.reasoning.isEmpty() && message.toolCalls.isEmpty() && message.toolStatus == null) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            ThinkingOrb(size = ThinkingOrbSize.Inline, state = ThinkingOrbState.Breathing)
                            Text("Working…", color = c.textSecondary, fontSize = 13.sp)
                        }
                    } else if (message.content.isNotEmpty()) {
                        Column(Modifier.animateContentSize(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            splitAnswer(message.content, message.isStreaming).forEach { piece ->
                                when (piece) {
                                    is AnswerPiece.Text -> MarkdownText(piece.text)
                                    is AnswerPiece.Card -> AnswerCard(piece.card, null, backend)
                                    AnswerPiece.Writing -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                        CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                                        Text("Writing a draft…", color = c.textSecondary, fontSize = 13.sp)
                                    }
                                }
                            }
                            if (message.isStreaming) ThinkingOrb(size = ThinkingOrbSize.Inline, state = ThinkingOrbState.Composing, modifier = Modifier.height(20.dp))
                        }
                    }
                }

                if (isUser && editedVersions.isNotEmpty()) EditedChip(editedVersions)

                if (!isUser) {
                    message.questions.forEach { q ->
                        QuestionCard(q, onAnswer = { a, err -> actions.onAnswer(q, a, err) }, onSkip = { err -> actions.onSkip(q, err) })
                    }
                    message.permissions.forEach { p ->
                        PermissionAskCard(p) { reply, err -> actions.onReplyPermission(p, reply, err) }
                    }
                }
                message.error?.let {
                    ChatErrorCard(it, actions.onChooseModel, { actions.onRetry(message.id) }, actions.availableErrorActions) { a -> actions.onErrorAction(message.id, a) }
                }
                message.artifacts.forEach { a -> ArtifactChip(a, actions.onOpenArtifact) }
                if (!isUser && !message.isStreaming && message.error == null && message.role == "assistant") {
                    MessageActionBar(message, isLastAssistant, actions.onRegenerate, actions.onShareLink)
                    message.perfStats?.let { p ->
                        Text(
                            "${formatDuration(p.timeToFirstTokenMs)} to first token · ~${p.tokensPerSecond.toInt()} tok/s",
                            color = c.textSecondary.copy(alpha = 0.7f),
                            fontSize = 11.sp,
                            modifier = Modifier.padding(start = 8.dp),
                        )
                    }
                }
            }
        }
    }
}

/** Collapsed reasoning (iOS thinking block): "Thinking…" while it streams, else "Thinking". */
@Composable
private fun ThinkingBlock(text: String, streaming: Boolean) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Column(Modifier.animateContentSize(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            Modifier.clickable(role = Role.Button, onClickLabel = if (open) "Hide thinking" else "Show thinking") { open = !open },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_brain), null, tint = c.textSecondary, modifier = Modifier.size(13.dp))
            Text(if (streaming) "Thinking…" else "Thinking", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
            Icon(painterResource(if (open) com.allternit.core.design.R.drawable.ph_caret_up else com.allternit.core.design.R.drawable.ph_caret_down), null, tint = c.textSecondary, modifier = Modifier.size(9.dp))
        }
        if (open) {
            SelectionContainer {
                Text(
                    text,
                    color = c.textSecondary,
                    fontSize = 12.sp,
                    lineHeight = 17.sp,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.bgPrimary).padding(10.dp),
                )
            }
        }
    }
}

/**
 * A typed artifact card (iOS artifact rows). With [onOpen] it opens the artifact screens
 * (iOS ArtifactDetailsView, the V2 viewer for `art_` ids); without it, its URL when it has one.
 */
@Composable
private fun ArtifactChip(a: ArtifactRecord, onOpen: ((ArtifactRecord) -> Unit)?) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val url = a.url?.takeIf { it.startsWith("https://") || it.startsWith("http://") }
    val openable = onOpen != null || url != null
    Row(
        Modifier
            .width(250.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(c.fillNeutral)
            .testTag("chatArtifactCard${a.id}")
            .clickable(enabled = openable, role = Role.Button) { if (onOpen != null) onOpen(a) else url?.let { openUrl(context, it) } }
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(com.allternit.core.design.R.drawable.ph_file), null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
        Column(Modifier.weight(1f)) {
            Text(a.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(if (openable) "Click to view" else a.kind, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
        }
        if (openable) Icon(painterResource(DesignR.drawable.ph_arrow_square_out), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
    }
}

/** "Edited" under a user turn with earlier versions; opens the versions sheet (iOS EditedVersionsSheet). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun EditedChip(versions: List<MessageBranch>) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Text(
        "Edited",
        color = c.textSecondary,
        fontSize = 11.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier
            .clip(CircleShape)
            .background(c.fillNeutral)
            .clickable(role = Role.Button) { open = true }
            .semantics { contentDescription = "Edited. Show earlier versions" }
            .padding(horizontal = 8.dp, vertical = 3.dp),
    )
    if (open) {
        ModalBottomSheet(onDismissRequest = { open = false }, containerColor = c.bgPrimary) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Earlier versions", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                    Text("Done", color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button) { open = false }.padding(8.dp))
                }
                versions.forEachIndexed { index, v ->
                    Text(if (index == versions.lastIndex) "Original" else "Version ${versions.size - index}", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                    Text("You", color = c.textSecondary, fontSize = 11.sp)
                    Text(v.content, color = c.textPrimary, fontSize = 15.sp)
                }
                Spacer(Modifier.height(24.dp))
            }
        }
    }
}

/**
 * Under a finished reply (iOS MessageActionBar): Copy, Share, Read aloud, Good / Bad response
 * (local only: the backend has no feedback endpoint), and Retry on the last reply.
 */
@Composable
private fun MessageActionBar(message: MessageRecord, isLast: Boolean, onRegenerate: () -> Unit, onShareLink: (() -> Unit)?) {
    val context = LocalContext.current
    val view = LocalView.current
    var feedback by remember(message.id) { mutableStateOf(0) }
    val speaking = Speaker.speakingId == message.id
    Row(Modifier.padding(start = 8.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        fun tap(block: () -> Unit) {
            Haptics.perform(view, Haptics.Kind.Tap)
            block()
        }
        ActionIcon(DesignR.drawable.ph_copy, "Copy") { tap { copyToClipboard(context, message.content) } }
        // Share: with a saved chat, a menu of Share text / Share link… (iOS); otherwise the share sheet.
        var shareMenu by remember { mutableStateOf(false) }
        Box {
            ActionIcon(com.allternit.core.design.R.drawable.ph_export, "Share") {
                tap { if (onShareLink != null) shareMenu = true else shareText(context, message.content) }
            }
            DropdownMenu(expanded = shareMenu, onDismissRequest = { shareMenu = false }, containerColor = AllternitTheme.colors.bgPrimary) {
                DropdownMenuItem(text = { Text("Share text", color = AllternitTheme.colors.textPrimary) }, onClick = {
                    shareMenu = false
                    shareText(context, message.content)
                })
                DropdownMenuItem(text = { Text("Share link…", color = AllternitTheme.colors.textPrimary) }, onClick = {
                    shareMenu = false
                    onShareLink?.invoke()
                })
            }
        }
        ActionIcon(if (speaking) com.allternit.core.design.R.drawable.ph_stop_fill else com.allternit.core.design.R.drawable.ph_speaker_high, if (speaking) "Stop" else "Read aloud") {
            tap { Speaker.toggle(context, message.id, MarkdownPlain.from(message.content)) }
        }
        ActionIcon(com.allternit.core.design.R.drawable.ph_thumbs_up, "Good response", active = feedback == 1) { tap { feedback = if (feedback == 1) 0 else 1 } }
        ActionIcon(com.allternit.core.design.R.drawable.ph_thumbs_down, "Bad response", active = feedback == -1) { tap { feedback = if (feedback == -1) 0 else -1 } }
        if (isLast) ActionIcon(com.allternit.core.design.R.drawable.ph_arrow_clockwise, "Retry") { tap(onRegenerate) }
    }
}

@Composable
private fun ActionIcon(icon: Int, label: String, active: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.size(30.dp).clip(CircleShape).clickable(role = Role.Button, onClick = onClick).semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), null, tint = if (active) c.textPrimary else c.textSecondary, modifier = Modifier.size(15.dp))
    }
}

/** Markdown → speakable text (drops fences, markers and link targets). */
internal object MarkdownPlain {
    fun from(md: String): String = md
        .replace(Regex("```[\\s\\S]*?```"), " ")
        .replace(Regex("""\[([^\]]+)\]\([^)]+\)"""), "$1")
        .replace(Regex("""[*_`#>~]"""), "")
        .trim()
}

/** One shared TextToSpeech (iOS SpeechSpeaker on AVSpeechSynthesizer). */
internal object Speaker {
    var speakingId by mutableStateOf<String?>(null)
        private set
    private var tts: TextToSpeech? = null
    private var ready = false
    private var pending: Pair<String, String>? = null

    fun toggle(context: Context, id: String, text: String) {
        if (speakingId == id) {
            tts?.stop()
            speakingId = null
            return
        }
        speakingId = id
        val engine = tts
        if (engine != null && ready) {
            speak(engine, id, text)
            return
        }
        pending = id to text
        if (engine == null) {
            tts = TextToSpeech(context.applicationContext) { status ->
                ready = status == TextToSpeech.SUCCESS
                val t = tts
                val p = pending
                pending = null
                if (ready && t != null && p != null) speak(t, p.first, p.second) else speakingId = null
            }.also {
                it.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
                    override fun onStart(utteranceId: String?) {}
                    override fun onDone(utteranceId: String?) { if (speakingId == utteranceId) speakingId = null }
                    @Deprecated("Deprecated in Java")
                    override fun onError(utteranceId: String?) { if (speakingId == utteranceId) speakingId = null }
                })
            }
        }
    }

    private fun speak(engine: TextToSpeech, id: String, text: String) {
        engine.speak(text.take(TextToSpeech.getMaxSpeechInputLength() - 1), TextToSpeech.QUEUE_FLUSH, null, id)
    }
}
