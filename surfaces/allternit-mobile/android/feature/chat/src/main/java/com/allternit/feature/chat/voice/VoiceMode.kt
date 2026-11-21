package com.allternit.feature.chat.voice

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearOutSlowInEasing
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import com.allternit.core.design.AllternitSerif
import com.allternit.core.design.AllternitStatic
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.semantics.clearAndSetSemantics
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatSendRequest
import com.allternit.feature.chat.ChatViewModel
import com.allternit.feature.chat.MessageRecord
import java.util.UUID
import kotlinx.coroutines.delay

/** iOS VoiceSummary: the feed's "Voice chat ended · Ns" card. */
data class VoiceSummary(val durationSeconds: Int, val endedAt: Long = System.currentTimeMillis()) {
    val formattedDuration: String
        get() {
            val m = durationSeconds / 60
            val s = durationSeconds % 60
            return if (m > 0) (if (s > 0) "${m}m ${s}s" else "${m}m") else "${s}s"
        }
}

/** iOS VoiceModeViewModel.TurnState. */
enum class TurnState { Idle, Listening, Thinking, Speaking }

/**
 * iOS VoiceModeViewModel on its system path: on-device recognition ([Dictation]) → the transcript goes
 * through the normal chat send (the conversation stays in the text thread) → the streamed reply is
 * read aloud sentence by sentence ([Speaker]) → back to listening (hands free) or idle (push to talk).
 *
 * Gap run GS-D: the engines come from [VoiceEngines.choose] (iOS VoiceRouter): the person's own OpenAI key
 * (Realtime listening + OpenAI speech) or ElevenLabs key (ElevenLabs speech), else the phone's recognizer and voice.
 * "On this device" runs the sherpa-onnx engine (LocalVoice.kt) when its native library loads ([DeviceVoice]);
 * the first session downloads the listening pack with progress in the status line, as iOS.
 */
