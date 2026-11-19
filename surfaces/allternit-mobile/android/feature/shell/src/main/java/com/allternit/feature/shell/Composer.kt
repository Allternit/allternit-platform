package com.allternit.feature.shell

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.expandVertically
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.delay

/**
 * Something staged on the composer (+ sheet: camera, photos, files, GitHub link). Photos and
 * files carry a content [uri]; the camera carries its JPEG [bytes]; a GitHub link carries the
 * URL in [uri]. The chat reads and uploads them on send (iOS AttachmentStore → POST /uploads).
 */
internal data class ComposerAttachment(
    val name: String,
    val uri: String?,
    val kind: String,
    val bytes: ByteArray? = null,
    val mediaType: String? = null,
)

/** What a send hands to the chat (step 8 sends it; the stub shows it). */
internal data class ComposerDraft(
    val text: String = "",
    val attachments: List<ComposerAttachment> = emptyList(),
    val modelId: String? = null,
    val modelLabel: String? = null,
    /** The model's effort level when it takes one (iOS ModelStore.effortForSend). */
    val effort: String? = null,
    val agentMode: String? = null,
    /** The armed deck mode's id, sent as `metadata.agentModeId`. */
    val agentModeId: String? = null,
    val onPhone: Boolean = false,
    val webSearch: Boolean = false,
    val research: Boolean = false,
)

/** Launch-state placeholders (iOS ComposerPlaceholders, web useRotatingPlaceholder.ts). */
internal object ComposerPlaceholders {
    val chat = listOf(
        "Ask anything, or paste something to work through",
        "Draft the email you’ve been putting off",
        "Explain this contract clause in plain terms",
        "Turn these notes into a clear plan",
        "Compare two options and tell me which fits",
        "Summarize a long article or PDF",
        "Think a decision through out loud with me",
    )

    val agentNoMode = listOf(
        "Pick a mode below, or describe the outcome you want",
        "Type / for plugins and skills, @ for bots and files",
    )

    fun byMode(tile: DeckMode): List<String> = when (tile.id) {
        "research" -> listOf("Ask for an in-depth, cited research report", "Compare five competitors with sources", "What does the latest data say about…")
        "slides" -> listOf("Describe the deck. Audience, length, tone", "Turn a brief into a 10-slide pitch", "Make a workshop deck with exercises")
        "website" -> listOf("Describe the site you want built", "Landing page for a bakery in Austin", "Rebuild a site from its URL, cleaner")
        "docs" -> listOf("Describe the document you need", "Write a one-page proposal from these notes")
        "data" -> listOf("Paste data or describe the sheet you need", "Chart monthly revenue from this CSV")
        "swarms" -> listOf("Describe the outcome. A team of agents will split it up")
        "image" -> listOf("Describe the image. Subject, style, format")
        "video" -> listOf("Describe the clip. Length, shots, mood")
        else -> agentNoMode
    }
}

/** The deck's Agent mode list (iOS ChatDeckMode.chat), with the shared deck icons. */
internal data class DeckMode(val id: String, val label: String, val icon: Int, val color: Color?)

internal val DeckModes = listOf(
    DeckMode("swarms", "Agent Swarm", DesignR.drawable.deck_swarms, Color(0xFF14B8A6)),
    DeckMode("research", "Deep Research", DesignR.drawable.deck_research, Color(0xFF60A5FA)),
    DeckMode("website", "Build", DesignR.drawable.deck_build, null),
    DeckMode("docs", "Docs", DesignR.drawable.deck_docs, Color(0xFF3B82F6)),
    DeckMode("data", "Sheets", DesignR.drawable.deck_sheets, Color(0xFF4ADE80)),
    DeckMode("slides", "Slides", DesignR.drawable.deck_slides, Color(0xFFFBBF24)),
    DeckMode("image", "Image", DesignR.drawable.deck_image, null),
    DeckMode("video", "Video", DesignR.drawable.deck_video, Color(0xFFEC4899)),
    DeckMode("artifact", "Artifact", DesignR.drawable.deck_artifact, null),
)

/** The one-time swipe tip (iOS ComposerSwipeTipState). */
private object SwipeTip {
    const val TEXT = "Swipe the composer up for modes, plugins and artifacts"
    const val KEY = "allternit.nudge.composer-swipe"
}

