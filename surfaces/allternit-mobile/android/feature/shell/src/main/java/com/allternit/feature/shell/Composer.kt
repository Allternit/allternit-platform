package com.allternit.feature.shell

import com.allternit.core.design.MatrixLogo
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.zIndex
import androidx.compose.foundation.Canvas
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.animateFloat
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalInputModeManager
import androidx.compose.ui.input.InputMode
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.customActions
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

/** Which deck rides under the card (iOS ComposerDeckVariant): Full on Home, Compact in a chat, None on Bots. */
internal enum class ComposerDeck { None, Compact, Full }

/** One-line rest pill, Runtime/Brain chips, and Tools behind an explicit sliders button. */
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
    /** iOS ComposerView `deck`: Full (Home, rotating hints), Compact (a chat, "Reply…"), None (Bots). */
    deck: ComposerDeck = ComposerDeck.Full,
    /** iOS ComposerView `playGamesAnytime`: Play shows from the start (Bots, Code threads), not only after 5 s of work. */
    playGamesAnytime: Boolean = false,
    /** A bot chat's Runtime · Brain · Bot chips (Harness & Bots 2.0), drawn where the model chip sits. */
    agentChips: (@Composable (iconOnly: Boolean) -> Unit)? = null,
    sessionOpen: Boolean = deck == ComposerDeck.Compact,
    onNewItem: (() -> Unit)? = null,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val prefs = remember { context.getSharedPreferences("allternit_shell", Context.MODE_PRIVATE) }

    var text by rememberSaveable { mutableStateOf("") }
    val attachments = remember { mutableStateListOf<ComposerAttachment>() }
    var focused by remember { mutableStateOf(false) }
    var deckFolded by remember { mutableStateOf(true) }
    var chipDismissed by rememberSaveable { mutableStateOf(false) }
    var agentMode by rememberSaveable { mutableStateOf<String?>(null) }
    var plusOpen by remember { mutableStateOf(false) }
    var pickerOpen by remember { mutableStateOf(false) }
    var placeholderIndex by remember { mutableIntStateOf(0) }
    var webSearch by remember { mutableStateOf(prefs.getBoolean("composer-web-search", false)) }
    var research by remember { mutableStateOf(prefs.getBoolean("composer-research", false)) }

    val hasDraft = text.isNotBlank() || attachments.isNotEmpty()
    val armedMode = DeckModes.firstOrNull { it.id == agentMode }
    // Tools opens only from the sliders button, regardless of typing/focus.
    val deckOpen = deck != ComposerDeck.None && !deckFolded
    val chipVisible = noComputer && !chipDismissed
    val canSend = hasDraft && !sendDisabled

    fun setDeckFolded(folded: Boolean) {
        Haptics.perform(view, Haptics.Kind.Tap)
        deckFolded = folded
    }

    val placeholders = when {
        placeholderOverride != null -> listOf(placeholderOverride)
        deck != ComposerDeck.Full -> listOf("Reply…")
        agentMode == null -> ComposerPlaceholders.chat
        armedMode != null -> ComposerPlaceholders.byMode(armedMode)
        else -> ComposerPlaceholders.agentNoMode
    }
    LaunchedEffect(focused, text.isEmpty(), agentMode) {
        if (agentMode != null) placeholderIndex = 0
        // Rotate every 7s at rest; paused while focused or typing, so it never changes under the user.
        if (deck != ComposerDeck.Full || focused || text.isNotEmpty()) return@LaunchedEffect
        while (true) {
            delay(7_000)
            placeholderIndex += 1
        }
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
    val inputMode = LocalInputModeManager.current
    val rest = !focused && !hasDraft && !isStreaming && !dictation.isRecording && !deckOpen
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
        AnimatedVisibility(
            visible = chipVisible,
            enter = fadeIn(),
            // Panel A: the card leaves on a spring (slides down and fades), not a bare fade.
            exit = androidx.compose.animation.slideOutVertically(com.allternit.core.design.Motion.standardSpring()) { it / 2 } + fadeOut(),
        ) {
            NoticeCard(
                icon = com.allternit.core.design.R.drawable.ph_desktop,
                title = NoComputerCopy.TITLE,
                detail = NoComputerCopy.DETAIL,
                testTag = "composerComputerChip",
                dismissLabel = "Dismiss",
                onDismiss = { chipDismissed = true },
            )
        }
        LocalComposerOrbs.current(deckOpen, sessionOpen, onNewItem)

        // Rest is one line. Focus or a draft opens the field without a pointer-focus ring.
        val cardShape = RoundedCornerShape(if (rest) 28.dp else 18.dp)
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .shadow(2.dp, cardShape, ambientColor = Color.Black.copy(alpha = 0.04f), spotColor = Color.Black.copy(alpha = 0.04f))
                .clip(cardShape)
                .background(c.bgPanel)
                .border(BorderStroke(1.dp, if (focused && inputMode.inputMode == InputMode.Keyboard) c.borderStrong else c.borderSubtle), cardShape)
                .zIndex(1f)
                .testTag("composerCard"),
        ) {
            // Expanded chips stay at the top right; scrolling keeps long model/bot names reachable.
            if (!rest) Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState(), reverseScrolling = true)
                    .padding(start = 12.dp, end = 8.dp, top = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.End),
            ) {
                if (agentChips != null) agentChips(false) else {
                    PlainChatRuntimeChip(models, iconOnly = false)
                    ModelChip(models = models, open = pickerOpen, iconOnly = false) { pickerOpen = true }
                }
            }
            if (attachments.isNotEmpty()) {
                Row(
                    modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    attachments.forEach { a -> AttachmentChip(a) { attachments.remove(a) } }
                }
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                if (rest) ComposerPlusButton { plusOpen = true }
                Box(Modifier.weight(1f).padding(start = if (rest) 0.dp else 16.dp, end = 16.dp, top = if (rest) 12.dp else 14.dp, bottom = if (rest) 12.dp else 4.dp)) {
                    if (text.isEmpty()) {
                        Crossfade(targetState = placeholders[placeholderIndex % placeholders.size], animationSpec = tween(250), label = "placeholder") { p ->
                            Text(p, color = c.textTertiary, fontSize = 16.sp, lineHeight = 24.sp, maxLines = if (rest) 1 else 3, overflow = TextOverflow.Ellipsis)
                        }
                    }
                    BasicTextField(
                        value = text,
                        onValueChange = { text = it },
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, lineHeight = 24.sp, fontFamily = ShellSans),
                        cursorBrush = SolidColor(c.textPrimary),
                        maxLines = if (rest) 1 else 6,
                        modifier = Modifier
                            .fillMaxWidth()
                            .onFocusChanged { focused = it.isFocused }
                            .testTag("composerInput")
                            .semantics { contentDescription = "Message" },
                    )
                }
                // Rest has no hanging controls: icon chips and Tools share the input line.
                if (rest) {
                    if (agentChips != null) agentChips(true) else {
                        PlainChatRuntimeChip(models, iconOnly = true)
                        ModelChip(models = models, open = pickerOpen, iconOnly = true) { pickerOpen = true }
                    }
                    if (deck != ComposerDeck.None) ControlGlyph(DesignR.drawable.ph_sliders_horizontal,
                        "Show tools", "composerTools") { setDeckFolded(false) }
                }
            }
            if (!rest) Box(Modifier.fillMaxWidth().padding(horizontal = 8.dp).height(1.dp).background(c.borderSubtle))
            // Expanded composer line: add, Tools, dictation/voice and the turn controls.
            if (!rest) Row(
                modifier = Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                ComposerPlusButton { plusOpen = true }
                Spacer(Modifier.weight(1f))
                if (deck != ComposerDeck.None) ControlGlyph(DesignR.drawable.ph_sliders_horizontal,
                    if (deckOpen) "Hide tools" else "Show tools", "composerTools") { setDeckFolded(!deckFolded) }
                if (dictationAvailable) ControlGlyph(
                    if (dictation.isRecording) DesignR.drawable.ph_microphone_fill else DesignR.drawable.ph_microphone,
                    if (dictation.isRecording) "Stop dictation" else "Dictate", "composerMic",
                    tint = if (dictation.isRecording) c.accentPrimary else null,
                ) { toggleDictation() }
                ControlGlyph(DesignR.drawable.ph_waveform, "Start voice mode", "composerVoice", enabled = onVoiceMode != null) {
                    Haptics.perform(view, Haptics.Kind.Tap)
                    startVoice()
                }
                // Play (the waiting games) sits beside Stop; it stays composed so it can tell an open game the reply is ready.
                com.allternit.feature.chat.games.WaitingGamesPlayButton(busy = isStreaming, anytime = playGamesAnytime, hidden = hasDraft)
                if (isStreaming) {
                    StreamingBars()
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
                            .alpha(if (canSend) 1f else 0.7f)
                            .clip(CircleShape)
                            .background(c.accentPrimary)
                            .testTag("composerSend")
                            .clickable(role = Role.Button, onClickLabel = "Send message") { send() }
                            .semantics { contentDescription = "Send message" },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(painterResource(DesignR.drawable.ph_arrow_up), contentDescription = null, tint = c.onAccent, modifier = Modifier.size(18.dp))
                    }
                }
            }
        }
        // Tools changes in place; reduced motion never animates the composer's height.
        if (deckOpen) {
            DeckBand(
                onHide = { setDeckFolded(true) },
                compact = deck == ComposerDeck.Compact,
                onMention = { tag -> if (!text.contains(tag)) text = tag + text },
                onAt = { text += if (text.isEmpty() || text.endsWith(" ")) "@" else " @" },
                armed = armedMode,
                onPickMode = { m ->
                    Haptics.perform(view, Haptics.Kind.Tap)
                    agentMode = m?.id
                },
                onPlugins = { onOpenSection(RailRow("plugins", "Plugins", DesignR.drawable.ph_puzzle_piece)) },
                onArtifacts = { onOpenSection(RailRow("artifacts-library", "Artifacts", com.allternit.core.design.R.drawable.ph_file_text)) },
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
private fun ControlGlyph(icon: Int, label: String, testTag: String, enabled: Boolean = true, tint: Color? = null, onClick: () -> Unit) {
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
        Icon(painterResource(icon), contentDescription = null, tint = tint ?: c.textSecondary.copy(alpha = if (enabled) 1f else 0.4f), modifier = Modifier.size(20.dp))
    }
}

/** Model chip: the matrix mark, the model's short name or "Pick a brain", and ▾ (iOS modelChip). */
@Composable
private fun ModelChip(models: ModelStore, open: Boolean, iconOnly: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val turn by animateFloatAsState(if (open) 180f else 0f, tween(180), label = "chipTurn")
    val label = models.chipLabel
    Row(
        modifier = Modifier
            .heightIn(min = 44.dp)
            .widthIn(min = if (iconOnly) 44.dp else 0.dp)
            .clip(RoundedCornerShape(10.dp))
            .testTag("composerModelChip")
            .clickable(role = Role.Button, onClick = onClick)
            .semantics(mergeDescendants = true) { contentDescription = "Model: $label" }
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp, Alignment.CenterHorizontally),
    ) {
        if (models.isLoading && models.chatModels.isEmpty()) {
            CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
            if (!iconOnly) Text("Loading...", color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
        } else {
            Box(
                Modifier
                    .size(20.dp)
                    .clip(RoundedCornerShape(6.dp))
                    .background(c.fillNeutral)
                    .border(1.dp, c.borderSubtle, RoundedCornerShape(6.dp)),
                contentAlignment = Alignment.Center,
            ) { MatrixLogo(size = 18.dp) }
            if (!iconOnly) Text(label, color = c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 150.dp))
        }
        if (!iconOnly) Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_down), contentDescription = null, tint = c.textSecondary.copy(alpha = 0.8f), modifier = Modifier.size(10.dp).rotate(turn))
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
        // Panel A: the glyph sits in a circle.
        Box(Modifier.size(28.dp).clip(CircleShape).background(c.bgPrimary), contentAlignment = Alignment.Center) {
            Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        }
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
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
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
        Icon(painterResource(com.allternit.core.design.R.drawable.ph_device_mobile), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(12.dp))
        Text("On this phone", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        Box(Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Don't run on this phone", onClick = onClear), contentAlignment = Alignment.Center) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), contentDescription = "Don't run on this phone", tint = c.textPrimary, modifier = Modifier.size(10.dp))
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
            "photo", "camera" -> com.allternit.core.design.R.drawable.ph_image
            "github" -> com.allternit.core.design.R.drawable.ph_link
            else -> com.allternit.core.design.R.drawable.ph_file_text
        }
        Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        Text(a.name, color = c.textPrimary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp).padding(vertical = 8.dp))
        if (onRemove != null) {
            Box(Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Remove ${a.name}", onClick = onRemove), contentAlignment = Alignment.Center) {
                Icon(painterResource(com.allternit.core.design.R.drawable.ph_x), contentDescription = "Remove ${a.name}", tint = c.textSecondary, modifier = Modifier.size(10.dp))
            }
        }
    }
}

