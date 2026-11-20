package com.allternit.feature.chat.voice

import com.allternit.core.design.AllternitSerif
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import androidx.compose.animation.animateColorAsState
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
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
 * The on-device sherpa-onnx engine is behind a feature check ([DeviceVoice]); without it the system route runs
 * with a notice, as on iOS with no voice pack.
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

    fun start(chat: ChatViewModel, runtimeModelId: String?, effort: String?, presentation: Presentation = Presentation.Full) {
        VoicePrefs.load(context)
        if (viewModel == null) {
            val choice = VoiceEngines.choose(context, VoicePrefs.interactionMode == VoiceInteractionMode.PushToTalk, dictation, speaker)
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

/**
 * iOS VoiceModeView (plain stage, no bot): the ambient gradient tinted by the turn state, the streamed reply
 * in serif (tap to interrupt), the live transcript in italics, and the controls row: settings, minimize,
 * the mic (tap mutes; hold talks in push to talk) and end.
 */
@Composable
fun VoiceModeView(host: VoiceHost, viewModel: VoiceModeViewModel, onEnd: (Int) -> Unit) {
    val c = AllternitTheme.colors
    var settingsOpen by remember { mutableStateOf(false) }
    val tintTarget = when (viewModel.state) {
        TurnState.Idle -> c.bgTertiary to c.bgSecondary
        TurnState.Listening -> Color(0xFF3B82F6) to Color(0xFF1E3A8A)
        TurnState.Thinking -> Color(0xFF7C8DB0) to Color(0xFF2E3A52)
        TurnState.Speaking -> c.accentPrimary to Color(0xFF7C3A1E)
    }
    val primary by animateColorAsState(tintTarget.first, tween(800), label = "voiceTint1")
    val secondary by animateColorAsState(tintTarget.second, tween(800), label = "voiceTint2")
    val animating = viewModel.state == TurnState.Listening || viewModel.state == TurnState.Speaking
    val pulse by rememberInfiniteTransition(label = "voicePulse").animateFloat(0f, 1f, infiniteRepeatable(tween(2200), RepeatMode.Reverse), label = "pulse")

    Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("voiceMode")) {
        Box(Modifier.size(420.dp).offset((-60).dp, 40.dp).blur(120.dp).clip(CircleShape)
            .background(primary.copy(alpha = if (animating) 0.3f + 0.2f * pulse else 0.25f)))
        Box(Modifier.size(460.dp).align(Alignment.BottomEnd).offset(70.dp, 60.dp).blur(130.dp).clip(CircleShape)
            .background(secondary.copy(alpha = if (animating) 0.25f + 0.2f * pulse else 0.2f)))
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding(), horizontalAlignment = Alignment.CenterHorizontally) {
            Spacer(Modifier.weight(1f))
            if (viewModel.replyText.isNotEmpty()) {
                Text(
                    viewModel.replyText,
                    color = c.textPrimary, fontSize = 22.sp, lineHeight = 30.sp, fontFamily = AllternitSerif, fontWeight = FontWeight.Medium,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.weight(2f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 36.dp)
                        .clickable(onClickLabel = "Interrupt") { viewModel.interrupt() }.testTag("voiceReply"),
                )
            } else {
                VoiceSpeakerRing(state = viewModel.state, muted = viewModel.isMuted, level = host.listener.micLevel, size = 120)
                Text(
                    viewModel.state.statusLabel(viewModel), color = c.textSecondary, fontSize = 22.sp, fontFamily = AllternitSerif,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.padding(horizontal = 40.dp, vertical = 12.dp).clickable { viewModel.interrupt() }.testTag("voiceStatus"),
                )
            }
            if (viewModel.canInterrupt) {
                Text("Tap to interrupt", color = c.textSecondary, fontSize = 12.sp,
                    modifier = Modifier.padding(top = 8.dp).clip(CircleShape).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 6.dp))
            }
            Spacer(Modifier.weight(1f))
            Text(viewModel.liveTranscript.ifEmpty { " " }, color = c.textPrimary.copy(alpha = 0.85f), fontStyle = FontStyle.Italic, fontSize = 16.sp,
                textAlign = TextAlign.Center, maxLines = 3, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = 32.dp).padding(bottom = 6.dp).testTag("voiceTranscript"))
            viewModel.statusLine?.let {
                Text(it, color = Color(0xFFD97706), fontSize = 12.sp, textAlign = TextAlign.Center, modifier = Modifier.padding(horizontal = 32.dp).padding(bottom = 6.dp))
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 36.dp).padding(bottom = 20.dp), verticalAlignment = Alignment.CenterVertically) {
                RoundControl(com.allternit.core.design.R.drawable.ph_gear, "Voice settings", "voiceSettings") { settingsOpen = true }
                Spacer(Modifier.size(12.dp))
                RoundControl(com.allternit.core.design.R.drawable.ph_arrows_in, "Minimize to voice bar", "voiceMinimize") { host.presentation = VoiceHost.Presentation.Bar }
                Spacer(Modifier.weight(1f))
                MicButton(viewModel)
                Spacer(Modifier.weight(1f))
                Spacer(Modifier.size(56.dp))
                RoundControl(DesignR.drawable.ph_x, "End voice chat", "voiceEnd") { host.end()?.let(onEnd) }
            }
        }
    }
    if (settingsOpen) VoiceSettingsSheet(host.speaker) { settingsOpen = false }
}