class VoiceModeViewModel(
    private val context: Context,
    private val chat: ChatViewModel,
    private val runtimeModelId: String?,
    private val effort: String?,
    private val dictation: VoiceListener,
    private val speaker: ReplySpeaker,
    /** iOS engineLabel: null on the system recognizer path. */
    val engineLabel: String? = null,
    /** Why the chosen route fell back (iOS VoiceRouter.fallbackNotice). */
    private val notice: String? = null,
) {
    var state by mutableStateOf(TurnState.Idle)
        private set
    var liveTranscript by mutableStateOf("")
    var replyText by mutableStateOf("")
        private set
    var isMuted by mutableStateOf(false)
        private set
    var statusLine by mutableStateOf<String?>(null)
        private set
    val isConnecting: Boolean = false

    private var replyMessageId: String? = null
    private var isActive = false
    private var suppressNextDictationEnd = false
    private var startedAt = System.currentTimeMillis()
    private val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private var focusRequest: AudioFocusRequest? = null

    val isPushToTalk: Boolean get() = VoicePrefs.interactionMode == VoiceInteractionMode.PushToTalk
    val canInterrupt: Boolean get() = state == TurnState.Thinking || state == TurnState.Speaking

    /** The on-phone engine's pack download line ("Downloading the voice model… 42%"), iOS statusLine + modelProgress. */
    fun packStatus(line: String?) {
        if (line != null) statusLine = line
        else if (statusLine?.startsWith("Downloading the voice model") == true) statusLine = notice
    }

    fun begin() {
        startedAt = System.currentTimeMillis()
        isActive = true
        statusLine = notice
        requestFocus()
        if (!hasMicPermission(context)) {
            statusLine = "Microphone access is off. Turn it on in Settings › Apps › Allternit."
            state = TurnState.Idle
            return
        }
        if (VoicePrefs.interactionMode == VoiceInteractionMode.HandsFree) startListening() else state = TurnState.Idle
    }

    /** X: tears down audio, returns the length in seconds for the summary card. */
    fun endSession(): Int {
        isActive = false
        suppressNextDictationEnd = true
        dictation.stop()
        speaker.stop()
        abandonFocus()
        return maxOf(1, ((System.currentTimeMillis() - startedAt) / 1000).toInt())
    }

    fun toggleMute() {
        isMuted = !isMuted
        if (isMuted) {
            suppressNextDictationEnd = true
            liveTranscript = ""
            dictation.stop()
            speaker.stop()
            if (state == TurnState.Listening || state == TurnState.Speaking) state = TurnState.Idle
        } else if (isActive && state == TurnState.Idle && !isPushToTalk) {
            startListening()
        }
    }

    /** Tap to interrupt: speaking stops playback, thinking aborts the stream; then the next turn. */
    fun interrupt() {
        if (!isActive) return
        when (state) {
            TurnState.Speaking -> { speaker.stop(); replyMessageId = null; advanceAfterSpeech() }
            TurnState.Thinking -> { chat.stopStreaming(); replyMessageId = null; advanceAfterSpeech() }
            else -> {}
        }
    }

    fun pressBegan() {
        if (!isActive || !isPushToTalk || isMuted) return
        if (state == TurnState.Idle || state == TurnState.Listening) startListening()
    }

    fun pressEnded() {
        if (!isActive || !isPushToTalk) return
        dictation.stop()
    }

    fun dictationEnded() {
        if (!isActive) return
        if (suppressNextDictationEnd) { suppressNextDictationEnd = false; return }
        val transcript = dictation.transcript.trim()
        if (transcript.isEmpty()) {
            if (!isPushToTalk && !isMuted && state == TurnState.Listening) startListening()
            else if (state == TurnState.Listening) state = TurnState.Idle
            return
        }
        sendTurn(transcript)
    }

    fun dictationFailed(message: String) {
        if (!isActive) return
        statusLine = message
        state = TurnState.Idle
    }

    fun replyUpdated(messages: List<MessageRecord>) {
        val id = replyMessageId ?: return
        if (!isActive) return
        val reply = messages.firstOrNull { it.id == id } ?: return
        if (state != TurnState.Thinking && state != TurnState.Speaking) return
        replyText = reply.content
        reply.error?.let {
            statusLine = it.title
            speaker.stop()
            state = TurnState.Idle
            replyMessageId = null
            return
        }
        if (reply.isStreaming || state != TurnState.Thinking) return
        replyMessageId = null
        if (isMuted || reply.content.isBlank()) { advanceAfterSpeech(); return }
        state = TurnState.Speaking
        sentences(reply.content).forEach { speaker.speakChunk(it) }
        if (!speaker.isSpeaking) advanceAfterSpeech()
    }

    fun speakerFinished() {
        if (!isActive || state != TurnState.Speaking) return
        advanceAfterSpeech()
    }

    private fun startListening() {
        if (isMuted) return
        statusLine = null
        liveTranscript = ""
        state = TurnState.Listening
        dictation.start()
    }

    private fun sendTurn(transcript: String) {
        liveTranscript = ""
        replyText = ""
        statusLine = null
        state = TurnState.Thinking
        chat.send(ChatSendRequest(text = transcript, runtimeModelId = runtimeModelId, effort = effort))
        replyMessageId = chat.messages.lastOrNull { it.role == "assistant" }?.id
        if (replyMessageId == null) {
            statusLine = "Couldn't start the reply stream."
            state = TurnState.Idle
        }
    }

    private fun advanceAfterSpeech() {
        if (!isActive) return
        if (!isPushToTalk && !isMuted) startListening() else state = TurnState.Idle
    }

    // Audio focus: iOS ducks others and pauses on a phone call / Siri interruption.
    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        if (!isActive) return@OnAudioFocusChangeListener
        when (change) {
            AudioManager.AUDIOFOCUS_LOSS, AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                suppressNextDictationEnd = true
                dictation.stop()
                speaker.stop()
                statusLine = "Paused — audio interrupted"
                state = TurnState.Idle
            }
            AudioManager.AUDIOFOCUS_GAIN -> if (!isPushToTalk && !isMuted && state == TurnState.Idle) startListening()
        }
    }

    private fun requestFocus() {
        if (Build.VERSION.SDK_INT >= 26) {
            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANT).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setOnAudioFocusChangeListener(focusListener)
                .build()
            focusRequest = req
            audio.requestAudioFocus(req)
        }
    }

    private fun abandonFocus() {
        if (Build.VERSION.SDK_INT >= 26) focusRequest?.let { audio.abandonAudioFocusRequest(it) }
        focusRequest = null
    }

    companion object {
        /** iOS sentences(in:): break after . ! ? or a newline, keeping the terminator. */
        fun sentences(text: String): List<String> {
            val out = ArrayList<String>()
            val cur = StringBuilder()
            for (ch in text) {
                cur.append(ch)
                if (ch == '.' || ch == '!' || ch == '?' || ch == '\n') {
                    cur.toString().trim().takeIf { it.isNotEmpty() }?.let { out += it }
                    cur.clear()
                }
            }
            cur.toString().trim().takeIf { it.isNotEmpty() }?.let { out += it }
            return out
        }
    }
}

/** iOS VoiceHost: one voice session per chat, shown full screen or as the bar over the composer. */
class VoiceHost(private val context: Context) {
    enum class Presentation { None, Bar, Full }

    var presentation by mutableStateOf(Presentation.None)
    var viewModel by mutableStateOf<VoiceModeViewModel?>(null)
        private set
    val dictation = Dictation(context)
    val speaker by lazy { Speaker(context) }
    /** Gap run GS-D: what this session listens and speaks with (VoiceEngines.choose). */
    var listener by mutableStateOf<VoiceListener>(dictation)
        private set
    private var replySpeaker: ReplySpeaker? = null
    val isActive: Boolean get() = viewModel != null
    /** True while replies are read by the on-phone Kokoro voice (the voice settings sheet then lists Kokoro's voices). */
    val repliesInKokoro: Boolean get() = replySpeaker is KokoroSpeaker

