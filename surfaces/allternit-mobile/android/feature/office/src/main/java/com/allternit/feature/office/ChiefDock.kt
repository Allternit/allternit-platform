package com.allternit.feature.office

import android.Manifest
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.str
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotRecord
import com.allternit.feature.bots.BotStatusAvatar
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.voice.Dictation
import com.allternit.feature.chat.voice.DictationOnboardingSheet
import com.allternit.feature.chat.voice.VoicePrefs
import com.allternit.feature.chat.voice.dictationOnboardingShown
import com.allternit.feature.chat.voice.hasMicPermission
import kotlinx.coroutines.launch

/**
 * The main bot (step 27): `mainBotId` from `/floor/state`, else the bot marked chief of staff
 * (`botProfile.chiefOfStaff`), else the `primary-a` template bot, else the account's primary bot. Nothing is
 * hard-coded to one bot.
 */
internal fun officeMainBot(store: BotModeStore, floor: FloorState?): BotRecord? =
    floor?.mainBotId?.let(store::bot)
        ?: store.bots.firstOrNull { it.isMainBot }
        ?: store.bots.firstOrNull { it.config.str("templateId") == "primary-a" }
        ?: store.bots.firstOrNull { it.isPrimaryTemplateBot }

/** Hold-to-talk: idle, held down (release sends), or toggled on by a tap (tap again sends). */
internal enum class MicMode { Idle, Holding, Toggled }

/** The dock mic's state over the app's dictation ([Dictation], the composer's own recognizer). */
internal class HoldToTalk(val dictation: Dictation) {
    var mode by mutableStateOf(MicMode.Idle)
    /** The finger moved off the mic while holding: release cancels. */
    var cancelling by mutableStateOf(false)
}

/**
 * The chief dock (step 27, iOS ChiefDockView; product shape C on phone), docked at the bottom of the Workspace floor:
 * the main bot's face and name (tap opens its thread sheet), a hold-to-talk mic (hold = record, release = send, slide
 * away = cancel; a tap toggles, which is also what TalkBack's double tap does), the Autopilot switch with its state and
 * an info control for the rules sheet, and a suggestion card ([OfficeApprovalCard], "Yes, do it" / "Not now") when the
 * main bot has an open suggestion. No main bot: "Pick your main bot".
 */