/** Upward travel on the composer that opens the folded deck band (iOS deckSwipeDistance). */
private val DeckSwipeDistance = 24.dp

/**
 * The home composer (iOS ComposerView, phone, as in the 11-18 build): the once swipe tip or the
 * no-computer chip, the rounded card with a rotating placeholder, and the controls row
 * (+ · Pick a brain ▾ · mic · voice, or Send once there is a draft). The deck band (Agent mode ·
 * Plugins · Artifacts) is folded at rest and opens on focus, a draft, an armed mode, or a swipe up.
 */
@Composable
internal fun Composer(
    models: ModelStore,
    noComputer: Boolean,
    phoneArmed: Boolean,
    onClearPhone: () -> Unit,
    onSend: (ComposerDraft) -> Unit,
    onOpenSection: (RailRow) -> Unit,
    /** A reply is streaming: Stop replaces Send (iOS ComposerView sendButton). */
    isStreaming: Boolean = false,
    /** The first send's session create is in flight: no double send. */
    sendDisabled: Boolean = false,
    onStop: () -> Unit = {},
    /** Text to load into the field (a failed send's draft, or Edit on the last message). */
    prefill: String? = null,
    onPrefillConsumed: () -> Unit = {},
    /** Bumped by the chat to open the model picker (a `no_brain` send, "Choose a model"). */
    pickerRequest: Int = 0,
    /** Wave E: a fixed placeholder (iOS ComposerView placeholderOverride), e.g. "Message Ada…" in a bot chat. */
    placeholderOverride: String? = null,
    /** Wave H2: starts voice mode for this chat (iOS ComposerView onVoiceMode); null leaves the button disabled. */
    onVoiceMode: (() -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val density = LocalDensity.current
    val prefs = remember { context.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE) }

    var text by rememberSaveable { mutableStateOf("") }
    val attachments = remember { mutableStateListOf<ComposerAttachment>() }
    var focused by remember { mutableStateOf(false) }
    var swipedOpen by remember { mutableStateOf(false) }
    var tipSeen by remember { mutableStateOf(prefs.getBoolean(SwipeTip.KEY, false)) }
    var chipDismissed by rememberSaveable { mutableStateOf(false) }
    var agentMode by rememberSaveable { mutableStateOf<String?>(null) }
    var plusOpen by remember { mutableStateOf(false) }
    var pickerOpen by remember { mutableStateOf(false) }
    var placeholderIndex by remember { mutableIntStateOf(0) }
    var webSearch by remember { mutableStateOf(prefs.getBoolean("composer-web-search", false)) }
    var research by remember { mutableStateOf(prefs.getBoolean("composer-research", false)) }

    val hasDraft = text.isNotBlank() || attachments.isNotEmpty()
    val armedMode = DeckModes.firstOrNull { it.id == agentMode }
    val deckOpen = focused || hasDraft || swipedOpen || agentMode != null
    val chipVisible = noComputer && !chipDismissed
    val showsTip = !tipSeen && !deckOpen && !chipVisible

    fun markTipSeen() {
        tipSeen = true
        prefs.edit().putBoolean(SwipeTip.KEY, true).apply()
    }

    val placeholders = when {
        placeholderOverride != null -> listOf(placeholderOverride)
        agentMode == null -> ComposerPlaceholders.chat
        armedMode != null -> ComposerPlaceholders.byMode(armedMode)
        else -> ComposerPlaceholders.agentNoMode
    }
    LaunchedEffect(focused, text.isEmpty(), agentMode) {
        if (agentMode != null) placeholderIndex = 0
        // Rotate every 7s at rest; paused while focused or typing, so it never changes under the user.
        if (focused || text.isNotEmpty()) return@LaunchedEffect
        while (true) {
            delay(7_000)
            placeholderIndex += 1
        }
    }
    LaunchedEffect(focused) {
        // Blur with an empty draft folds a swiped-open band.
        if (!focused && !hasDraft) swipedOpen = false
    }

    LaunchedEffect(Unit) { models.load() }
    LaunchedEffect(prefill) {
        if (prefill != null) {
            text = prefill
            onPrefillConsumed()
        }
    }
    LaunchedEffect(pickerRequest) { if (pickerRequest > 0) pickerOpen = true }

    // Dictation availability: the phone has a speech recognition service.
    val dictationAvailable = remember { SpeechRecognizer.isRecognitionAvailable(context) }
    // Wave H2: in-field dictation (iOS DictationController on Android SpeechRecognizer), the first-use
    // onboarding sheet, then the RECORD_AUDIO system dialog.
    val dictation = remember { com.allternit.feature.chat.voice.Dictation(context) }
    var dictationBase by remember { mutableStateOf("") }
    var dictationOnboarding by remember { mutableStateOf(false) }
    androidx.compose.runtime.DisposableEffect(dictation) { onDispose { dictation.stop() } }
    LaunchedEffect(dictation) {
        androidx.compose.runtime.snapshotFlow { dictation.transcript }.collect { t -> if (t.isNotEmpty()) text = dictationBase + t }
    }
    LaunchedEffect(dictation) {
        androidx.compose.runtime.snapshotFlow { dictation.errorMessage }.collect { m ->
            if (m != null) android.widget.Toast.makeText(context, m, android.widget.Toast.LENGTH_LONG).show()
        }
    }
    fun startDictation() {
        com.allternit.feature.chat.voice.VoicePrefs.load(context)
        dictationBase = if (text.isEmpty() || text.endsWith(" ")) text else "$text "
        dictation.start()
    }
    val micPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startDictation() else dictation.errorMessage = "Microphone access is off. Enable it in Settings to dictate messages."
    }
    fun dictateWithPermission() {
        if (com.allternit.feature.chat.voice.hasMicPermission(context)) startDictation() else micPermission.launch(android.Manifest.permission.RECORD_AUDIO)
    }
    fun toggleDictation() {
        Haptics.perform(view, Haptics.Kind.Tap)
        when {
            dictation.isRecording -> dictation.stop()
            !com.allternit.feature.chat.voice.dictationOnboardingShown(context) -> dictationOnboarding = true
            else -> dictateWithPermission()
        }
    }
    // Voice mode: the microphone priming sheet first when the system dialog is still pending (iOS voiceModeTapped).
    val startVoice = com.allternit.feature.chat.voice.rememberPrimedPermission(com.allternit.feature.chat.voice.AppPermission.Microphone) { onVoiceMode?.invoke() }

    fun send() {
        dictation.stop()
        // A turn needs words (iOS sendMessage guards on the trimmed text); files ride along.
        if (text.isBlank() || sendDisabled || isStreaming) return
        // No brain picked: the picker opens instead of sending (iOS requiresModelPick).
        if (models.selectedId.isNullOrEmpty()) {
            pickerOpen = true
            return
        }
        Haptics.perform(view, Haptics.Kind.Tap)
        val draft = ComposerDraft(
            text = text.trim(),
            attachments = attachments.toList(),
            modelId = models.selectedId,
            modelLabel = models.selectedId?.let { models.chipLabel },
            effort = models.selected?.let { models.level(it) },
            agentMode = armedMode?.label,
            agentModeId = armedMode?.id,
            onPhone = phoneArmed,
            webSearch = webSearch,
            research = research,
        )
        text = ""
        attachments.clear()
        onSend(draft)
    }

    Column(
        modifier = Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .imePadding()
            .padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 10.dp),
    ) {
        if (phoneArmed) {
            Row(Modifier.padding(bottom = 6.dp)) { PhoneArmedChip(onClear = onClearPhone) }
        }
        AnimatedVisibility(visible = chipVisible, enter = fadeIn(), exit = fadeOut()) {
            NoticeCard(
                icon = R.drawable.shell_desktop,
                title = NoComputerCopy.TITLE,
                detail = NoComputerCopy.DETAIL,
                testTag = "composerComputerChip",
                dismissLabel = "Dismiss",
                onDismiss = { chipDismissed = true },
            )
        }
        AnimatedVisibility(visible = showsTip, enter = fadeIn(), exit = fadeOut()) {
            NoticeCard(
                icon = R.drawable.shell_caret_up,
                title = SwipeTip.TEXT,
                detail = null,
                testTag = "composerSwipeTip",
                dismissLabel = "Dismiss tip",
                onDismiss = ::markTipSeen,
            )
        }

        // The card.
        val cardShape = RoundedCornerShape(18.dp)
        val swipePx = with(density) { DeckSwipeDistance.toPx() }
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .shadow(2.dp, cardShape, ambientColor = Color.Black.copy(alpha = 0.04f), spotColor = Color.Black.copy(alpha = 0.04f))
                .clip(cardShape)
                .background(c.bgPanel)
                .border(BorderStroke(1.dp, if (focused) c.borderStrong else c.borderSubtle), cardShape)
                .pointerInput(Unit) {
                    var dy = 0f
                    detectVerticalDragGestures(
                        onDragStart = { dy = 0f },
                        onDragEnd = {
                            // Up by 24dp opens, down by 24dp closes (iOS nextSwipedOpen).
                            if (-dy >= swipePx && !swipedOpen) {
                                swipedOpen = true
                                markTipSeen()
                                Haptics.perform(view, Haptics.Kind.Tap)
                            } else if (dy >= swipePx && swipedOpen) {
                                swipedOpen = false
                            }
                        },
                    ) { _, amount -> dy += amount }
                }
                .testTag("composerCard"),
        ) {
            if (attachments.isNotEmpty()) {
                Row(
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    attachments.forEach { a -> AttachmentChip(a) { attachments.remove(a) } }
                }
            }
            Box(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 12.dp)) {
                if (text.isEmpty()) {
                    Crossfade(targetState = placeholders[placeholderIndex % placeholders.size], animationSpec = tween(250), label = "placeholder") { p ->
                        Text(p, color = c.textTertiary, fontSize = 17.sp, lineHeight = 22.sp)
                    }
                }
                BasicTextField(
                    value = text,
                    onValueChange = { text = it },
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 17.sp, lineHeight = 22.sp, fontFamily = ShellSans),
                    cursorBrush = SolidColor(c.textPrimary),
                    maxLines = 8,
                    modifier = Modifier
                        .fillMaxWidth()
                        .onFocusChanged { focused = it.isFocused }
                        .testTag("composerInput")
                        .semantics { contentDescription = "Message" },
                )
            }
            Box(Modifier.fillMaxWidth().padding(horizontal = 8.dp).height(1.dp).background(c.borderSubtle))
            // Controls row.
            Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                val plusTurn by animateFloatAsState(if (plusOpen) 45f else 0f, tween(180), label = "plusTurn")
                Box(
                    modifier = Modifier
                        .size(44.dp)
                        .clip(CircleShape)
                        .background(c.fillNeutral)
                        .testTag("composerPlus")
                        .clickable(role = Role.Button, onClickLabel = "Add to chat") {
                            Haptics.perform(view, Haptics.Kind.Tap)
                            plusOpen = true
                        }
                        .semantics { contentDescription = "Add to chat" },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(painterResource(R.drawable.shell_plus), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(22.dp).rotate(plusTurn))
                }
                Spacer(Modifier.weight(1f))
                ModelChip(models = models, open = pickerOpen) { pickerOpen = true }
                if (isStreaming) {
                    // Stop while a reply runs (iOS: square.fill on the neutral circle, label "Stop").
                    Box(
                        modifier = Modifier
                            .size(44.dp)
                            .clip(CircleShape)
                            .background(c.fillNeutral)
                            .border(1.dp, c.borderSubtle, CircleShape)
                            .testTag("composerStop")
                            .clickable(role = Role.Button, onClickLabel = "Stop") {
                                Haptics.perform(view, Haptics.Kind.Tap)
                                onStop()
                            }
                            .semantics { contentDescription = "Stop" },
                        contentAlignment = Alignment.Center,
                    ) {
                        Box(Modifier.size(12.dp).clip(RoundedCornerShape(2.dp)).background(c.textPrimary))
                    }
                } else if (hasDraft) {
                    Box(
                        modifier = Modifier
                            .size(44.dp)
                            .clip(CircleShape)
                            .background(if (sendDisabled || text.isBlank()) c.accentPrimary.copy(alpha = 0.4f) else c.accentPrimary)
                            .testTag("composerSend")
                            .clickable(role = Role.Button, onClickLabel = "Send message") { send() }
                            .semantics { contentDescription = "Send message" },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(painterResource(DesignR.drawable.ph_arrow_up), contentDescription = null, tint = c.onAccent, modifier = Modifier.size(20.dp))
                    }
                } else {
                    Row {
                        if (dictationAvailable) {
                            ControlGlyph(R.drawable.shell_mic, if (dictation.isRecording) "Stop dictation" else "Dictate", "composerMic") { toggleDictation() }
                        }
                        // iOS draws the voice button at 0.4, disabled, where voice mode has no chat to talk to.
                        ControlGlyph(R.drawable.shell_waveform, "Start voice mode", "composerVoice", enabled = onVoiceMode != null) {
                            Haptics.perform(view, Haptics.Kind.Tap)
                            startVoice()
                        }
                    }
                }
            }
        }

        AnimatedVisibility(visible = deckOpen, enter = expandVertically() + fadeIn(), exit = shrinkVertically() + fadeOut()) {
            DeckBand(
                armed = armedMode,
                onPickMode = { m ->
                    Haptics.perform(view, Haptics.Kind.Tap)
                    agentMode = m?.id
                },
                onPlugins = { onOpenSection(RailRow("plugins", "Plugins", DesignR.drawable.ph_puzzle_piece)) },
                onArtifacts = { onOpenSection(RailRow("artifacts-library", "Artifacts", R.drawable.shell_file_text)) },
            )
        }
    }

    if (dictationOnboarding) {
        com.allternit.feature.chat.voice.DictationOnboardingSheet(onDismiss = { dictationOnboarding = false }) {
            dictationOnboarding = false
            dictateWithPermission()
        }
    }

    if (plusOpen) {
        PlusSheet(
            noComputer = noComputer,
            webSearch = webSearch,
            research = research,
            onWebSearch = {
                webSearch = it
                prefs.edit().putBoolean("composer-web-search", it).apply()
            },
            onResearch = {
                research = it
                prefs.edit().putBoolean("composer-research", it).apply()
            },
            onAttach = { attachments += it },
            onOpenSection = { row ->
                plusOpen = false
                onOpenSection(row)
            },
            onDismiss = { plusOpen = false },
        )
    }
    if (pickerOpen) {
        ModelPickerSheet(models = models, onOpenSection = { row ->
            pickerOpen = false
            onOpenSection(row)
        }, onDismiss = { pickerOpen = false })
    }
}