    /**
     * iOS VoiceHost.start: starts a voice conversation in [presentation] (a no-op when one is live: it only changes
     * the presentation). Like iOS, the composer starts it as the bar over the composer; the bar expands to full.
     * [bot] is the bot this chat is with: its own Kokoro voice reads the replies on the on-phone engine.
     */
    fun start(chat: ChatViewModel, runtimeModelId: String?, effort: String?, bot: VoiceBot? = null, presentation: Presentation = Presentation.Bar) {
        VoicePrefs.load(context)
        if (viewModel == null) {
            val choice = VoiceEngines.choose(context, VoicePrefs.interactionMode == VoiceInteractionMode.PushToTalk, dictation, speaker, botVoice = bot?.voiceId)
            replySpeaker?.takeIf { it !== choice.speaker && it !== speaker }?.shutdown()
            listener = choice.listener
            replySpeaker = choice.speaker
            val model = VoiceModeViewModel(context, chat, runtimeModelId, effort, choice.listener, choice.speaker, choice.label, choice.notice)
            choice.listener.onStopped = { viewModel?.dictationEnded() }
            choice.speaker.onFinished = { viewModel?.speakerFinished() }
            speaker.onFinished = { viewModel?.speakerFinished() }
            viewModel = model
            model.begin()
        }
        this.presentation = presentation
    }

    /** Ends the session; the seconds for the summary card, or null when none was running. */
    fun end(): Int? {
        val vm = viewModel ?: return null
        val seconds = vm.endSession()
        viewModel = null
        presentation = Presentation.None
        replySpeaker?.stop()
        speaker.stop()
        return seconds
    }

    fun dispose() {
        end()
        replySpeaker?.takeIf { it !== speaker }?.shutdown()
        replySpeaker = null
        speaker.shutdown()
    }
}

/** iOS VoiceHostWiring: the transcript, errors and the chat feed flow into the view model. */
@Composable
fun VoiceHostWiring(host: VoiceHost, chat: ChatViewModel) {
    LaunchedEffect(host) { snapshotFlow { host.listener.transcript }.collect { host.viewModel?.liveTranscript = it } }
    LaunchedEffect(host) { snapshotFlow { host.listener.status }.collect { host.viewModel?.packStatus(it) } }
    LaunchedEffect(host) { snapshotFlow { host.listener.errorMessage }.collect { m -> if (m != null) host.viewModel?.dictationFailed(m) } }
    LaunchedEffect(host, chat) { snapshotFlow { chat.messages }.collect { host.viewModel?.replyUpdated(it) } }
    DisposableEffect(host) { onDispose { host.dispose() } }
}

private fun TurnState.statusLabel(vm: VoiceModeViewModel): String = when (this) {
    TurnState.Idle -> if (vm.isMuted) "Muted" else if (vm.isPushToTalk) "Hold to talk" else "Tap to talk"
    TurnState.Listening -> "Listening…"
    TurnState.Thinking -> "Thinking…"
    TurnState.Speaking -> "Speaking…"
}

/** iOS stateTint: the turn state's two colours (blue family listening, warm speaking), eased over 0.8 s like iOS. */
@Composable
private fun rememberStateTint(state: TurnState): Pair<Color, Color> {
    val c = AllternitTheme.colors
    val target = when (state) {
        TurnState.Idle -> c.bgTertiary to c.bgSecondary
        TurnState.Listening -> AllternitStatic.StatusInfo to Color(0xFF1E3A8A)
        TurnState.Thinking -> Color(0xFF7C8DB0) to Color(0xFF2E3A52)
        TurnState.Speaking -> c.accentPrimary to Color(0xFF7C3A1E)
    }
    val primary by animateColorAsState(target.first, tween(800), label = "voiceTint1")
    val secondary by animateColorAsState(target.second, tween(800), label = "voiceTint2")
    return primary to secondary
}

/**
 * iOS VoiceModeView: full-screen voice mode. Like iOS it has two stages:
 *
 * - A bot's chat ([bot] set, iOS botStage): "Talking with <bot>", the bot's avatar as the speaker's face at 148 in the
 *   bot's own live mode look (Settings › Voice › Live mode, a bot's own pick winning), the bot's colour, the state in
 *   words, the captions band and the controls.
 * - A plain chat (iOS plainStage): no speaker. The soft ambient light tinted by the turn state (blue listening, warm
 *   speaking), the reply in serif in the middle (else the state in words, "Listening…"), "Tap to interrupt" while a
 *   tap would act, your words in italics, the engine ("On this device") just above the controls, and the glow behind
 *   the mic.
 *
 * Controls (both stages): settings, minimize to the bar, the mic (tap mutes; hold talks in push to talk) and end. The
 * ambient glows are full-screen radial gradients, not blurred boxes: `Modifier.blur` clips to its box (hard-edged
 * rectangles on Android 12+) and does nothing below API 31.
 */
@Composable
fun VoiceModeView(host: VoiceHost, viewModel: VoiceModeViewModel, onEnd: (Int) -> Unit, bot: VoiceBot? = null) {
    SpeakerLookStore.load(LocalContext.current)
    var settingsOpen by remember { mutableStateOf(false) }
    val tint = rememberStateTint(viewModel.state)
    val controls: @Composable () -> Unit = { ControlsRow(host, viewModel, onEnd, tint.first) { settingsOpen = true } }
    if (bot != null) BotStage(host, viewModel, bot, controls) else PlainStage(viewModel, tint, controls)
    if (settingsOpen) VoiceSettingsSheet(host.speaker, bot, kokoro = host.repliesInKokoro) { settingsOpen = false }
}