/** The Allternit Office suite and the Office extensions in Plugins ▾ (iOS DeckPlugin). */
private val DeckPluginSuite = listOf(
    "Allternit Docs" to DesignR.drawable.deck_docs,
    "Allternit Sheets" to DesignR.drawable.deck_sheets,
    "Allternit Slides" to DesignR.drawable.deck_slides,
    "Allternit PDF" to DesignR.drawable.deck_pdf,
    "Allternit Sign" to DesignR.drawable.deck_sign,
)
private val DeckPluginExtensions = listOf("Allternit for Word", "Allternit for Excel", "Allternit for PowerPoint")

/**
 * The deck band under the card (iOS ComposerDeckBand): Agent mode ▾ | Plugins ▾ | Artifacts,
 * tucked 8dp under the card (its top padding clears it), rounded bottom corners, outlined on
 * the sides and bottom. Full: 16 side padding, 52 high; Compact: 12, 40.
 */
@Composable
private fun DeckBand(
    /** With no tab while open, TalkBack folds the band from here (iOS accessibilityAction "Hide tools"). */
    onHide: () -> Unit,
    compact: Boolean,
    armed: DeckMode?,
    onPickMode: (DeckMode?) -> Unit,
    onMention: (String) -> Unit,
    onAt: () -> Unit,
    onPlugins: () -> Unit,
    onArtifacts: () -> Unit,
) {
    val c = AllternitTheme.colors
    var menu by remember { mutableStateOf(false) }
    var plugins by remember { mutableStateOf(false) }
    val shape = RoundedCornerShape(bottomStart = 16.dp, bottomEnd = 16.dp)
    val border = c.borderSubtle
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .offset(y = (-8).dp)
            .zIndex(0f)
            .clip(shape)
            .background(c.bgPanel)
            .drawBehindOutline(border)
            .horizontalScroll(rememberScrollState())
            .heightIn(min = if (compact) 48.dp else 60.dp)
            .padding(start = if (compact) 12.dp else 16.dp, end = if (compact) 12.dp else 16.dp, top = if (compact) 20.dp else 22.dp, bottom = if (compact) 6.dp else 8.dp)
            .semantics { customActions = listOf(CustomAccessibilityAction("Hide tools") { onHide(); true }) }
            .testTag("composerDeck"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box {
            if (armed != null) {
                // Picking a mode turns the trigger into that mode's chip.
                DeckTrigger(armed.icon, armed.label, "deckMode", iconSize = 14, ink = armed.color ?: c.textSecondary, tintIcon = false) { menu = true }
            } else {
                DeckTrigger(DesignR.drawable.ph_diamond_fill, "Agent mode", "deckMode", iconSize = 10) { menu = true }
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
                Text("Agent mode", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                DeckModes.forEach { m ->
                    MenuRow(icon = m.icon, title = m.label, checked = armed?.id == m.id, testTag = "deckMode${m.id}") {
                        menu = false
                        onPickMode(m)
                    }
                }
                if (armed != null) {
                    MenuRow(icon = DesignR.drawable.ph_x, title = "Clear mode", testTag = "deckModeOff") {
                        menu = false
                        onPickMode(null)
                    }
                }
            }
        }
        DeckSeparator()
        Box {
            DeckTrigger(null, "Plugins", "deckPlugins") { plugins = true }
            DropdownMenu(expanded = plugins, onDismissRequest = { plugins = false }, containerColor = c.bgPrimary, shape = RoundedCornerShape(16.dp)) {
                Text("Allternit Office", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                DeckPluginSuite.forEach { (name, icon) ->
                    MenuRow(icon = icon, title = name, testTag = "deckPlugin$name") {
                        plugins = false
                        onMention("@$name ")
                    }
                }
                Text("Extensions", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp))
                DeckPluginExtensions.forEach { name ->
                    MenuRow(icon = DesignR.drawable.ph_puzzle_piece, title = name, testTag = "deckPlugin$name") {
                        plugins = false
                        onMention("@$name ")
                    }
                }
                MenuRow(icon = null, title = "More plugins…", testTag = "deckPluginsMore") {
                    plugins = false
                    onAt()
                }
                MenuRow(icon = null, title = "Manage plugins…", testTag = "deckPluginsManage") {
                    plugins = false
                    onPlugins()
                }
            }
        }
        DeckSeparator()
        DeckTrigger(DesignR.drawable.ph_stack, "Artifacts", "deckArtifacts", onClick = onArtifacts)
    }
}

/** The band's outline: both sides and the rounded bottom, no top edge (iOS DeckOutline). */
private fun Modifier.drawBehindOutline(color: Color): Modifier = this.then(
    Modifier.drawBehind {
        val r = 16.dp.toPx()
        val w = 1.dp.toPx()
        val path = androidx.compose.ui.graphics.Path().apply {
            moveTo(w / 2, 0f)
            lineTo(w / 2, size.height - r)
            arcTo(androidx.compose.ui.geometry.Rect(w / 2, size.height - 2 * r, 2 * r, size.height - w / 2), 180f, -90f, false)
            lineTo(size.width - r, size.height - w / 2)
            arcTo(androidx.compose.ui.geometry.Rect(size.width - 2 * r, size.height - 2 * r, size.width - w / 2, size.height - w / 2), 90f, -90f, false)
            lineTo(size.width - w / 2, 0f)
        }
        drawPath(path, color, style = androidx.compose.ui.graphics.drawscope.Stroke(width = w))
    },
)

/** One filled add control, shared by the rest and open composer. */
@Composable
private fun ComposerPlusButton(onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.padding(6.dp).size(44.dp).clip(CircleShape).background(c.textPrimary)
        .testTag("composerPlus").clickable(role = Role.Button, onClickLabel = "Add to chat", onClick = onClick),
        contentAlignment = Alignment.Center) {
        Icon(painterResource(DesignR.drawable.ph_plus), "Add to chat", tint = c.bgPrimary, modifier = Modifier.size(20.dp))
    }
}