/** A 44dp glyph button in the controls row (iOS micButton / voiceButton). */
@Composable
private fun ControlGlyph(icon: Int, label: String, testTag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier = Modifier
            .size(44.dp)
            .clip(CircleShape)
            .testTag(testTag)
            .clickable(enabled = enabled, role = Role.Button, onClickLabel = label, onClick = onClick)
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary.copy(alpha = if (enabled) 1f else 0.4f), modifier = Modifier.size(22.dp))
    }
}

/** Model chip: the matrix mark, the model's short name or "Pick a brain", and ▾ (iOS modelChip). */
@Composable
private fun ModelChip(models: ModelStore, open: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val turn by animateFloatAsState(if (open) 180f else 0f, tween(180), label = "chipTurn")
    val label = models.chipLabel
    Row(
        modifier = Modifier
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(10.dp))
            .testTag("composerModelChip")
            .clickable(role = Role.Button, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = "Model: $label" }
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        if (models.isLoading && models.chatModels.isEmpty()) {
            CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
            Text("Loading...", color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
        } else {
            Box(
                Modifier
                    .size(20.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .background(c.fillNeutral)
                    .border(1.dp, c.borderSubtle, RoundedCornerShape(6.dp)),
                contentAlignment = Alignment.Center,
            ) { MatrixLogo(size = 18.dp) }
            Text(label, color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 150.dp))
        }
        Icon(painterResource(R.drawable.shell_caret_down), contentDescription = null, tint = c.textSecondary.copy(alpha = 0.8f), modifier = Modifier.size(11.dp).rotate(turn))
    }
}