/** A tap that interrupts the reply (iOS onTapGesture { viewModel.interrupt() }); no ripple, like iOS. */
@Composable
private fun interruptModifier(vm: VoiceModeViewModel): Modifier = Modifier.clickable(
    interactionSource = remember { MutableInteractionSource() }, indication = null,
    enabled = vm.canInterrupt, onClickLabel = "Interrupt",
) { vm.interrupt() }

/** iOS botStage. */
@Composable
private fun BotStage(host: VoiceHost, viewModel: VoiceModeViewModel, bot: VoiceBot, controls: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    val (style, palette) = SpeakerLookStore.look(bot.id)
    val botColor = bot.color
    val state = viewModel.state
    val statusLabel = state.statusLabel(viewModel)
    val youTalking = state == TurnState.Listening && viewModel.liveTranscript.isNotEmpty()
    val captionText = when {
        youTalking -> viewModel.liveTranscript
        state == TurnState.Speaking || state == TurnState.Thinking -> viewModel.replyText
        else -> ""
    }
    val quiet = when {
        viewModel.isConnecting -> "Connecting"
        state == TurnState.Thinking && viewModel.replyText.isEmpty() -> "${bot.name} is thinking"
        else -> null
    }
    val interrupt = interruptModifier(viewModel)
    Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("voiceMode")) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            Text("Talking with ${bot.name}", color = c.textSecondary, fontSize = 15.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 12.dp).testTag("voiceTalkingWith"))
            Spacer(Modifier.weight(1f))
            VoiceSpeakerView(
                phase = state.speakerPhase(viewModel.isConnecting), muted = viewModel.isMuted, style = style, palette = palette,
                botColor = botColor, sizeDp = 148f, level = { host.listener.micLevel },
                modifier = interrupt.testTag("voiceSpeaker"),
            ) { bot.face(148f) }
            Text(
                statusLabel.uppercase(), color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 1.6.sp,
                modifier = Modifier.padding(top = 8.dp).semantics { contentDescription = statusLabel }.testTag("voiceStatus"),
            )
            viewModel.engineLabel?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.padding(top = 6.dp).testTag("voiceEngine")) }
            viewModel.statusLine?.let {
                Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(horizontal = 32.dp).padding(top = 6.dp))
            }
            Spacer(Modifier.weight(1f))
            Box(Modifier.fillMaxWidth().padding(horizontal = 14.dp).padding(bottom = 14.dp).then(interrupt), contentAlignment = Alignment.Center) {
                VoiceCaptions(
                    you = youTalking, name = bot.name, text = captionText, quiet = quiet, sizeSp = SpeakerLookStore.captionSize.sp, color = botColor,
                    avatar = { bot.face(18f) },
                )
            }
            controls()
        }
    }
}

/** iOS plainStage: the ambient light, the reply (or the state) in serif, your words in italics, the engine, the controls. */
@Composable
private fun PlainStage(vm: VoiceModeViewModel, tint: Pair<Color, Color>, controls: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    val state = vm.state
    val interrupt = interruptModifier(vm)
    Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("voiceMode")) {
        AmbientLight(state, tint)
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            // iOS centerContent between two spacers.
            Column(Modifier.weight(1f).fillMaxWidth(), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                if (vm.replyText.isNotEmpty()) {
                    // The reply in serif (Claude voice-mode parity), scrollable; a tap while thinking or speaking interrupts.
                    Text(
                        vm.replyText, color = c.textPrimary, fontSize = 22.sp, lineHeight = 29.sp, fontWeight = FontWeight.Medium, fontFamily = AllternitSerif,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.weight(1f, fill = false).fillMaxWidth().verticalScroll(rememberScrollState()).then(interrupt)
                            .padding(horizontal = 36.dp).testTag("voiceReply"),
                    )
                } else {
                    Text(
                        state.statusLabel(vm), color = c.textSecondary, fontSize = 22.sp, lineHeight = 29.sp, fontWeight = FontWeight.Medium, fontFamily = AllternitSerif,
                        textAlign = TextAlign.Center, modifier = Modifier.then(interrupt).padding(horizontal = 40.dp).testTag("voiceStatus"),
                    )
                }
                // Barge-in affordance, only while a tap would act.
                AnimatedVisibility(vm.canInterrupt, enter = fadeIn(tween(200)), exit = fadeOut(tween(200))) { InterruptHint() }
            }
            // Live transcript (Claude parity: italics, bottom); a blank line keeps the height while it's empty.
            Text(
                vm.liveTranscript.ifEmpty { " " }, color = c.textPrimary.copy(alpha = 0.85f), fontSize = 17.sp, fontStyle = FontStyle.Italic,
                textAlign = TextAlign.Center, maxLines = 3, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 32.dp).padding(bottom = 6.dp).testTag("voiceTranscript"),
            )
            vm.engineLabel?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.padding(bottom = 4.dp).testTag("voiceEngine")) }
            // In-place error / status line: no alerts.
            vm.statusLine?.let {
                Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(horizontal = 32.dp).padding(bottom = 6.dp))
            }
            controls()
        }
    }
}