@Composable
private fun MicButton(vm: VoiceModeViewModel) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val listening = vm.state == TurnState.Listening
    Box(
        Modifier.size(72.dp).clip(CircleShape).background(if (vm.isMuted) c.bgSecondary else Color.White)
            .border(if (listening) 3.dp else 0.dp, Color(0xFF3B82F6).copy(alpha = 0.5f), CircleShape)
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
        Icon(painterResource(if (vm.isMuted) com.allternit.core.design.R.drawable.ph_microphone_slash else com.allternit.core.design.R.drawable.ph_microphone), null,
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
 * iOS VoiceSpeakerView, the ring style: a ring that breathes with the mic level while listening, with a
 * syllable rhythm while speaking, and turns slowly while thinking or connecting. Muted draws it grey and still.
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

/** iOS VoiceQuickBar: the minimized session above the composer (mute, expand, end). */
@Composable
fun VoiceQuickBar(host: VoiceHost, viewModel: VoiceModeViewModel, onEnd: (Int) -> Unit) {
    val c = AllternitTheme.colors
    val status = when (viewModel.state) {
        TurnState.Idle -> if (viewModel.isMuted) "Muted" else "Voice paused"
        TurnState.Listening -> viewModel.liveTranscript.ifEmpty { "Listening…" }
        TurnState.Thinking -> "Thinking…"
        TurnState.Speaking -> "Speaking…"
    }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp).clip(RoundedCornerShape(AllternitRadii.MD))
            .background(c.bgPrimary).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.MD))
            .padding(horizontal = 12.dp, vertical = 8.dp).testTag("voiceQuickBar")
            .semantics { contentDescription = "Voice chat, $status" },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        VoiceSpeakerRing(viewModel.state, viewModel.isMuted, host.listener.micLevel, size = 22)
        Column(Modifier.weight(1f)) {
            Text(status, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            (viewModel.statusLine ?: viewModel.engineLabel)?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, maxLines = 1) }
        }
        RoundControl(if (viewModel.isMuted) com.allternit.core.design.R.drawable.ph_microphone_slash else com.allternit.core.design.R.drawable.ph_microphone,
            if (viewModel.isMuted) "Unmute microphone" else "Mute microphone", "voiceBarMute") { viewModel.toggleMute() }
        RoundControl(com.allternit.core.design.R.drawable.ph_arrows_out, "Open full-screen voice", "voiceBarExpand") { host.presentation = VoiceHost.Presentation.Full }
        RoundControl(DesignR.drawable.ph_x, "End voice chat", "voiceBarEnd") { host.end()?.let(onEnd) }
    }
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
 * iOS VoiceCaptionsView: the dark caption band (who is talking, then the words, two lines, revealed word by
 * word for the agent and all at once for you). Used by voice mode's bot stage and by the meeting live view.
 */
@Composable
fun VoiceCaptions(you: Boolean, name: String, text: String, quiet: String? = null, sizeSp: Float = 17f) {
    val words = remember(text) { text.split(Regex("\\s+")).filter { it.isNotEmpty() } }
    var shown by remember { mutableStateOf(0) }
    var lastText by remember { mutableStateOf("") }
    LaunchedEffect(text) {
        if (!text.startsWith(lastText)) shown = 0
        lastText = text
    }
    LaunchedEffect(text, shown) {
        if (!you && shown < words.size) { delay(170); shown += 1 }
    }
    if (text.isEmpty() && quiet == null) return
    val visible = words.take(if (you) words.size else minOf(shown, words.size)).joinToString(" ")
    Column(
        Modifier.widthIn(max = 640.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(Color(0xE01C1C1F))
            .border(1.dp, Color.White.copy(alpha = 0.08f), RoundedCornerShape(16.dp))
            .padding(start = 14.dp, end = 14.dp, top = 10.dp, bottom = 12.dp).testTag("voiceCaptions")
            .semantics { contentDescription = if (text.isEmpty()) quiet.orEmpty() else "${if (you) "You" else name}: $text" },
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(Modifier.height(22.dp).clip(CircleShape).background(if (you) Color.White.copy(alpha = 0.12f) else Color(0x38D97757)).padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            if (you) Box(Modifier.size(6.dp).clip(CircleShape).background(Color(0xFFFF5A4E)))
            Text((if (you) "You" else name).uppercase(), color = Color.White.copy(alpha = 0.85f), fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.9.sp)
        }
        if (text.isEmpty() && quiet != null) {
            Text("$quiet …", color = Color.White.copy(alpha = 0.55f), fontStyle = FontStyle.Italic, fontSize = (sizeSp * 0.88f).sp)
        } else {
            Text(visible, color = if (you) Color.White.copy(alpha = 0.78f) else Color.White, fontSize = sizeSp.sp, lineHeight = (sizeSp * 1.38f).sp,
                fontWeight = FontWeight.Medium, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** A stable id for test fixtures and keys. */
internal fun newVoiceId(): String = UUID.randomUUID().toString()