/** The grey card above the composer: the swipe tip, or the no-computer chip (one at a time). */
@Composable
private fun NoticeCard(icon: Int, title: String, detail: String?, testTag: String, dismissLabel: String, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 6.dp)
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(c.fillNeutral)
            .testTag(testTag)
            .padding(start = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        Column(Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(title, color = c.textPrimary, fontSize = 14.sp, lineHeight = 18.sp, maxLines = 2, fontWeight = if (detail != null) FontWeight.SemiBold else FontWeight.Normal)
            if (detail != null) Text(detail, color = c.textSecondary, fontSize = 13.sp, lineHeight = 17.sp)
        }
        Box(
            modifier = Modifier
                .size(44.dp)
                .clickable(role = Role.Button, onClickLabel = dismissLabel, onClick = onDismiss)
                .semantics { contentDescription = dismissLabel },
            contentAlignment = Alignment.Center,
        ) {
            Icon(painterResource(R.drawable.shell_x), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        }
    }
}

/** "On <phone>" with ✕ (iOS PhoneArmedChip): the next send runs on this phone. */
@Composable
private fun PhoneArmedChip(onClear: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .clip(CircleShape)
            .background(c.fillNeutral)
            .testTag("composerPhoneChip")
            .padding(start = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Icon(painterResource(R.drawable.shell_device_mobile), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(12.dp))
        Text("On this phone", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Box(Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Don't run on this phone", onClick = onClear), contentAlignment = Alignment.Center) {
            Icon(painterResource(R.drawable.shell_x), contentDescription = "Don't run on this phone", tint = c.textPrimary, modifier = Modifier.size(10.dp))
        }
    }
}

/** A staged attachment: name with ✕ (iOS attachmentThumb, as a chip). */
@Composable
internal fun AttachmentChip(a: ComposerAttachment, onRemove: (() -> Unit)?) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .padding(top = 4.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(c.fillNeutral)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(10.dp))
            .padding(start = 10.dp, end = if (onRemove == null) 10.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        val icon = when (a.kind) {
            "photo", "camera" -> R.drawable.shell_photo
            "github" -> R.drawable.shell_link
            else -> R.drawable.shell_file_text
        }
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        Text(a.name, color = c.textPrimary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp).padding(vertical = 8.dp))
        if (onRemove != null) {
            Box(Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Remove ${a.name}", onClick = onRemove), contentAlignment = Alignment.Center) {
                Icon(painterResource(R.drawable.shell_x), contentDescription = "Remove ${a.name}", tint = c.textSecondary, modifier = Modifier.size(10.dp))
            }
        }
    }
}