/** iOS "Tap to interrupt": a small capsule under the reply. */
@Composable
private fun InterruptHint() {
    val c = AllternitTheme.colors
    Row(
        Modifier.padding(top = 10.dp, bottom = 10.dp).clip(CircleShape).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 6.dp).testTag("voiceInterruptHint"),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_hand_tap), null, tint = c.textSecondary, modifier = Modifier.size(11.dp))
        Text("Tap to interrupt", color = c.textSecondary, fontSize = 12.sp)
    }
}

/** iOS controlsRow: settings, minimize to the bar, the mic in the middle, end. */
@Composable
private fun ControlsRow(host: VoiceHost, vm: VoiceModeViewModel, onEnd: (Int) -> Unit, glow: Color, onSettings: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 36.dp).padding(bottom = 20.dp), verticalAlignment = Alignment.CenterVertically) {
        RoundControl(DesignR.drawable.ph_gear, "Voice settings", "voiceSettings", onClick = onSettings)
        Spacer(Modifier.size(12.dp))
        RoundControl(DesignR.drawable.ph_arrows_in, "Minimize to voice bar", "voiceMinimize") { host.presentation = VoiceHost.Presentation.Bar }
        Spacer(Modifier.weight(1f))
        MicButton(vm, glow)
        Spacer(Modifier.weight(1f))
        Spacer(Modifier.size(56.dp))
        RoundControl(DesignR.drawable.ph_x, "End voice chat", "voiceEnd") { host.end()?.let(onEnd) }
    }
}

/**
 * iOS ambientBackground: two soft glows tinted by the turn state, breathing while listening or speaking (held still
 * with reduce motion). Painted as radial gradients over the whole screen (same look on every API level), centred
 * where iOS centres its circles.
 */
@Composable
private fun AmbientLight(state: TurnState, tint: Pair<Color, Color>) {
    val (primary, secondary) = tint
    val animating = state == TurnState.Listening || state == TurnState.Speaking
    val reduce = rememberReduceMotion()
    val pulse by rememberInfiniteTransition(label = "voicePulse").animateFloat(0f, 1f, infiniteRepeatable(tween(2200), RepeatMode.Reverse), label = "pulse")
    Box(
        Modifier.fillMaxSize().drawBehind {
            val p = if (reduce) 0.5f else pulse
            val a1 = if (animating) 0.3f + 0.2f * p else 0.25f
            val a2 = if (animating) 0.25f + 0.2f * p else 0.2f
            // iOS: a 420 pt circle blurred 120 at (-60, -180) from the centre, and a 460 pt one blurred 130 at (70, 220).
            glow(primary, a1, Offset(center.x - 60.dp.toPx(), center.y - 180.dp.toPx()), (210 + 120).dp.toPx())
            glow(secondary, a2, Offset(center.x + 70.dp.toPx(), center.y + 220.dp.toPx()), (230 + 130).dp.toPx())
        },
    )
}

/** A blurred disc's light: full strength in the middle, easing to nothing at [radius]. */
private fun DrawScope.glow(color: Color, alpha: Float, at: Offset, radius: Float) {
    val a = color.alpha * alpha
    drawRect(
        Brush.radialGradient(
            0f to color.copy(alpha = a), 0.35f to color.copy(alpha = a * 0.82f), 0.65f to color.copy(alpha = a * 0.35f), 1f to color.copy(alpha = 0f),
            center = at, radius = radius,
        ),
    )
}

/**
 * iOS micButton: a white disc (grey when muted) with the turn state's glow behind it, stronger while listening (iOS
 * `.shadow(color: tint.opacity(listening ? 0.6 : 0.2), radius: listening ? 16 : 6)`), painted as a radial gradient.
 * Hands free: a tap toggles mute. Push to talk: hold to record, release to send; a short tap still toggles mute.
 */
@Composable
private fun MicButton(vm: VoiceModeViewModel, glow: Color) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val listening = vm.state == TurnState.Listening
    Box(
        Modifier.size(72.dp)
            .drawBehind {
                val r = size.minDimension / 2f
                val outer = r + (if (listening) 16.dp else 6.dp).toPx() * 2f
                val a = glow.alpha * (if (listening) 0.6f else 0.2f)
                val k = r / outer
                drawCircle(
                    Brush.radialGradient(
                        0f to glow.copy(alpha = a), k to glow.copy(alpha = a * 0.6f), (k + 1f) / 2f to glow.copy(alpha = a * 0.18f), 1f to glow.copy(alpha = 0f),
                        center = center, radius = outer,
                    ),
                    radius = outer,
                )
            }
            .clip(CircleShape).background(if (vm.isMuted) c.bgSecondary else Color.White)
            .testTag("voiceMic")
            .semantics { contentDescription = if (vm.isMuted) "Unmute microphone" else "Mute microphone" }
            .pointerInput(vm) {
                awaitEachGesture {
                    awaitFirstDown()
                    val downAt = System.currentTimeMillis()
                    if (vm.isPushToTalk) vm.pressBegan()
                    waitForUpOrCancellation()
                    val wasTap = System.currentTimeMillis() - downAt < 250
                    Haptics.perform(view, Haptics.Kind.Tap)
                    if (vm.isPushToTalk && !wasTap) vm.pressEnded() else vm.toggleMute()
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(if (vm.isMuted) DesignR.drawable.ph_microphone_slash else DesignR.drawable.ph_microphone_fill), null,
            tint = if (vm.isMuted) c.textSecondary else Color.Black, modifier = Modifier.size(26.dp))
    }
}