/** Three small bars beside Stop while a turn runs (iOS StreamingBars). */
@Composable
private fun StreamingBars() {
    val c = AllternitTheme.colors
    val t = rememberInfiniteTransition(label = "bars")
    Row(Modifier.height(16.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        repeat(3) { i ->
            val h by t.animateFloat(6f, 16f, infiniteRepeatable(tween(500, delayMillis = i * 180), RepeatMode.Reverse), label = "bar$i")
            Box(Modifier.width(2.dp).height(h.dp).clip(CircleShape).background(c.textPrimary))
        }
    }
}

@Composable
private fun DeckSeparator() {
    Text("|", color = AllternitTheme.colors.textSecondary.copy(alpha = 0.4f), fontSize = 11.sp)
}

/** One deck trigger (iOS trigger): icon (bold), 12pt bold label, a small ▴ at 0.6. */
@Composable
private fun DeckTrigger(icon: Int?, label: String, testTag: String, iconSize: Int = 13, ink: Color? = null, tintIcon: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val color = ink ?: c.textSecondary
    Row(
        modifier = Modifier
            .heightIn(min = 28.dp)
            .clip(RoundedCornerShape(8.dp))
            .testTag(testTag)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (icon != null) {
            if (tintIcon) Icon(painterResource(icon), contentDescription = null, tint = color, modifier = Modifier.size(iconSize.dp))
            else androidx.compose.foundation.Image(painterResource(icon), contentDescription = null, modifier = Modifier.size(iconSize.dp))
        }
        Text(label, color = color, fontSize = 12.sp, fontWeight = FontWeight.Bold)
        Icon(painterResource(DesignR.drawable.ph_caret_up), contentDescription = null, tint = color.copy(alpha = 0.6f), modifier = Modifier.size(8.dp))
    }
}


/** What the Code composer's deck row opens (iOS CodeDeckRow): the computer, the usage screen, the shell, the file attach. */
internal class CodeComposerActions(
    val host: com.allternit.feature.code.CodeHost,
    val onTerminal: () -> Unit,
    val onFiles: () -> Unit,
    val onComputer: () -> Unit,
    val onUsage: () -> Unit,
)

private val CodeCoral = Color(0xFFD97757)
private val CodeIdle = Color(0xFF8EAB84)

/**
 * The Code launch composer (iOS CodeLaunchComposer): the computer chip, the prompt edge (dashed
 * rules with the model ▾ and a status dot), the `❯` prompt in monospace with + · voice · Return,
 * a dashed hairline, then the deck row (environment · folder · terminal). No card, no deck band.
 */
@Composable
internal fun CodeLaunchComposer(
    models: ModelStore,
    noComputer: Boolean,
    actions: CodeComposerActions,
    onSend: (ComposerDraft) -> Unit,
    onOpenSection: (RailRow) -> Unit,
) {
    val inputMode = LocalInputModeManager.current
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    var text by rememberSaveable { mutableStateOf("") }
    var focused by remember { mutableStateOf(false) }
    var pickerOpen by remember { mutableStateOf(false) }
    var chipDismissed by rememberSaveable { mutableStateOf(false) }
    val trimmed = text.trim()
    LaunchedEffect(Unit) { models.load() }
    // The plan window the deck's usage meter fills from (iOS UsageStore, `GET /api/v1/me/usage`).
    var planFraction by remember { mutableStateOf<Double?>(null) }
    LaunchedEffect(Unit) {
        com.allternit.feature.billing.StoreBillingClient.usage()?.let { u ->
            planFraction = if (u.weeklyLimit > 0) u.weeklyUsed / u.weeklyLimit else 0.0
        }
    }
    val dictation = remember { com.allternit.feature.chat.voice.Dictation(context) }
    var dictationBase by remember { mutableStateOf("") }
    androidx.compose.runtime.DisposableEffect(dictation) { onDispose { dictation.stop() } }
    LaunchedEffect(dictation) {
        androidx.compose.runtime.snapshotFlow { dictation.transcript }.collect { t -> if (t.isNotEmpty()) text = dictationBase + t }
    }
    val micPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) {
            dictationBase = if (text.isEmpty() || text.endsWith(" ")) text else "$text "
            dictation.start()
        }
    }
    fun toggleDictation() {
        Haptics.perform(view, Haptics.Kind.Tap)
        if (dictation.isRecording) dictation.stop()
        else if (com.allternit.feature.chat.voice.hasMicPermission(context)) {
            com.allternit.feature.chat.voice.VoicePrefs.load(context)
            dictationBase = if (text.isEmpty() || text.endsWith(" ")) text else "$text "
            dictation.start()
        } else micPermission.launch(android.Manifest.permission.RECORD_AUDIO)
    }
    fun send() {
        if (trimmed.isEmpty()) return
        if (models.selectedId.isNullOrEmpty()) { pickerOpen = true; return }
        dictation.stop()
        Haptics.perform(view, Haptics.Kind.Tap)
        val draft = ComposerDraft(text = trimmed, modelId = models.selectedId, modelLabel = models.chipLabel, effort = models.selected?.let { models.level(it) })
        text = ""
        onSend(draft)
    }
    val border = if (focused && inputMode.inputMode == InputMode.Keyboard) c.borderStrong else c.borderSubtle
    Column(
        Modifier
            .fillMaxWidth()
            .navigationBarsPadding()
            .imePadding()
            .padding(horizontal = 10.dp)
            .testTag("codeComposer"),
    ) {
        AnimatedVisibility(visible = noComputer && !chipDismissed, enter = fadeIn(), exit = fadeOut()) {
            NoticeCard(
                icon = DesignR.drawable.ph_desktop,
                title = NoComputerCopy.TITLE,
                detail = NoComputerCopy.DETAIL,
                testTag = "composerComputerChip",
                dismissLabel = "Dismiss",
                onDismiss = { chipDismissed = true },
            )
        }
        LocalComposerOrbs.current(false, false, null)
        // The prompt edge: rules, the model ▾, the status dot.
        Row(Modifier.fillMaxWidth().height(36.dp).zIndex(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            DashedRule(border, Modifier.width(10.dp))
            DashedRule(border, Modifier.weight(1f))
            Row(
                Modifier
                    .heightIn(min = 36.dp)
                    .testTag("codeModelChip")
                    .clickable(role = Role.Button, onClickLabel = "Model: ${models.chipLabel}") { pickerOpen = true },
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                MatrixLogo(size = 14.dp)
                Text(models.chipLabel, color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 200.dp))
                Icon(painterResource(DesignR.drawable.ph_caret_down), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(9.dp))
            }
            Box(Modifier.size(6.dp).clip(CircleShape).background(CodeIdle))
            DashedRule(border, Modifier.width(10.dp))
        }
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 6.dp),
            verticalAlignment = Alignment.Bottom,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Box(
                Modifier.padding(vertical = 6.dp).size(44.dp).clip(CircleShape).background(c.textPrimary).testTag("codeAttach")
                    .clickable(role = Role.Button, onClickLabel = "Attach") { actions.onFiles() },
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_plus), contentDescription = "Attach", tint = c.bgPrimary, modifier = Modifier.size(18.dp)) }
            Row(Modifier.weight(1f).padding(start = 12.dp, end = 4.dp, top = 9.dp, bottom = 9.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("❯", color = CodeCoral, fontSize = 15.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace, lineHeight = 22.sp)
                Box(Modifier.weight(1f)) {
                    if (text.isEmpty()) Text("Describe a task...", color = c.textTertiary, fontSize = 16.sp, lineHeight = 22.sp, fontFamily = FontFamily.Monospace)
                    BasicTextField(
                        value = text,
                        onValueChange = { text = it },
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, lineHeight = 22.sp, fontFamily = FontFamily.Monospace),
                        cursorBrush = SolidColor(CodeCoral),
                        maxLines = 6,
                        modifier = Modifier.fillMaxWidth().onFocusChanged { focused = it.isFocused }.testTag("composerInput").semantics { contentDescription = "Describe a task" },
                    )
                }
            }
            Row(Modifier.padding(vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.size(40.dp).testTag("codeVoice").clickable(role = Role.Button, onClickLabel = if (dictation.isRecording) "Stop voice input" else "Start voice mode") { toggleDictation() },
                    contentAlignment = Alignment.Center,
                ) { Icon(painterResource(DesignR.drawable.ph_waveform), contentDescription = null, tint = if (dictation.isRecording) CodeCoral else c.textSecondary, modifier = Modifier.size(16.dp)) }
                // Play shows before typing starts, never over a draft (iOS CodeLaunchComposer). Nothing runs on the launch screen.
                com.allternit.feature.chat.games.WaitingGamesPlayButton(busy = false, anytime = true, hidden = trimmed.isNotEmpty())
                if (trimmed.isNotEmpty()) {
                    // A terminal runs on Enter: the run key is the Return key.
                    Box(
                        Modifier.size(40.dp).clip(RoundedCornerShape(6.dp)).background(CodeCoral).testTag("composerSend")
                            .clickable(role = Role.Button, onClickLabel = "Send") { send() },
                        contentAlignment = Alignment.Center,
                    ) { Icon(painterResource(DesignR.drawable.ph_arrow_elbow_down_left), contentDescription = "Send", tint = Color.White, modifier = Modifier.size(16.dp)) }
                }
            }
        }
        DashedRule(border, Modifier.fillMaxWidth())
        // The deck row (iOS CodeDeckRow): environment · folder · branch · worktree · permission · (spacer) · usage · view menu.
        com.allternit.feature.code.CodeDeckRow(
            host = actions.host,
            planFraction = planFraction,
            onOpenTerminal = actions.onTerminal,
            onOpenUsage = actions.onUsage,
            onConnect = actions.onComputer,
        )
    }
    if (pickerOpen) {
        ModelPickerSheet(models = models, onOpenSection = { row -> pickerOpen = false; onOpenSection(row) }, onDismiss = { pickerOpen = false })
    }
}

/** A 1dp dashed rule, 3 on 3 off (iOS DashedHairline). */
@Composable
private fun DashedRule(color: Color, modifier: Modifier) {
    Canvas(modifier.height(1.dp)) {
        drawLine(color, androidx.compose.ui.geometry.Offset(0f, 0.5f), androidx.compose.ui.geometry.Offset(size.width, 0.5f), strokeWidth = 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(3.dp.toPx(), 3.dp.toPx())))
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
