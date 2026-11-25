package com.allternit.feature.meetings

import android.content.ClipboardManager
import android.content.Context
import android.net.Uri
import androidx.activity.compose.BackHandler
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.billing.StoreKind
import com.allternit.feature.billing.StorePurchaseSheet
import com.allternit.feature.chat.voice.TurnState
import com.allternit.feature.chat.voice.VoiceSpeakerRing
import java.time.Instant
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** What Meetings needs from the shell. */
class MeetingsHost(
    val signedIn: Boolean,
    val onClose: () -> Unit,
    /** "Get a cloud computer" (iOS DeepLinkRouter .computers). */
    val openComputers: () -> Unit,
    /** Settings › Meetings (calendars, auto-join, Zoom). */
    val openMeetingsSettings: () -> Unit,
)

private sealed interface MeetingRoute {
    data class Live(val sid: String, val meeting: Meeting?) : MeetingRoute
    data class Recap(val meeting: Meeting, val session: MeetingSession?) : MeetingRoute
}

private fun openUrl(context: Context, url: String) {
    runCatching { CustomTabsIntent.Builder().build().launchUrl(context, Uri.parse(url)) }
}

/**
 * iOS MeetingsView: Gizzi in your calls. The runner note (which of your own computers joins), live cards,
 * the agenda by day with a Gizzi switch per call, and Past with recaps. Paste a link to send Gizzi now.
 * The bot runs on the person's own computer (or their cloud computer), never on Allternit's servers.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MeetingsScreen(host: MeetingsHost, focusMeetingId: String? = null) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val store = MeetingsStore
    val noticeContext = androidx.compose.ui.platform.LocalContext.current
    androidx.compose.runtime.LaunchedEffect(Unit) { MeetingsStore.installNotifier(noticeContext) }
    var route by remember { mutableStateOf<MeetingRoute?>(null) }
    var pasteOpen by remember { mutableStateOf(false) }
    var sendTarget by remember { mutableStateOf<Meeting?>(null) }
    var refreshing by remember { mutableStateOf(false) }

    LaunchedEffect(Unit) {
        store.load(host.signedIn, quiet = store.phase == MeetingsStore.Phase.Loaded)
        focusMeetingId?.let { id ->
            val m = (store.live + store.upcoming + store.past).firstOrNull { it.id == id } ?: runCatching { MeetingsClient.meeting(id) }.getOrNull()
            if (m != null) {
                val sid = store.session(m)?.id ?: m.sessionId
                if (m.status.isActive && sid != null) route = MeetingRoute.Live(sid, m)
                else if (m.status == GizziStatus.Ended || m.status == GizziStatus.NotAdmitted) route = MeetingRoute.Recap(m, store.session(m))
            }
        }
    }

    when (val r = route) {
        is MeetingRoute.Live -> {
            BackHandler { route = null }
            MeetingLiveScreen(r.sid, r.meeting, onBack = { route = null }, onRecap = { m, s -> route = MeetingRoute.Recap(m, s) })
            return
        }
        is MeetingRoute.Recap -> {
            BackHandler { route = null }
            MeetingRecapScreen(r.meeting, r.session, onBack = { route = null })
            return
        }
        null -> BackHandler { host.onClose() }
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("meetingsScreen")) {
        Header("Meetings", leadingIcon = DesignR.drawable.ph_x, leadingLabel = "Close meetings", onLeading = host.onClose,
            trailing = if (store.phase == MeetingsStore.Phase.Loaded) ({ RoundButton(DesignR.drawable.ph_link_simple, "Paste a call link", "meetingsPasteLink") { pasteOpen = true } }) else null)
        PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; store.load(host.signedIn, quiet = true); refreshing = false } }, modifier = Modifier.weight(1f)) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 32.dp).navigationBarsPadding()) {
                val empty = store.live.isEmpty() && store.upcoming.isEmpty() && store.past.isEmpty()
                when (val p = store.phase) {
                    MeetingsStore.Phase.Idle -> Skeleton()
                    MeetingsStore.Phase.Loading -> if (empty) Skeleton() else Agenda(host, onPaste = { pasteOpen = true }, onSend = { sendTarget = it }, onRoute = { route = it })
                    MeetingsStore.Phase.SignedOut -> MeetingsState(FriendlyStyle.Empty, "Sign in to use Meetings", "Gizzi joins your calls, takes notes and makes what you ask for, once you're signed in.")
                    MeetingsStore.Phase.PaidPlan -> PaidPlanCard()
                    is MeetingsStore.Phase.Forbidden -> MeetingsState(FriendlyStyle.Error, "Meetings is off for your organization", p.message)
                    is MeetingsStore.Phase.Failed -> if (empty) MeetingsState(FriendlyStyle.Offline, "Couldn't load meetings", p.message, actionTitle = "Retry", onAction = { scope.launch { store.load(host.signedIn) } })
                        else Agenda(host, onPaste = { pasteOpen = true }, onSend = { sendTarget = it }, onRoute = { route = it })
                    MeetingsStore.Phase.Loaded -> Agenda(host, onPaste = { pasteOpen = true }, onSend = { sendTarget = it }, onRoute = { route = it })
                }
            }
        }
    }

    if (pasteOpen) PasteLinkSheet(host, onDismiss = { pasteOpen = false }) { m, s -> pasteOpen = false; if (s != null) route = MeetingRoute.Live(s.id, m) }
    sendTarget?.let { m ->
        val context = LocalContext.current
        ModalBottomSheet(onDismissRequest = { sendTarget = null }, containerColor = c.bgPrimary) {
            Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp).padding(bottom = 16.dp).testTag("meetingSendSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(m.displayTitle, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text("Gizzi joins as “Gizzi (Allternit)” and posts a notice in the call chat.", color = c.textSecondary, fontSize = 13.sp)
                SheetButton("Send Gizzi now", primary = true, tag = "meetingSendNow") {
                    sendTarget = null
                    scope.launch { store.sendNow(m)?.let { route = MeetingRoute.Live(it.id, m) } }
                }
                if (m.joinUrl.isNotEmpty()) SheetButton("Open in ${m.platformName}", tag = "meetingOpenCall") { sendTarget = null; openUrl(context, m.joinUrl) }
                SheetButton("Cancel", tag = "meetingSendCancel") { sendTarget = null }
            }
        }
    }
    store.actionError?.let { msg ->
        AlertDialog(onDismissRequest = { store.actionError = null }, title = { Text("Meetings") }, text = { Text(msg) },
            confirmButton = { TextButton(onClick = { store.actionError = null }) { Text("OK") } })
    }
}

@Composable
private fun Agenda(host: MeetingsHost, onPaste: () -> Unit, onSend: (Meeting) -> Unit, onRoute: (MeetingRoute) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val store = MeetingsStore
    store.runner?.let { RunnerNote(it, host.openComputers) }
    store.live.forEach { m ->
        LiveCard(m, store.session(m)) { (store.session(m)?.id ?: m.sessionId)?.let { onRoute(MeetingRoute.Live(it, m)) } }
    }
    if (store.upcoming.isEmpty() && store.live.isEmpty()) {
        Column(Modifier.fillMaxWidth().padding(vertical = 28.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("No calls coming up", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text("Calls with a Meet, Zoom or Teams link on your calendar show here. Connect a calendar in Meetings settings, or paste a call link to send Gizzi now.",
                color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp, textAlign = androidx.compose.ui.text.style.TextAlign.Center)
            SheetButton("Paste a call link", primary = true, tag = "meetingsEmptyPaste", onClick = onPaste)
            Text("Meetings settings", color = c.accentPrimary, fontSize = 14.sp, modifier = Modifier.clickable(onClick = host.openMeetingsSettings).padding(8.dp).testTag("meetingsOpenSettings"))
        }
    }
    val groups = store.upcoming.groupBy { MeetingFormat.dayLabel(it.startDate) }
    groups.forEach { (label, rows) ->
        SectionLabel(label)
        Card {
            rows.forEachIndexed { i, m ->
                if (i > 0) Divider()
                AgendaRow(m, pending = m.id in store.pendingSwitches, onToggle = { on -> scope.launch { store.setGizzi(m, on) } }, onTap = { onSend(m) })
            }
        }
    }
    if (store.past.isNotEmpty()) {
        SectionLabel("Past")
        Card {
            store.past.take(30).forEachIndexed { i, m ->
                if (i > 0) Divider()
                PastRow(m, store.session(m)) { onRoute(MeetingRoute.Recap(m, store.session(m))) }
            }
        }
    }
}

@Composable
private fun RunnerNote(runner: MeetingRunner, openComputers: () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(top = 12.dp).clip(RoundedCornerShape(AllternitRadii.MD)).background(c.fillNeutral).padding(14.dp).testTag("meetingsRunnerNote"),
        verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(runner.text, color = c.textSecondary, fontSize = 13.sp, lineHeight = 18.sp)
        if (runner.isNone) Text(MeetingRunner.NONE_ACTION, color = c.accentPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(onClick = openComputers))
    }
}

@Composable
private fun LiveCard(m: Meeting, s: MeetingSession?, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val status = s?.status ?: m.status
    val detail = when {
        status == GizziStatus.Live -> (if (s?.listening != false) "Gizzi is listening" else "Gizzi stopped listening") + " · ${m.platformName}"
        status == GizziStatus.Paused -> "Gizzi stopped listening · ${m.platformName}"
        status == GizziStatus.WaitingAdmit && m.platform == "zoom" -> "Join the Zoom call so Gizzi can come in"
        status == GizziStatus.WaitingAdmit -> "Let “Gizzi (Allternit)” in from ${m.platformName}"
        else -> "Gizzi is joining ${m.platformName}"
    }
    Row(Modifier.fillMaxWidth().padding(top = 12.dp).clip(RoundedCornerShape(AllternitRadii.LG)).background(c.bgPrimary).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.LG))
        .clickable(onClickLabel = "Opens the live meeting", onClick = onOpen).padding(14.dp).testTag("meetingLiveCard"),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        VoiceSpeakerRing(if (status == GizziStatus.Live) TurnState.Listening else TurnState.Thinking, muted = status == GizziStatus.Paused, level = 0.2f, size = 32)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(m.displayTitle, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            StateChip(status, s, m)
            Text(detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 2)
        }
        Text("Open", color = c.onAccent, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clip(CircleShape).background(c.accentPrimary).padding(horizontal = 14.dp, vertical = 7.dp))
    }
}

@Composable
private fun AgendaRow(m: Meeting, pending: Boolean, onToggle: (Boolean) -> Unit, onTap: () -> Unit) {
    val c = AllternitTheme.colors
    val subtitle = buildList {
        add(m.platformName)
        m.peopleLabel?.takeIf { it.isNotEmpty() }?.let { add(it) }
        if (!m.gizziOn && m.isOneOnOne == true && m.autoJoin == null) add("Skipped: 1:1")
    }.joinToString(" · ")
    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.weight(1f).clickable(onClickLabel = "Send Gizzi now or open the call", onClick = onTap), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(MeetingFormat.time(m.startDate), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.width(64.dp))
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(m.displayTitle, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                Text(subtitle, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
            }
        }
        Switch(checked = m.gizziOn, onCheckedChange = onToggle, enabled = !pending,
            colors = SwitchDefaults.colors(checkedTrackColor = c.brandAccent),
            modifier = Modifier.testTag("meetingSwitch-${m.id}").semantics { contentDescription = "Gizzi joins ${m.displayTitle}" })
    }
}

@Composable
private fun PastRow(m: Meeting, s: MeetingSession?, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().clickable(onClickLabel = "Opens the recap", onClick = onOpen).padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(MeetingFormat.shortDay(m.startDate ?: s?.endedDate), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.width(64.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(m.displayTitle, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
            Text(MeetingFormat.recapLine(m, s), color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
        }
        Text("Recap", color = c.accentPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun StateChip(status: GizziStatus, s: MeetingSession?, m: Meeting? = null) {
    val tint = when {
        status == GizziStatus.Live -> Color(0xFF16A34A)
        status == GizziStatus.Launching || status == GizziStatus.Joining || status == GizziStatus.WaitingAdmit -> Color(0xFFD97706)
        status == GizziStatus.NotAdmitted -> Color(0xFFDC2626)
        status == GizziStatus.Ended && s?.summary != null -> AllternitTheme.colors.brandAccent
        else -> AllternitTheme.colors.textTertiary
    }
    var now by remember { mutableStateOf(Instant.now()) }
    LaunchedEffect(status) { while (status == GizziStatus.Live) { delay(30_000); now = Instant.now() } }
    val since = s?.liveDate ?: m?.liveDate
    val label = if (status == GizziStatus.Live && since != null) "Live · ${MeetingFormat.minutes(since, now)}" else MeetingFormat.stateLabel(status, s, m?.platform)
    Row(Modifier.clip(CircleShape).background(tint.copy(alpha = 0.12f)).padding(horizontal = 8.dp, vertical = 3.dp).testTag("meetingStateChip"),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
        Box(Modifier.size(6.dp).clip(CircleShape).background(tint))
        Text(label, color = tint, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** iOS MeetingsPaidPlanCard: Meetings is part of the paid plans; Choose a plan opens the store sheet (Play Billing). */