@Composable
internal fun RoundControl(icon: Int, label: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.size(44.dp).clip(CircleShape).background(c.fillNeutral).border(1.dp, Color.White.copy(alpha = 0.18f), CircleShape)
            .testTag(tag)
            .clickable(role = Role.Button, onClickLabel = label) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), null, tint = c.textPrimary, modifier = Modifier.size(18.dp)) }
}

/**
 * iOS ChatView's quick voice bar slot: the bar above the composer while a voice conversation runs in the bar
 * presentation. It moves in from the bottom with a fade (iOS `.move(edge: .bottom)` + `.opacity`, ease-out 0.2 s);
 * with reduce motion it appears in place.
 */
@Composable
fun VoiceBarSlot(host: VoiceHost, onEnd: (Int) -> Unit, bot: VoiceBot? = null) {
    val vm = host.viewModel
    // The last session stays on screen while the bar animates out after the session ends.
    val last = remember { arrayOfNulls<VoiceModeViewModel>(1) }
    if (vm != null) last[0] = vm
    val reduce = rememberReduceMotion()
    AnimatedVisibility(
        visible = host.presentation == VoiceHost.Presentation.Bar && vm != null,
        enter = if (reduce) EnterTransition.None else expandVertically(tween(200, easing = LinearOutSlowInEasing), expandFrom = Alignment.Top) +
            slideInVertically(tween(200, easing = LinearOutSlowInEasing)) { it / 2 } + fadeIn(tween(200)),
        exit = if (reduce) ExitTransition.None else shrinkVertically(tween(200, easing = LinearOutSlowInEasing), shrinkTowards = Alignment.Top) +
            slideOutVertically(tween(200, easing = LinearOutSlowInEasing)) { it / 2 } + fadeOut(tween(200)),
    ) { last[0]?.let { VoiceQuickBar(host, it, onEnd, bot) } }
}

/**
 * iOS ChatView's fullScreenCover for voice mode: it slides up over the whole chat (a fade with reduce motion), and
 * Back returns to the bar, as iOS's dismiss does.
 */
@Composable
fun VoiceFullSlot(host: VoiceHost, onEnd: (Int) -> Unit, bot: VoiceBot? = null) {
    val vm = host.viewModel
    val last = remember { arrayOfNulls<VoiceModeViewModel>(1) }
    if (vm != null) last[0] = vm
    val visible = host.presentation == VoiceHost.Presentation.Full && vm != null
    BackHandler(enabled = visible) { host.presentation = if (host.isActive) VoiceHost.Presentation.Bar else VoiceHost.Presentation.None }
    val reduce = rememberReduceMotion()
    AnimatedVisibility(
        visible = visible,
        enter = if (reduce) fadeIn(tween(150)) else slideInVertically(tween(320, easing = FastOutSlowInEasing)) { it },
        exit = if (reduce) fadeOut(tween(150)) else slideOutVertically(tween(260, easing = FastOutSlowInEasing)) { it },
    ) { last[0]?.let { VoiceModeView(host, it, onEnd, bot) } }
}

/**
 * The Meetings screens' small status ring (voice mode and its quick bar draw [VoiceSpeakerView] in the live mode
 * look instead): it breathes with a level while listening, with a syllable rhythm while speaking, and turns slowly
 * while thinking or connecting. Muted draws it grey and still.
 */
@Composable
fun VoiceSpeakerRing(state: TurnState, muted: Boolean, level: Float, size: Int, connecting: Boolean = false) {
    val c = AllternitTheme.colors
    val t by rememberInfiniteTransition(label = "ring").animateFloat(0f, 1000f, infiniteRepeatable(tween(1_000_000)), label = "t")
    val time = t
    val lv = when {
        muted -> 0f
        state == TurnState.Listening -> level
        state == TurnState.Speaking -> (kotlin.math.sin(time * 11) * 0.5f + kotlin.math.sin(time * 3.1f) * 0.35f + 0.25f).coerceIn(0f, 1f) * 0.8f
        else -> 0f
    }
    val color = when {
        muted -> c.textTertiary
        state == TurnState.Speaking -> c.accentPrimary
        state == TurnState.Listening -> Color(0xFF3B82F6)
        else -> Color(0xFF7C8DB0)
    }
    Canvas(Modifier.size((size * 1.4f).dp).semantics { contentDescription = "" }) {
        val r = this.size.minDimension * 0.34f
        drawCircle(color.copy(alpha = 0.18f + lv * 0.25f), radius = r * (1.15f + lv * 0.25f))
        val working = state == TurnState.Thinking || connecting
        val n = 48
        for (i in 0 until n) {
            val ang = (i.toFloat() / n) * (2 * Math.PI).toFloat() + if (working) time * 0.8f else 0f
            val wob = 0.5f + 0.5f * kotlin.math.sin(time * 6 + i * 1.7f)
            val amp = if (working) 0.3f + 0.7f * ((kotlin.math.sin(time * 4 - i * 0.5f) + 1) / 2) else 0.2f + lv * 1.4f * wob
            val inner = r * 0.98f
            val outer = r * (1.02f + amp * 0.16f)
            val cx = center.x; val cy = center.y
            drawLine(color.copy(alpha = 0.35f + minOf(0.65f, amp)), Offset(cx + kotlin.math.cos(ang) * inner, cy + kotlin.math.sin(ang) * inner),
                Offset(cx + kotlin.math.cos(ang) * outer, cy + kotlin.math.sin(ang) * outer), strokeWidth = 3.dp.toPx())
        }
        drawCircle(color.copy(alpha = 0.6f), radius = r * 0.92f, style = Stroke(1.5.dp.toPx()))
    }
}