@Composable
fun ChiefDock(
    mainBot: BotRecord?,
    signedIn: Boolean,
    store: BotModeStore,
    backend: ChatBackend,
    autopilot: AutopilotModel,
    suggestion: OfficeNeed.Suggestion?,
    /** A line from the last send ("Sent to …", or why it failed), or null. */
    sendStatus: String?,
    onTalk: (String) -> Unit,
    onOpenMain: () -> Unit,
    onOpenAutopilot: () -> Unit,
    onPickMain: () -> Unit,
    onSignIn: () -> Unit,
    onSuggestionDone: (need: OfficeNeed, bubble: String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val context = LocalContext.current
    val talk = remember { HoldToTalk(Dictation(context.applicationContext)) }
    var micNote by remember { mutableStateOf<String?>(null) }
    var switchNote by remember { mutableStateOf<String?>(null) }
    DisposableEffect(talk) { onDispose { talk.dictation.onStopped = null; talk.dictation.stop() } }
    LaunchedEffect(talk) { snapshotFlow { talk.dictation.errorMessage }.collect { m -> if (m != null) micNote = m } }
    LaunchedEffect(sendStatus) { if (sendStatus != null) micNote = null }

    Column(
        modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 12.dp).padding(bottom = 12.dp).testTag("chiefDock"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        // The main bot's open suggestion, answered through the thread's approve / decline (BotModeStore.act).
        if (mainBot != null && suggestion != null && talk.mode == MicMode.Idle) {
            androidx.compose.runtime.key(suggestion.id) {
                OfficeApprovalCard(
                    suggestion, mainBot.displayName, backend, store, more = 0, onDone = onSuggestionDone,
                    modifier = Modifier.shadow(6.dp, RoundedCornerShape(16.dp), clip = false),
                    suggestionApproveTitle = "Yes, do it",
                )
            }
        }
        // What the mic hears, live.
        if (talk.mode != MicMode.Idle) {
            val heard = talk.dictation.transcript
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPrimary)
                    .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp)).padding(horizontal = 14.dp, vertical = 10.dp)
                    .semantics { liveRegion = LiveRegionMode.Polite }.testTag("chiefDockTranscript"),
            ) {
                Text(heard.ifEmpty { "Listening…" }, color = if (heard.isEmpty()) c.textTertiary else c.textPrimary, fontSize = 14.5.sp, maxLines = 4, overflow = TextOverflow.Ellipsis)
            }
        }
        Column(
            Modifier.fillMaxWidth().shadow(10.dp, RoundedCornerShape(24.dp), clip = false).clip(RoundedCornerShape(24.dp)).background(c.bgPrimary)
                .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(24.dp)),
        ) {
            when {
                !signedIn -> DockMessage(
                    "Sign in to talk to your main bot", "Your bots and Autopilot need your account.", "Sign in", "chiefDockSignIn", onSignIn,
                )
                mainBot == null && store.agents.isEmpty() && store.agentsLoading -> Row(
                    Modifier.fillMaxWidth().height(64.dp).padding(horizontal = 16.dp).testTag("chiefDockLoading"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Loading your main bot…", color = c.textSecondary, fontSize = 14.sp)
                }
                mainBot == null && store.agents.isEmpty() && store.agentsError != null -> DockMessage(
                    "Couldn't load your bots", store.agentsError.orEmpty(), "Try again", "chiefDockRetry",
                ) { scope.launch { store.refreshAgents() } }
                mainBot == null -> DockMessage(
                    "Pick your main bot", "It answers for your team and runs Autopilot.", "Pick", "chiefDockPickMain", onPickMain,
                )
                else -> {
                    val threads = store.threads(mainBot.id)
                    val waiting = threads.any { it.group == "waiting" }
                    val working = threads.any { it.group == "working" }
                    val line = when (talk.mode) {
                        MicMode.Holding -> if (talk.cancelling) "Release to cancel" else "Listening. Release to send."
                        MicMode.Toggled -> "Listening. Tap the mic to send."
                        MicMode.Idle -> micNote ?: sendStatus ?: "Hold the mic to talk to ${mainBot.displayName}"
                    }
                    Row(
                        Modifier.fillMaxWidth().padding(start = 12.dp, end = 10.dp, top = 10.dp, bottom = 8.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Row(
                            Modifier.weight(1f).clip(RoundedCornerShape(14.dp))
                                .clickable(onClickLabel = "Open ${mainBot.displayName}'s thread", role = Role.Button, onClick = onOpenMain)
                                .padding(2.dp).testTag("chiefDockMain"),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            BotStatusAvatar(mainBot, 40.dp, working = working && !waiting, waiting = waiting)
                            Column(Modifier.weight(1f)) {
                                Text(mainBot.displayName, color = c.textPrimary, fontSize = 15.5.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text(
                                    line, color = if (talk.cancelling) AllternitStatic.StatusError else c.textSecondary, fontSize = 12.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite }.testTag("chiefDockLine"),
                                )
                            }
                        }
                        ChiefMic(talk, mainBot.displayName, onSend = { text -> micNote = null; onTalk(text) }, onNote = { micNote = it })
                    }
                    // Autopilot: its state, the switch, and the rules sheet.
                    Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                    val st = autopilot.state
                    val rules = st?.rules?.size ?: 0
                    val stateLine = when {
                        switchNote != null -> switchNote!!
                        st != null -> if (st.enabled) "On · " + (if (rules == 1) "1 rule" else "$rules rules") else "Off"
                        autopilot.loading -> "Checking…"
                        autopilot.offline -> "Offline"
                        else -> autopilot.problem ?: "Unavailable"
                    }
                    Row(
                        Modifier.fillMaxWidth().height(48.dp).padding(start = 14.dp, end = 6.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        Icon(painterResource(DesignR.drawable.ph_sparkle), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
                        Text("Autopilot", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                        Text(
                            stateLine, color = if (switchNote != null) AllternitStatic.StatusError else c.textSecondary, fontSize = 12.5.sp,
                            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f).testTag("chiefDockAutopilotState"),
                        )
                        if (st == null && !autopilot.loading) {
                            Text(
                                "Try again", color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium,
                                modifier = Modifier.clip(CircleShape).clickable(role = Role.Button) { scope.launch { autopilot.load() } }
                                    .padding(horizontal = 10.dp, vertical = 8.dp).testTag("chiefDockAutopilotRetry"),
                            )
                        } else {
                            Switch(
                                checked = st?.enabled == true,
                                enabled = st != null && !autopilot.saving,
                                onCheckedChange = { on ->
                                    Haptics.perform(view, Haptics.Kind.Tap)
                                    switchNote = null
                                    scope.launch { autopilot.setEnabled(on)?.let { switchNote = it; Haptics.perform(view, Haptics.Kind.Warning) } }
                                },
                                colors = officeSwitchColors(),
                                modifier = Modifier.scale(0.85f).semantics { contentDescription = "Autopilot" }.testTag("chiefDockAutopilotSwitch"),
                            )
                        }
                        Box(
                            Modifier.size(40.dp).clip(CircleShape).clickable(onClickLabel = "Autopilot rules", role = Role.Button, onClick = onOpenAutopilot)
                                .semantics { contentDescription = "Autopilot rules" }.testTag("chiefDockAutopilotInfo"),
                            contentAlignment = Alignment.Center,
                        ) { Icon(painterResource(DesignR.drawable.ph_info), null, tint = c.textSecondary, modifier = Modifier.size(18.dp)) }
                    }
                }
            }
        }
    }
}

@Composable
private fun DockMessage(title: String, detail: String, action: String, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().clickable(onClickLabel = action, role = Role.Button, onClick = onClick).padding(horizontal = 16.dp, vertical = 12.dp).testTag(testTag),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.size(40.dp).clip(CircleShape).background(c.fillNeutral), contentAlignment = Alignment.Center) {
            Icon(painterResource(DesignR.drawable.ph_user_circle), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(detail, color = c.textSecondary, fontSize = 12.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Box(Modifier.clip(CircleShape).background(c.textPrimary).padding(horizontal = 14.dp, vertical = 8.dp)) {
            Text(action, color = c.bgPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium)
        }
    }
}

/**
 * The dock mic on the app's dictation path (the composer's [Dictation] on Android SpeechRecognizer, its first-use
 * [DictationOnboardingSheet], then the RECORD_AUDIO dialog). Press and hold records; release sends the words; sliding
 * more than 48 dp off the mic cancels. A quick tap leaves it listening until the next tap (or a 6.5 s silence), and
 * TalkBack's double tap does the same toggle through the semantics click.
 */
@Composable
private fun ChiefMic(talk: HoldToTalk, botName: String, onSend: (String) -> Unit, onNote: (String?) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val send by rememberUpdatedState(onSend)
    val note by rememberUpdatedState(onNote)
    var onboarding by remember { mutableStateOf(false) }
    val reduceMotion = remember(context) { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        note(if (granted) "Ready. Hold the mic to talk." else "Microphone access is off. Enable it in Settings to talk to $botName.")
    }
    val d = talk.dictation

    fun finish(sendIt: Boolean) {
        val was = talk.mode
        talk.mode = MicMode.Idle
        talk.cancelling = false
        d.stop()
        if (!sendIt || was == MicMode.Idle) return
        val text = d.transcript.trim()
        if (text.isEmpty()) note("Didn't catch that. Hold the mic and speak.") else { Haptics.perform(view, Haptics.Kind.Success); send(text) }
    }

    fun begin(next: MicMode): Boolean {
        if (!dictationOnboardingShown(context)) { onboarding = true; return false }
        if (!hasMicPermission(context)) { permission.launch(Manifest.permission.RECORD_AUDIO); return false }
        VoicePrefs.load(context)
        note(null)
        d.start()
        if (!d.isRecording) return false
        talk.mode = next
        Haptics.perform(view, Haptics.Kind.Tap)
        return true
    }

    // A silence or final result while toggled on sends what was heard; while held, release sends.
    LaunchedEffect(d) { d.onStopped = { if (talk.mode == MicMode.Toggled) finish(true) } }

    val on = talk.mode != MicMode.Idle
    val level by animateFloatAsState(if (on && !reduceMotion) 1f + d.micLevel * 0.18f else 1f, tween(if (reduceMotion) 0 else 90), label = "mic")
    Box(
        Modifier.size(52.dp)
            .semantics {
                role = Role.Button
                contentDescription = if (on) "Stop and send to $botName" else "Talk to $botName"
                stateDescription = if (on) "Listening" else "Hold to talk, or double tap to start"
                onClick(label = if (on) "Stop and send" else "Start talking") { if (on) finish(true) else begin(MicMode.Toggled); true }
            }
            .pointerInput(talk) {
                awaitEachGesture {
                    val down = awaitFirstDown()
                    down.consume()
                    if (talk.mode == MicMode.Toggled) {
                        // Toggled on: this tap stops and sends.
                        waitForUpOrCancellation()
                        finish(true)
                        return@awaitEachGesture
                    }
                    val t0 = down.uptimeMillis
                    if (!begin(MicMode.Holding)) { waitForUpOrCancellation(); return@awaitEachGesture }
                    val slop = 48.dp.toPx()
                    var up: PointerInputChange? = null
                    while (true) {
                        val ch = awaitPointerEvent().changes.firstOrNull { it.id == down.id } ?: break
                        if (!ch.pressed) { up = ch; break }
                        val p = ch.position
                        val away = p.x < -slop || p.y < -slop || p.x > size.width + slop || p.y > size.height + slop
                        if (away != talk.cancelling) { talk.cancelling = away; if (away) Haptics.perform(view, Haptics.Kind.Warning) }
                    }
                    val held = (up?.uptimeMillis ?: t0) - t0
                    when {
                        up == null -> finish(false)
                        talk.cancelling -> { finish(false); note("Cancelled. Nothing was sent.") }
                        // A quick tap: keep listening until the next tap.
                        held < 350 -> { talk.mode = MicMode.Toggled }
                        else -> finish(true)
                    }
                }
            }
            .testTag("chiefDockMic"),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            Modifier.size(48.dp).scale(level).clip(CircleShape)
                .background(if (talk.cancelling) c.fillNeutral else if (on) AllternitStatic.StatusError else c.textPrimary),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painterResource(if (on) DesignR.drawable.ph_microphone_fill else DesignR.drawable.ph_microphone), null,
                tint = if (talk.cancelling) c.textPrimary else if (on) Color.White else c.bgPrimary, modifier = Modifier.size(22.dp),
            )
        }
    }
    if (onboarding) {
        DictationOnboardingSheet(onDismiss = { onboarding = false }) {
            onboarding = false
            if (hasMicPermission(context)) note("Ready. Hold the mic to talk.") else permission.launch(Manifest.permission.RECORD_AUDIO)
        }
    }
}