@Composable
private fun PaidPlanCard() {
    val c = AllternitTheme.colors
    var store by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(top = 12.dp).clip(RoundedCornerShape(AllternitRadii.LG)).background(c.fillNeutral).padding(16.dp).testTag("meetingsPaidPlan"),
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Gizzi in your calls", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
        Text("Meetings is part of the Plus, Max and Ultra plans. Gizzi joins your Meet, Zoom and Teams calls, writes the transcript, answers in the chat and sends a recap.",
            color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp)
        SheetButton("Choose a plan", primary = true, tag = "meetingsChoosePlan") { store = true }
    }
    if (store) StorePurchaseSheet(focus = StoreKind.Plan, onDismiss = { store = false })
}

@Composable
private fun Skeleton() {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(top = 16.dp).alpha(0.7f).semantics { contentDescription = "Loading meetings" }, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        repeat(4) { Box(Modifier.fillMaxWidth().height(56.dp).clip(RoundedCornerShape(AllternitRadii.MD)).background(c.fillNeutral)) }
    }
}

/** iOS MeetingPasteLinkSheet. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun PasteLinkSheet(host: MeetingsHost, onDismiss: () -> Unit, onDone: (Meeting, MeetingSession?) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var link by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf<Boolean?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val valid = MeetingFormat.isCallLink(link)
    fun submit(joinNow: Boolean) {
        busy = joinNow; error = null
        scope.launch {
            try { val (m, s) = MeetingsStore.addLink(link, joinNow, host.signedIn); onDone(m, s) }
            catch (e: MeetingsException) {
                if (e.error == MeetingsError.PaidPlan) { MeetingsStore.handle(e.error); onDismiss() } else error = e.error.message
            } finally { busy = null }
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp).padding(bottom = 16.dp).testTag("meetingPasteSheet"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Cancel", color = c.accentPrimary, fontSize = 16.sp, modifier = Modifier.clickable(onClick = onDismiss).padding(end = 12.dp))
                Text("Paste a call link", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            }
            Text("Paste a Google Meet, Zoom or Microsoft Teams link. Gizzi joins as “Gizzi (Allternit)” and posts a notice in the call chat.", color = c.textSecondary, fontSize = 14.sp, lineHeight = 20.sp)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.weight(1f).clip(RoundedCornerShape(AllternitRadii.MD)).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 12.dp)) {
                    if (link.isEmpty()) Text("https://meet.google.com/abc-defg-hij", color = c.textTertiary, fontSize = 15.sp)
                    BasicTextField(link, { link = it }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), modifier = Modifier.fillMaxWidth().testTag("meetingLinkField"))
                }
                Text("Paste", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.clip(CircleShape).background(c.fillNeutral).clickable {
                    (context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager)?.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString()?.let { link = it.trim() }
                }.padding(horizontal = 14.dp, vertical = 10.dp))
            }
            if (link.isNotEmpty() && !valid) Text("That isn't a Meet, Zoom or Teams link.", color = Color(0xFFDC2626), fontSize = 13.sp)
            if (error == null) MeetingsStore.runner?.takeIf { it.isNone }?.let { RunnerNote(it, host.openComputers) }
            error?.let { Text(it, color = Color(0xFFDC2626), fontSize = 13.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.SM)).background(Color(0x14DC2626)).padding(10.dp)) }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.weight(1f)) { SheetButton(if (busy == true) "Sending…" else "Send Gizzi now", primary = true, enabled = valid && busy == null, tag = "meetingSendNowLink") { submit(true) } }
                Box(Modifier.weight(1f)) { SheetButton(if (busy == false) "Saving…" else "Save for later", enabled = valid && busy == null, tag = "meetingSaveLink") { submit(false) } }
            }
        }
    }
}

/** iOS MeetingLiveView: the stage (Gizzi's ring and state), Transcript / Artifacts / Approvals, and the controls. */
@Composable
fun MeetingLiveScreen(sessionId: String, meeting: Meeting?, onBack: () -> Unit, onRecap: (Meeting, MeetingSession?) -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val live = remember(sessionId) { MeetingLiveSession(sessionId, meeting) }
    var tab by remember { mutableStateOf(0) }
    var confirmLeave by remember { mutableStateOf(false) }
    LaunchedEffect(live) { live.run() }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("meetingLive")) {
        Header(live.meeting?.displayTitle ?: "Meeting", leadingIcon = DesignR.drawable.ph_caret_left, leadingLabel = "Back", onLeading = onBack,
            trailing = if (live.session != null) ({ StateChip(live.status, live.session, live.meeting) }) else null)
        when (val p = live.phase) {
            MeetingLiveSession.Phase.Loading -> if (live.session == null) { Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = c.textSecondary) }; return@Column }
            is MeetingLiveSession.Phase.Failed -> if (live.session == null) { MeetingsState(FriendlyStyle.Offline, "Couldn't open the meeting", p.m, actionTitle = "Retry", onAction = { scope.launch { live.loadAll() } }); return@Column }
            MeetingLiveSession.Phase.PaidPlan -> { Box(Modifier.padding(16.dp)) { PaidPlanCard() }; return@Column }
            MeetingLiveSession.Phase.SignedOut -> { MeetingsState(FriendlyStyle.Empty, "Sign in to use Meetings", "Your session ended. Sign in again to see this meeting."); return@Column }
            MeetingLiveSession.Phase.Ready -> {}
        }
        val platform = live.meeting?.platformName ?: "the call"
        val zoom = live.meeting?.platform == "zoom"
        val word = when (live.status) {
            GizziStatus.Live -> if (live.session?.listening != false) "Listening" else "Paused"
            GizziStatus.Paused -> "Paused"
            GizziStatus.WaitingAdmit -> if (zoom) "Waiting for you to join Zoom" else "Waiting to be let in"
            GizziStatus.NotAdmitted -> "Not let in"
            GizziStatus.Ended -> if (live.session?.summary != null) "Recap ready" else "Left the call"
            else -> "Joining"
        }
        val detail = when (live.status) {
            GizziStatus.Live -> live.lastReply?.let { "Gizzi: $it" } ?: "Say “Gizzi, …” in $platform to ask for something."
            GizziStatus.Paused -> "Gizzi is in the call but not listening."
            GizziStatus.WaitingAdmit -> if (zoom) "Zoom lets Gizzi in once you're in the call. Join it from Zoom." else "Let “Gizzi (Allternit)” in from $platform."
            GizziStatus.NotAdmitted -> MeetingFormat.endReason(live.session?.endReason ?: "not_admitted")
            GizziStatus.Ended -> if (live.session?.summary != null) "Summary, decisions and action items are ready." else MeetingFormat.endReason(live.session?.endReason)
            else -> "Gizzi is joining $platform."
        }
        val phase = when (live.status) {
            GizziStatus.Live -> if (live.lastReply != null && live.session?.speakReplies == true) TurnState.Speaking else TurnState.Listening
            GizziStatus.Launching, GizziStatus.Joining, GizziStatus.WaitingAdmit, GizziStatus.Scheduled -> TurnState.Thinking
            else -> TurnState.Idle
        }
        Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            VoiceSpeakerRing(phase, muted = live.status == GizziStatus.Paused, level = 0.3f, size = 72, connecting = phase == TurnState.Thinking)
            Text(word.uppercase(), color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 1.6.sp)
            Text(detail, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 24.dp), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp).clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(3.dp)) {
            listOf("Transcript", "Artifacts", if (live.approvals.isEmpty()) "Approvals" else "Approvals · ${live.approvals.size}").forEachIndexed { i, label ->
                Text(label, color = c.textPrimary, fontSize = 13.sp, fontWeight = if (tab == i) FontWeight.SemiBold else FontWeight.Normal,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    modifier = Modifier.weight(1f).clip(RoundedCornerShape(8.dp)).background(if (tab == i) c.bgPrimary else Color.Transparent).clickable { tab = i }.padding(vertical = 7.dp).testTag("meetingTab$i"))
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (tab) {
                0 -> {
                    val listState = rememberLazyListState()
                    LaunchedEffect(live.transcript.size) { if (live.transcript.isNotEmpty()) listState.animateScrollToItem(live.transcript.size - 1) }
                    LazyColumn(state = listState, modifier = Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        if (live.transcript.isEmpty()) item { PanelEmpty(if (live.status == GizziStatus.Live) "The transcript shows here as people talk." else "No transcript yet.", com.allternit.core.design.EmptyArtKind.Envelope) }
                        items(live.transcript, key = { it.seq }) { s ->
                            Column(Modifier.padding(top = 4.dp)) {
                                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                    Text(s.speaker ?: "Someone", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                                    Text("%d:%02d".format(s.startMs / 60000, (s.startMs / 1000) % 60), color = c.textTertiary, fontSize = 12.sp)
                                }
                                Text(s.text, color = c.textSecondary, fontSize = 15.sp)
                            }
                        }
                    }
                }
                1 -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (live.artifacts.isEmpty()) PanelEmpty("Docs, slides and canvases Gizzi makes in the call show here.", com.allternit.core.design.EmptyArtKind.Sheets)
                    live.artifacts.forEach { a -> ArtifactRow(a) { openUrl(context, a.url) } }
                }
                else -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (live.approvals.isEmpty()) PanelEmpty("When someone asks Gizzi to use one of your tools, you approve it here. Names in a call aren't verified.", com.allternit.core.design.EmptyArtKind.Bell)
                    live.approvals.forEach { a ->
                        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.MD)).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.MD)).padding(14.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(a.argsSummary, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            Text(listOfNotNull(a.askedBy?.let { "Asked by $it" }, a.tool).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Box(Modifier.weight(1f)) { SheetButton("Allow", primary = true, tag = "meetingApprove-${a.id}") { scope.launch { live.decide(a, true) } } }
                                Box(Modifier.weight(1f)) { SheetButton("Deny", tag = "meetingDeny-${a.id}") { scope.launch { live.decide(a, false) } } }
                            }
                        }
                    }
                }
            }
        }
        if (live.isEnded) {
            if (live.session?.summary != null) Box(Modifier.padding(16.dp)) {
                SheetButton("See the recap", primary = true, tag = "meetingSeeRecap") { live.meeting?.let { onRecap(it, live.session) } }
            }
        } else {
            Row(Modifier.fillMaxWidth().padding(horizontal = 40.dp, vertical = 14.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                val paused = live.status == GizziStatus.Paused
                ControlButton(if (paused) com.allternit.core.design.R.drawable.ph_microphone else com.allternit.core.design.R.drawable.ph_microphone_slash, if (paused) "Resume" else "Stop listening", active = paused,
                    busy = live.busyAction == "stop_listening" || live.busyAction == "resume", enabled = live.status == GizziStatus.Live || paused) { scope.launch { live.toggleListening() } }
                val speak = live.session?.speakReplies == true
                ControlButton(if (speak) com.allternit.core.design.R.drawable.ph_speaker_high else com.allternit.core.design.R.drawable.ph_speaker_slash, "Speak replies", active = speak, busy = live.busyAction == "speak", enabled = live.session != null) {
                    scope.launch { live.setSpeakReplies(!speak) }
                }
                ControlButton(com.allternit.core.design.R.drawable.ph_phone_x, "Leave", destructive = true, busy = live.busyAction == "leave", enabled = true) { confirmLeave = true }
            }
        }
    }
    if (confirmLeave) {
        AlertDialog(onDismissRequest = { confirmLeave = false }, title = { Text("Gizzi leaves the call") }, text = { Text("The transcript so far is kept and the recap is written.") },
            confirmButton = { TextButton(onClick = { confirmLeave = false; scope.launch { live.leave() } }) { Text("Leave", color = Color(0xFFDC2626)) } },
            dismissButton = { TextButton(onClick = { confirmLeave = false }) { Text("Cancel") } })
    }
    live.actionError?.let { msg ->
        AlertDialog(onDismissRequest = { live.actionError = null }, title = { Text("Meetings") }, text = { Text(msg) },
            confirmButton = { TextButton(onClick = { live.actionError = null }) { Text("OK") } })
    }
}