/**
 * iOS VoiceQuickBar: the minimized session above the composer. A bot's chat shows the bot's face in a small version
 * of its live mode look; a plain chat shows the Allternit thinking mark at half size. Then the status (your words
 * while you talk), the engine or the warning, and mute, expand and end.
 */
@Composable
fun VoiceQuickBar(host: VoiceHost, viewModel: VoiceModeViewModel, onEnd: (Int) -> Unit, bot: VoiceBot? = null) {
    val c = AllternitTheme.colors
    SpeakerLookStore.load(LocalContext.current)
    val state = viewModel.state
    val status = when (state) {
        TurnState.Idle -> if (viewModel.isMuted) "Muted" else "Voice paused"
        TurnState.Listening -> viewModel.liveTranscript.ifEmpty { "Listening…" }
        TurnState.Thinking -> "Thinking…"
        TurnState.Speaking -> "Speaking…"
    }
    val secondary = viewModel.statusLine ?: viewModel.engineLabel
    val shape = RoundedCornerShape(AllternitRadii.MD)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 6.dp).clip(shape)
            // iOS BgSecondary with Theme.borderWarmDefault (TextPrimary at 11%).
            .background(c.bgSecondary).border(1.dp, c.textPrimary.copy(alpha = 0.11f), shape)
            .padding(horizontal = 14.dp, vertical = 8.dp).testTag("voiceQuickBar"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(34.dp), contentAlignment = Alignment.Center) {
            if (bot != null) {
                val (style, palette) = SpeakerLookStore.look(bot.id)
                VoiceSpeakerView(
                    phase = state.speakerPhase(viewModel.isConnecting), muted = viewModel.isMuted, style = style, palette = palette,
                    botColor = bot.color, sizeDp = 28f, compact = true, level = { host.listener.micLevel },
                ) { bot.face(28f) }
            } else {
                ThinkingOrb(
                    modifier = Modifier.requiredSize(ThinkingOrbSize.Avatar.dimension.dp).graphicsLayer { scaleX = 0.5f; scaleY = 0.5f },
                    state = when (state) {
                        TurnState.Idle, TurnState.Listening -> ThinkingOrbState.Listening
                        TurnState.Thinking -> ThinkingOrbState.Breathing
                        TurnState.Speaking -> ThinkingOrbState.Composing
                    },
                    size = ThinkingOrbSize.Avatar, paused = state == TurnState.Idle, label = "",
                )
            }
        }
        Column(Modifier.weight(1f).semantics(mergeDescendants = true) { contentDescription = "Voice chat, $status" }, verticalArrangement = Arrangement.spacedBy(1.dp)) {
            Text(status, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            secondary?.let {
                Text(it, color = if (viewModel.statusLine != null) AllternitStatic.StatusWarning else c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        BarButton(
            if (viewModel.isMuted) DesignR.drawable.ph_microphone_slash else DesignR.drawable.ph_microphone_fill,
            if (viewModel.isMuted) "Unmute microphone" else "Mute microphone", "voiceBarMute",
            tint = if (viewModel.isMuted) AllternitStatic.StatusError else null,
        ) { viewModel.toggleMute() }
        BarButton(DesignR.drawable.ph_arrows_out, "Open full-screen voice", "voiceBarExpand") { host.presentation = VoiceHost.Presentation.Full }
        BarButton(DesignR.drawable.ph_x, "End voice chat", "voiceBarEnd") { host.end()?.let(onEnd) }
    }
}

/** iOS barButton: a 36 pt glass circle with a 14 pt glyph. */
@Composable
private fun BarButton(icon: Int, label: String, tag: String, tint: Color? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.size(36.dp).clip(CircleShape).background(c.fillNeutral).testTag(tag)
            .clickable(role = Role.Button, onClickLabel = label) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { contentDescription = label },
        contentAlignment = Alignment.Center,
    ) { Icon(painterResource(icon), null, tint = tint ?: c.textPrimary, modifier = Modifier.size(15.dp)) }
}

/** iOS VoiceSummaryCardView: "Voice chat ended · 2m 5s" with thumbs up / down (local, like iOS). */
@Composable
fun VoiceSummaryCard(summary: VoiceSummary) {
    val c = AllternitTheme.colors
    var feedback by remember { mutableStateOf(0) }
    Row(
        Modifier.widthIn(max = 360.dp).fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(12.dp).testTag("voiceSummaryCard"),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(36.dp).clip(CircleShape).background(c.accentPrimary.copy(alpha = 0.14f)), contentAlignment = Alignment.Center) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_waveform), null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
        }
        Column(Modifier.weight(1f)) {
            Text("Voice chat ended · ${summary.formattedDuration}", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text("The conversation continues below", color = c.textSecondary, fontSize = 11.sp)
        }
        Icon(painterResource(com.allternit.core.design.R.drawable.ph_thumbs_up), "Good session", tint = if (feedback == 1) c.textPrimary else c.textTertiary,
            modifier = Modifier.size(30.dp).clip(CircleShape).clickable { feedback = if (feedback == 1) 0 else 1 }.padding(7.dp))
        Icon(painterResource(com.allternit.core.design.R.drawable.ph_thumbs_down), "Bad session", tint = if (feedback == -1) c.textPrimary else c.textTertiary,
            modifier = Modifier.size(30.dp).clip(CircleShape).clickable { feedback = if (feedback == -1) 0 else -1 }.padding(7.dp))
    }
}