/**
 * The deck band under the card (iOS ComposerDeckBand): Agent mode ▾ | Plugins | Artifacts,
 * tucked 8dp under the card with rounded bottom corners.
 */
@Composable
private fun DeckBand(armed: DeckMode?, onPickMode: (DeckMode?) -> Unit, onPlugins: () -> Unit, onArtifacts: () -> Unit) {
    val c = AllternitTheme.colors
    var menu by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(bottomStart = 16.dp, bottomEnd = 16.dp)
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = 0.dp)
            .clip(shape)
            .background(c.bgPanel)
            .border(1.dp, c.borderSubtle, shape)
            .horizontalScroll(rememberScrollState())
            .heightIn(min = 52.dp)
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .testTag("composerDeck"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box {
            DeckTrigger(armed?.icon ?: DesignR.drawable.deck_auto, armed?.label ?: "Agent mode", "deckMode") { menu = true }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
                Text("Agent mode", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                DeckModes.forEach { m ->
                    MenuRow(icon = m.icon, title = m.label, checked = armed?.id == m.id, testTag = "deckMode${m.id}") {
                        menu = false
                        onPickMode(m)
                    }
                }
                if (armed != null) {
                    MenuRow(icon = null, title = "Turn off agent mode", testTag = "deckModeOff") {
                        menu = false
                        onPickMode(null)
                    }
                }
            }
        }
        DeckSeparator()
        DeckTrigger(DesignR.drawable.ph_puzzle_piece, "Plugins", "deckPlugins", onClick = onPlugins)
        DeckSeparator()
        DeckTrigger(R.drawable.shell_stack, "Artifacts", "deckArtifacts", chevron = false, onClick = onArtifacts)
    }
}

@Composable
private fun DeckSeparator() {
    Text("|", color = AllternitTheme.colors.textSecondary.copy(alpha = 0.4f), fontSize = 11.sp)
}

@Composable
private fun DeckTrigger(icon: Int, label: String, testTag: String, chevron: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .heightIn(min = 32.dp)
            .clip(RoundedCornerShape(8.dp))
            .testTag(testTag)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        Text(label, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Bold)
        if (chevron) Icon(painterResource(R.drawable.shell_caret_up), contentDescription = null, tint = c.textSecondary.copy(alpha = 0.6f), modifier = Modifier.size(9.dp))
    }
}

/** The display name of a content Uri (OpenableColumns), else its last path segment. */
internal fun displayName(context: Context, uri: Uri): String {
    runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0)?.takeIf { it.isNotBlank() }?.let { return it }
        }
    }
    return uri.lastPathSegment ?: "File"
}