/** iOS MeetingRecapView: summary, decisions, action items, what was made in the call, and the transcript on demand. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MeetingRecapScreen(meeting: Meeting, initialSession: MeetingSession?, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var session by remember { mutableStateOf(initialSession ?: MeetingsStore.session(meeting)) }
    var artifacts by remember { mutableStateOf<List<MeetingArtifact>>(emptyList()) }
    var transcript by remember { mutableStateOf<List<TranscriptSegment>?>(null) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var paid by remember { mutableStateOf(false) }
    var refreshing by remember { mutableStateOf(false) }
    suspend fun load() {
        loading = true
        val sid = session?.id ?: meeting.sessionId
        if (sid != null) {
            try {
                val (s, m, _) = MeetingsClient.session(sid)
                session = s; MeetingsStore.didUpdate(s, m)
                artifacts = runCatching { MeetingsClient.artifacts(sid) }.getOrDefault(artifacts)
                error = null
            } catch (e: MeetingsException) { if (e.error == MeetingsError.PaidPlan) paid = true else error = e.error.message }
        }
        loading = false
    }
    LaunchedEffect(meeting.id) { load() }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("meetingRecap")) {
        Header("Recap", leadingIcon = DesignR.drawable.ph_caret_left, leadingLabel = "Back", onLeading = onBack, trailing = null)
        PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; load(); refreshing = false } }, modifier = Modifier.weight(1f)) {
            Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 32.dp).navigationBarsPadding()) {
                Text(meeting.displayTitle, color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
                val parts = mutableListOf(meeting.platformName)
                val s0 = session
                if (s0?.liveDate != null && s0.endedDate != null) parts += MeetingFormat.minutes(s0.liveDate!!, s0.endedDate!!)
                (meeting.startDate ?: s0?.liveDate)?.let { parts += DateTimeFormatter.ofPattern("EEE, MMM d, h:mm a", Locale.getDefault()).format(it.atZone(java.time.ZoneId.systemDefault())) }
                Text(parts.joinToString(" · "), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 4.dp))
                s0?.let { Box(Modifier.padding(top = 6.dp)) { StateChip(it.status, it) } }
                when {
                    paid -> PaidPlanCard()
                    loading && session == null -> Box(Modifier.fillMaxWidth().padding(top = 40.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = c.textSecondary) }
                    error != null && session == null -> MeetingsState(FriendlyStyle.Offline, "Couldn't load the recap", error!!, actionTitle = "Retry", onAction = { scope.launch { load() } })
                    session == null -> Text("Gizzi wasn't in this call, so there's no recap.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(top = 16.dp))
                    else -> {
                        val s = session!!
                        if (s.summary != null) {
                            SectionLabel("Summary")
                            Text(s.summary, color = c.textPrimary, fontSize = 15.sp, lineHeight = 22.sp)
                            if (!s.decisions.isNullOrEmpty()) {
                                SectionLabel("Decisions")
                                Card { s.decisions.forEachIndexed { i, d -> if (i > 0) Divider(); BulletRow(d, null) } }
                            }
                            if (!s.actionItems.isNullOrEmpty()) {
                                SectionLabel("Action items")
                                Card {
                                    s.actionItems.forEachIndexed { i, item ->
                                        if (i > 0) Divider()
                                        val due = item.due?.let { d -> parseDate(d)?.let { "Due " + DateTimeFormatter.ofPattern("MMM d", Locale.getDefault()).format(it.atZone(java.time.ZoneId.systemDefault())) } ?: d.takeIf { it.isNotEmpty() }?.let { "Due $it" } }
                                        BulletRow(item.text, listOfNotNull(item.owner, due).joinToString(" · "))
                                    }
                                }
                            }
                        } else {
                            SectionLabel(if (s.status == GizziStatus.Ended) "No recap" else "In progress")
                            Text(when {
                                s.status.isActive -> "Gizzi is still in this call. The recap is written when it ends."
                                MeetingFormat.recapLine(meeting, s) == "Writing the recap…" -> "Gizzi is writing the recap. Pull down to check again."
                                else -> MeetingFormat.endReason(s.endReason)
                            }, color = c.textSecondary, fontSize = 14.sp)
                        }
                        if (artifacts.isNotEmpty()) {
                            SectionLabel("Made in the call")
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) { artifacts.forEach { a -> ArtifactRow(a) { openUrl(context, a.url) } } }
                        }
                        SectionLabel("Transcript")
                        val t = transcript
                        if (t == null) {
                            SheetButton("Show the transcript", tag = "meetingShowTranscript") {
                                transcript = emptyList()
                                scope.launch { transcript = runCatching { MeetingsClient.transcript(s.id, 2000) }.getOrDefault(emptyList()) }
                            }
                        } else {
                            if (t.isEmpty()) Text("No transcript was kept for this call.", color = c.textSecondary, fontSize = 14.sp)
                            t.forEach { seg ->
                                Column(Modifier.padding(vertical = 5.dp)) {
                                    Text(seg.speaker ?: "Someone", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                                    Text(seg.text, color = c.textSecondary, fontSize = 15.sp)
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- shared pieces ----

@Composable
private fun Header(title: String, leadingIcon: Int, leadingLabel: String, onLeading: () -> Unit, trailing: (@Composable () -> Unit)?) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        RoundButton(leadingIcon, leadingLabel, "meetingsBack", onLeading)
        Text(title, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, textAlign = androidx.compose.ui.text.style.TextAlign.Center, modifier = Modifier.weight(1f).padding(horizontal = 8.dp))
        if (trailing != null) trailing() else Spacer(Modifier.size(40.dp))
    }
}

@Composable
private fun RoundButton(icon: Int, label: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.size(40.dp).clip(CircleShape).background(c.fillNeutral).clickable(onClickLabel = label, onClick = onClick).testTag(tag).semantics { contentDescription = label },
        contentAlignment = Alignment.Center) { Icon(painterResource(icon), null, tint = c.textPrimary, modifier = Modifier.size(18.dp)) }
}

@Composable
private fun SheetButton(label: String, primary: Boolean = false, enabled: Boolean = true, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().height(46.dp).alpha(if (enabled) 1f else 0.45f).clip(RoundedCornerShape(AllternitRadii.MD))
        .background(if (primary) c.accentPrimary else c.fillNeutral).clickable(enabled = enabled, onClick = onClick).testTag(tag), contentAlignment = Alignment.Center) {
        Text(label, color = if (primary) c.onAccent else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun ControlButton(icon: Int, caption: String, active: Boolean = false, destructive: Boolean = false, busy: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val bg = if (destructive) Color(0xFFDC2626) else if (active) c.textPrimary else c.fillNeutral
    val fg = if (destructive || active) c.bgPrimary else c.textPrimary
    Column(Modifier.alpha(if (enabled) 1f else 0.45f).clickable(enabled = enabled && !busy, onClickLabel = caption, onClick = onClick), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.size(56.dp).clip(CircleShape).background(bg), contentAlignment = Alignment.Center) {
            if (busy) CircularProgressIndicator(Modifier.size(20.dp), color = fg, strokeWidth = 2.dp) else Icon(painterResource(icon), caption, tint = fg, modifier = Modifier.size(22.dp))
        }
        Text(caption, color = c.textSecondary, fontSize = 12.sp)
    }
}

@Composable
private fun ArtifactRow(a: MeetingArtifact, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    val kind = when (a.kind) { "slides" -> "Slides"; "canvas" -> "Canvas"; else -> "Doc" }
    val icon = when (a.kind) { "slides" -> DesignR.drawable.deck_slides; "canvas" -> DesignR.drawable.deck_design; else -> DesignR.drawable.deck_docs }
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.MD)).border(1.dp, c.borderSubtle, RoundedCornerShape(AllternitRadii.MD)).clickable(onClickLabel = "Opens the ${kind.lowercase()}", onClick = onOpen).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Icon(painterResource(icon), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text(a.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
            Text(kind + if ((a.version ?: 1) > 1) " · version ${a.version}" else "", color = c.textSecondary, fontSize = 12.sp)
        }
        Icon(painterResource(DesignR.drawable.ph_arrow_square_out), null, tint = c.textTertiary, modifier = Modifier.size(16.dp))
    }
}

@Composable private fun SectionLabel(text: String) = Text(text.uppercase(), color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.6.sp, modifier = Modifier.padding(top = 22.dp, bottom = 8.dp, start = 4.dp))

@Composable private fun Card(content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(AllternitRadii.LG)).background(c.fillNeutral)) { content() }
}

@Composable private fun Divider() = Box(Modifier.fillMaxWidth().padding(start = 14.dp).height(1.dp).background(AllternitTheme.colors.borderSubtle))

@Composable private fun PanelEmpty(text: String, art: com.allternit.core.design.EmptyArtKind? = null) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
        if (art != null) com.allternit.core.design.EmptyArt(art, size = 72.dp)
        Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(vertical = 24.dp))
    }
}

@Composable private fun BulletRow(text: String, detail: String?) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().padding(14.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.padding(top = 6.dp).size(8.dp).clip(CircleShape).border(1.5.dp, c.textSecondary, CircleShape))
        Column {
            Text(text, color = c.textPrimary, fontSize = 15.sp)
            if (!detail.isNullOrEmpty()) Text(detail, color = c.textTertiary, fontSize = 12.sp)
        }
    }
}

/** [FriendlyState] with the Meetings icon for each style (the design slot needs one). */
@Composable
private fun MeetingsState(style: FriendlyStyle, title: String, message: String, actionTitle: String? = null, onAction: (() -> Unit)? = null) {
    FriendlyState(style, title, message, actionTitle = actionTitle, onAction = onAction) {
        val c = AllternitTheme.colors
        when (style) {
            FriendlyStyle.Empty -> Icon(painterResource(DesignR.drawable.ph_calendar_blank), null, tint = c.accentPrimary, modifier = Modifier.size(28.dp))
            else -> Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary, modifier = Modifier.size(28.dp))
        }
    }
}