/**
 * iOS VoiceCaptionsView: TV-style captions in a dark band. The speaker's chip in their colour (or "You" with a live
 * dot), the reply's words appearing at a speaking pace (all at once for you, or with reduce motion), and past two
 * lines the text rolls up, the older line fading out at the top. Between turns a quiet "is thinking" line keeps the
 * band from going blank. [sizeSp] is Settings › Voice › Live mode › Caption size (15, 17 or 21).
 */
@Composable
fun VoiceCaptions(
    you: Boolean,
    name: String,
    text: String,
    quiet: String? = null,
    sizeSp: Float = 17f,
    color: Color? = null,
    avatar: (@Composable () -> Unit)? = null,
) {
    val reduce = rememberReduceMotion()
    val words = remember(text) { text.split(Regex("\\s+")).filter { it.isNotEmpty() } }
    var shown by remember { mutableStateOf(0) }
    var lastText by remember { mutableStateOf("") }
    LaunchedEffect(text) {
        if (!text.startsWith(lastText)) shown = 0
        lastText = text
    }
    LaunchedEffect(text, shown) {
        if (!you && !reduce && shown < words.size) { delay(170); shown += 1 }
    }
    if (text.isEmpty() && quiet == null) return
    val visible = words.take(if (you || reduce) words.size else minOf(shown, words.size)).joinToString(" ")
    val chip = color ?: AllternitTheme.colors.accentPrimary
    val lineHeight = sizeSp * 1.38f
    // Rough check for more than two lines, for the top fade only (iOS wordsOverflow).
    val overflow = visible.length > maxOf(18, (300 / (sizeSp * 0.55f)).toInt()) * 2
    val band = RoundedCornerShape(16.dp)
    Column(
        Modifier.widthIn(max = 640.dp).fillMaxWidth().clip(band)
            .background(Brush.verticalGradient(listOf(Color(0xCC1C1C1F), Color(0xE0121214))))
            .border(1.dp, Color.White.copy(alpha = 0.08f), band)
            .padding(start = 14.dp, end = 14.dp, top = 10.dp, bottom = 12.dp).testTag("voiceCaptions")
            .clearAndSetSemantics { contentDescription = if (text.isEmpty()) quiet.orEmpty() else "${if (you) "You" else name}: $text" },
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(
            Modifier.height(22.dp).clip(CircleShape).background(if (you) Color.White.copy(alpha = 0.12f) else chip.copy(alpha = chip.alpha * 0.22f))
                .padding(start = if (you || avatar == null) 8.dp else 3.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (you) Box(Modifier.size(6.dp).clip(CircleShape).background(Color(0xFFFF5A4E)))
            else if (avatar != null) Box(Modifier.size(18.dp).clip(CircleShape).background(Color.White.copy(alpha = 0.92f)), contentAlignment = Alignment.Center) { avatar() }
            Text(
                (if (you) "You" else name).uppercase(),
                color = if (you) Color.White.copy(alpha = 0.85f) else lerp(chip.copy(alpha = 1f), Color.White, 0.45f),
                fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.9.sp, maxLines = 1,
            )
        }
        // Newest lines at the bottom; older ones roll off the top.
        Box(
            Modifier.fillMaxWidth().height((lineHeight * 2).dp).clipToBounds()
                .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
                .drawWithContent {
                    drawContent()
                    if (overflow) drawRect(Brush.verticalGradient(0f to Color.Transparent, 0.34f to Color.Black), blendMode = BlendMode.DstIn)
                },
            contentAlignment = Alignment.BottomStart,
        ) {
            if (text.isEmpty() && quiet != null) {
                Text("$quiet …", color = Color.White.copy(alpha = 0.55f), fontStyle = FontStyle.Italic, fontWeight = FontWeight.Medium, fontSize = (sizeSp * 0.88f).sp)
            } else {
                Text(
                    visible, color = if (you) Color.White.copy(alpha = 0.78f) else Color.White, fontSize = sizeSp.sp, lineHeight = lineHeight.sp,
                    fontWeight = FontWeight.Medium, modifier = Modifier.fillMaxWidth().wrapContentHeight(align = Alignment.Bottom, unbounded = true),
                )
            }
        }
    }
}

/** A stable id for test fixtures and keys. */
internal fun newVoiceId(): String = UUID.randomUUID().toString()
