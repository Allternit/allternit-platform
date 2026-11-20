package com.allternit.feature.bots

import com.allternit.core.design.AllternitSerif
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.LibrarySearchField
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.QuestionAsk
import com.allternit.feature.chat.QuestionCard
import com.allternit.feature.chat.QuestionItem
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

// MARK: - Agent activity (iOS Features/AgentActivity, the Factory mail threads on the computer)

/** iOS AgentActivityThreadSummary. */
data class AgentActivityThread(val threadId: String, val messageCount: Int, val lastActivityAt: String)

private data class ActivityState(val pendingReview: Boolean, val guardActivity: Boolean, val reservation: Boolean)

/** iOS AgentActivityStore: threads, the ledger tail and each thread's derived state. */
private object AgentActivityData {
    var threads: List<AgentActivityThread> = emptyList()
    var states: Map<String, ActivityState> = emptyMap()
    var events: Map<String, List<Triple<String, String, String>>> = emptyMap()
    val viewed = mutableSetOf<String>()

    suspend fun load(backend: ChatBackend): String? {
        when (val r = backend.runtimeResult("GET", "/api/factory/mail/threads", doing = "load agent activity")) {
            is BotsResult.Ok -> threads = r.value.listUnder("threads").mapNotNull { o -> AgentActivityThread(o.str("thread_id") ?: return@mapNotNull null, o.num("messages")?.toInt() ?: 0, o.str("last_ts").orEmpty()) }
            is BotsResult.Failed -> return r.message
        }
        (backend.runtimeResult("POST", "/api/factory/ledger/tail", "{\"count\":200}", "load agent activity") as? BotsResult.Ok)?.value?.let { j ->
            val ledger = (j as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty().map { e ->
                val p = e["payload"] as? JsonObject
                Triple(e.str("event_id").orEmpty(), e.str("event_type").orEmpty(), e.str("timestamp").orEmpty()) to (p.str("thread_id") ?: p.str("mail_thread_id"))
            }
            val st = mutableMapOf<String, ActivityState>()
            val ev = mutableMapOf<String, List<Triple<String, String, String>>>()
            for (t in threads) {
                val window = ledger.filter { it.second == t.threadId }.map { it.first }.takeLast(4)
                if (window.isEmpty()) continue
                ev[t.threadId] = window
                fun decide(type: String) = type.contains("decide", true) || type == "ReviewDecision"
                val lastReview = window.indexOfLast { it.second.contains("review", true) && !decide(it.second) }
                val lastDecide = window.indexOfLast { decide(it.second) }
                st[t.threadId] = ActivityState(lastReview >= 0 && lastDecide < lastReview, window.any { it.second.contains("guard", true) }, window.any { it.second.contains("reserve", true) })
            }
            states = st
            events = ev
        }
        return null
    }
}

/** iOS AgentActivityListView: search, the tag filter (a sheet, not a menu), the thread rows with their tags. */
@Composable
internal fun AgentActivityListScreen(backend: ChatBackend, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var threads by remember { mutableStateOf(AgentActivityData.threads) }
    var search by remember { mutableStateOf("") }
    var filter by remember { mutableStateOf("all") }
    var filterOpen by remember { mutableStateOf(false) }
    suspend fun load() { loading = true; error = AgentActivityData.load(backend); threads = AgentActivityData.threads; loading = false }
    LaunchedEffect(Unit) { load() }
    val filters = listOf("all" to "All threads", "review" to "Needs review", "guard" to "Guard activity", "reservation" to "Reservation")
    val visible = threads.filter { t ->
        val s = AgentActivityData.states[t.threadId]
        when (filter) { "review" -> s?.pendingReview == true; "guard" -> s?.guardActivity == true; "reservation" -> s?.reservation == true; else -> true }
    }.filter { search.isBlank() || it.threadId.contains(search.trim(), true) }
    BotScreen("Agent Activity", nav, testTag = "agentActivityList", trailing = {
        Box {
            LibraryIconButton(DesignR.drawable.ph_sliders_horizontal, "Filter threads", "agentActivityFilter") { filterOpen = true }
            if (filter != "all") Box(Modifier.align(Alignment.TopEnd).padding(10.dp)) { Dot(c.accentPrimary, 7.dp) }
        }
    }) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            if (!ListState(loading, error?.let { com.allternit.core.design.FriendlyErrorMessage.from(it) }, threads.isEmpty(), "Couldn't load agent activity", "No agent activity yet. Threads show up here as agents send mail through the Factory.") { scope.launch { load() } }) {
                LibrarySearchField(search, "Search threads", "agentActivitySearch", Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { search = it }
                if (visible.isEmpty()) Text("No threads match.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                visible.forEach { t ->
                    val s = AgentActivityData.states[t.threadId]
                    Column(
                        Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp))
                            .testTag("agentActivity${t.threadId}").clickable { nav.open(BotRoute.ActivityDetail(t)) }.padding(12.dp),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text("Thread", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                            if (t.threadId !in AgentActivityData.viewed) Dot(c.accentPrimary, 7.dp)
                            Spacer(Modifier.weight(1f))
                            Text(botTimeRelative(t.lastActivityAt), color = c.textTertiary, fontSize = 12.sp)
                        }
                        Text("${t.threadId.take(8)} · ${if (t.messageCount == 1) "1 message" else "${t.messageCount} messages"}", color = c.textSecondary, fontSize = 13.sp)
                        if (s != null) Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (s.pendingReview) Tag("Needs review", AllternitStatic.StatusWarning)
                            if (s.guardActivity) Tag("Guard activity", AllternitStatic.StatusError)
                            if (s.reservation) Tag("Reservation", c.textSecondary)
                        }
                    }
                }
            }
        }
    }
    if (filterOpen) LibrarySheet(title = "Filter", testTag = "agentActivityFilterSheet", onDismiss = { filterOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { filterOpen = false }) {
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            filters.forEachIndexed { i, (id, label) -> NavRow(label, null, if (filter == id) "✓" else null, "agentActivityFilter$id", last = i == filters.lastIndex) { filter = id; filterOpen = false } }
        }
    }
}

@Composable
private fun Tag(label: String, color: androidx.compose.ui.graphics.Color) =
    Text(label, color = color, fontSize = 11.sp, fontWeight = FontWeight.Medium, modifier = Modifier.clip(CircleShape).background(color.copy(alpha = 0.12f)).padding(horizontal = 8.dp, vertical = 3.dp))

/** iOS AgentActivityDetailView: header, guard and reservation activity, the review card, messages, reply. */
@Composable
internal fun AgentActivityDetailScreen(thread: AgentActivityThread, backend: ChatBackend, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var messages by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var reply by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var deciding by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    val esc = BotsApi.esc(thread.threadId)
    suspend fun load() {
        if (messages.isEmpty()) loading = true
        error = null
        when (val r = backend.runtimeResult("GET", "/api/factory/mail/thread/$esc", doing = "load messages")) { is BotsResult.Ok -> messages = r.value.listUnder("messages"); is BotsResult.Failed -> error = r.message }
        loading = false
    }
    LaunchedEffect(thread.threadId) { AgentActivityData.viewed.add(thread.threadId); load() }
    val state = AgentActivityData.states[thread.threadId]
    BotScreen(thread.threadId.take(12), nav, testTag = "agentActivityDetail") {
        Column(Modifier.fillMaxSize().imePadding()) {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                actionError?.let { ErrorLine(it, Modifier.padding(0.dp)) }
                Row {
                    Text(if (thread.messageCount == 1) "1 message" else "${thread.messageCount} messages", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    botTimeRelative(thread.lastActivityAt).takeIf { it.isNotEmpty() }?.let { Text("Last activity $it", color = c.textTertiary, fontSize = 12.sp) }
                }
                val events = AgentActivityData.events[thread.threadId].orEmpty().filter { it.second.contains("guard", true) || it.second.contains("reserve", true) }
                if (events.isNotEmpty()) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("Guard & reservation activity", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                    events.forEach { (_, type, ts) -> Row { Text(type, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f)); Text(botTimeRelative(ts), color = c.textTertiary, fontSize = 12.sp) } }
                }
                if (state?.pendingReview == true) Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(AllternitStatic.StatusWarning.copy(alpha = 0.1f)).padding(12.dp).testTag("agentActivityReview"),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text("REVIEW REQUESTED", color = AllternitStatic.StatusWarning, fontSize = 11.sp, fontWeight = FontWeight.Bold, letterSpacing = 0.6.sp)
                    Text("Recent ledger activity on this thread includes a review request with no matching decision yet.", color = c.textPrimary, fontSize = 13.sp)
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        listOf(true to "Approve", false to "Reject").forEach { (approve, label) ->
                            Pill(label, "agentActivity$label", primary = approve, enabled = !deciding) {
                                deciding = true
                                scope.launch {
                                    when (val r = backend.runtimeResult("POST", "/api/factory/mail/decide", "{\"thread\":${JsonPrimitive(thread.threadId)},\"approve\":$approve}", "record the decision")) {
                                        is BotsResult.Ok -> { AgentActivityData.load(backend); load() }
                                        is BotsResult.Failed -> actionError = "Couldn't record the decision: ${r.message}"
                                    }
                                    deciding = false
                                }
                            }
                        }
                    }
                }
                when {
                    loading && messages.isEmpty() -> LibrarySpinner("Loading messages")
                    error != null -> ListState(false, error, true, "Couldn't load messages", "") { scope.launch { load() } }
                    messages.isEmpty() -> Text("No messages yet. Messages from this thread will appear here.", color = c.textSecondary, fontSize = 14.sp)
                    else -> messages.forEach { m ->
                        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Row { Text(m.str("from_agent") ?: "Agent", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f)); Text(botTimeRelative(m.str("timestamp")), color = c.textTertiary, fontSize = 11.5.sp) }
                            Text(m.str("body") ?: "(no plain-text body — ${m.str("event_type").orEmpty()})", color = c.textSecondary, fontSize = 13.5.sp)
                        }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.weight(1f).heightIn(min = 40.dp).clip(RoundedCornerShape(20.dp)).background(c.fillNeutral).padding(horizontal = 14.dp, vertical = 10.dp)) {
                    if (reply.isEmpty()) Text("Reply in this thread…", color = c.textTertiary, fontSize = 15.sp)
                    BasicTextField(reply, { reply = it }, textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("agentActivityReply"))
                }
                Pill("Send", "agentActivitySend", primary = true, enabled = reply.isNotBlank(), busy = sending) {
                    val text = reply.trim()
                    sending = true
                    scope.launch {
                        when (val r = backend.runtimeResult("POST", "/api/factory/mail/send", "{\"thread\":${JsonPrimitive(thread.threadId)},\"body\":${JsonPrimitive(text)}}", "send the reply")) {
                            is BotsResult.Ok -> { reply = ""; load() }
                            is BotsResult.Failed -> actionError = "Couldn't send: ${r.message}"
                        }
                        sending = false
                    }
                }
            }
        }
    }
}

// MARK: - Decisions (iOS Features/Decisions)

private fun parseQuestionAsk(o: JsonObject): Pair<QuestionAsk, String?>? {
    val id = o.str("id") ?: return null
    val items = (o["questions"] as? JsonArray)?.mapNotNull { el ->
        val q = el as? JsonObject ?: return@mapNotNull null
        QuestionItem(q.str("header"), q.str("question") ?: return@mapNotNull null,
            (q["options"] as? JsonArray)?.mapNotNull { opt -> (opt as? JsonObject)?.str("label") ?: (opt as? JsonPrimitive)?.contentOrNull }.orEmpty(),
            (q["multiple"] as? JsonPrimitive)?.contentOrNull == "true")
    }.orEmpty()
    if (items.isEmpty()) return null
    val sid = o.str("sessionID") ?: o.str("sessionId") ?: o.str("session_id")
    return QuestionAsk(id, sid, items) to sid
}

/** A Factory approval (iOS FactoryApprovalsClient.Approval). */
private data class FactoryApproval(val id: String, val nodeId: String, val title: String, val summary: String, val risk: String, val riskReason: String?)

/**
 * The decisions deck (iOS DecisionsDeckView + DecisionsScreen): every question an agent is waiting
 * on, one card at a time with "1 of N" and the pager, then the Factory approvals (Reject / Approve).
 * Questions poll every 5 s, approvals every 15 s. Phone approval holds live in the Inbox's Review.
 */
@Composable
internal fun DecisionsScreen(host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var questions by remember { mutableStateOf<List<Pair<QuestionAsk, String?>>>(emptyList()) }
    var approvals by remember { mutableStateOf<List<FactoryApproval>>(emptyList()) }
    var qError by remember { mutableStateOf<String?>(null) }
    var aError by remember { mutableStateOf<String?>(null) }
    var loaded by remember { mutableStateOf(false) }
    var index by remember { mutableStateOf(0) }
    var resolved by remember { mutableStateOf(setOf<String>()) }
    var names by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    suspend fun refreshQuestions() {
        when (val r = LibraryNet.get("questions")) {
            is ApiResult.Ok -> { questions = r.value.listUnder("questions").mapNotNull(::parseQuestionAsk); qError = null }
            else -> qError = "Couldn't load decisions. " + r.friendly("load decisions")
        }
        val missing = questions.mapNotNull { it.second }.toSet() - names.keys
        if (missing.isNotEmpty()) BotsApi.listSessions().orNull()?.let { rows ->
            names = names + rows.filter { it.id in missing }.mapNotNull { s -> (s.agentName ?: s.agentId?.let { store.bot(it)?.displayName })?.let { s.id to it } }
        }
    }
    suspend fun refreshApprovals() {
        when (val r = host.backend.runtimeResult("GET", "/api/factory/approvals?state=pending", doing = "load approvals")) {
            is BotsResult.Ok -> { approvals = r.value.listUnder("approvals").mapNotNull { o -> FactoryApproval(o.str("id") ?: return@mapNotNull null, o.str("nodeId").orEmpty(), o.str("title") ?: "Approval", o.str("summary").orEmpty(), o.str("risk") ?: "low", o.str("riskReason")) }.filter { it.id !in resolved }; aError = null }
            is BotsResult.Failed -> aError = "Couldn't load approvals. ${r.message}"
        }
    }
    LaunchedEffect(Unit) {
        var tick = 0
        while (true) {
            refreshQuestions()
            if (tick % 3 == 0) refreshApprovals()
            loaded = true
            tick++
            delay(5_000)
        }
    }
    val items = questions.filter { it.first.id !in resolved }
    BotScreen("Decisions", nav, testTag = "decisionsScreen") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (items.isNotEmpty()) {
                val at = index.coerceIn(0, items.lastIndex)
                val (ask, sid) = items[at]
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Decisions", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text("${items.size} waiting for you", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    if (items.size > 1) {
                        Text("${at + 1} of ${items.size}", color = c.textTertiary, fontSize = 12.sp)
                        LibraryIconButton(DesignR.drawable.ph_caret_left, "Previous decision", "decisionsPrev", enabled = at > 0, iconSize = 16) { index = at - 1 }
                        LibraryIconButton(com.allternit.core.design.R.drawable.ph_caret_right, "Next decision", "decisionsNext", enabled = at < items.lastIndex, iconSize = 16) { index = at + 1 }
                    }
                }
                sid?.let { names[it] }?.let { Text("From $it", color = c.textTertiary, fontSize = 12.sp) }
                androidx.compose.runtime.key(ask.id) {
                    QuestionCard(
                        ask,
                        onAnswer = { answers, onError ->
                            scope.launch {
                                when (val r = host.backend.replyQuestion(ask.id, answers)) {
                                    is ApiResult.Ok -> { resolved = resolved + ask.id; delay(700); index = at.coerceAtMost((items.size - 2).coerceAtLeast(0)) }
                                    else -> onError(r.friendly("answer"))
                                }
                            }
                        },
                        onSkip = { onError ->
                            scope.launch {
                                when (val r = host.backend.rejectQuestion(ask.id)) {
                                    is ApiResult.Ok -> resolved = resolved + ask.id
                                    else -> onError(r.friendly("skip"))
                                }
                            }
                        },
                    )
                }
            } else qError?.takeIf { approvals.isEmpty() }?.let { ErrorRetry(it) { scope.launch { refreshQuestions() } } }
            if (approvals.isNotEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Approvals", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text("${approvals.size} waiting", color = c.textSecondary, fontSize = 13.sp)
                }
                approvals.forEach { a -> ApprovalCard(a, host.backend) { resolved = resolved + a.id; approvals = approvals.filter { it.id != a.id } } }
            } else aError?.let { ErrorRetry(it) { scope.launch { refreshApprovals() } } }
            if (items.isEmpty() && approvals.isEmpty() && qError == null) {
                if (loaded) Column(Modifier.fillMaxWidth().padding(vertical = 32.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("✓", color = c.textTertiary, fontSize = 28.sp)
                    Text("Nothing is waiting for you.", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium, modifier = Modifier.testTag("decisionsEmpty"))
                    Text("When an agent asks a question or a Factory step needs approval, it shows here.", color = c.textSecondary, fontSize = 13.sp)
                } else LibrarySpinner("Loading decisions")
            }
            // Phone approval holds are reviewed in the Inbox (iOS PhoneHoldReviewView, wave C).
            Pill("Phone approvals in Inbox", "decisionsInbox", icon = com.allternit.core.design.R.drawable.ph_tray) { host.openInbox() }
        }
    }
}

@Composable
private fun ErrorRetry(message: String, onRetry: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(message, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
        Pill("Retry", "decisionsRetry", onClick = onRetry)
    }
}

/** iOS FactoryApprovalCard. */
@Composable
private fun ApprovalCard(a: FactoryApproval, backend: ChatBackend, onResolved: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val high = a.risk == "high"
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, if (high) AllternitStatic.StatusWarning else c.borderSubtle), RoundedCornerShape(14.dp)).padding(12.dp).testTag("approval${a.id}"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(a.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f, fill = false))
            if (high) Tag("High risk", AllternitStatic.StatusWarning)
        }
        if (a.summary.isNotEmpty()) Text(a.summary, color = c.textSecondary, fontSize = 13.sp)
        if (high) a.riskReason?.takeIf { it.isNotEmpty() }?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.5.sp) }
        Text("Node ${a.nodeId}", color = c.textTertiary, fontSize = 11.5.sp, fontFamily = FontFamily.Monospace)
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            listOf("reject" to "Reject", "approve" to "Approve").forEach { (decision, label) ->
                Pill(label, "approval$label${a.id}", primary = decision == "approve", enabled = busy == null, busy = busy == decision) {
                    busy = decision
                    error = null
                    scope.launch {
                        when (val r = backend.runtimeResult("POST", "/api/factory/approvals/${BotsApi.esc(a.id)}/resolve", "{\"decision\":\"$decision\"}", "resolve the approval")) {
                            is BotsResult.Ok -> onResolved()
                            is BotsResult.Failed -> error = r.message
                        }
                        busy = null
                    }
                }
            }
        }
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
    }
}

// Impact map: see ImpactMap.kt.

// MARK: - Bot project page (iOS BotProjectPage)

/**
 * A bot project (iOS BotProjectPage): the title and counts, the coordinator's messages, the bots
 * on the project (start one on a new thread), the project's threads grouped (Reopen on resolved),
 * the coordinator composer, and the project actions sheet (edit details, favorite, archive,
 * delete, the project detail in Projects).
 */
@Composable
internal fun BotProjectPage(project: BotProjectSummary, host: BotsHost, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var title by remember { mutableStateOf(project.title) }
    var details by remember { mutableStateOf(project.description.orEmpty()) }
    var messages by remember { mutableStateOf<List<JsonObject>>(emptyList()) }
    var threads by remember { mutableStateOf<List<BotThread>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf(false) }
    var sendError by remember { mutableStateOf<String?>(null) }
    var prefill by remember { mutableStateOf<String?>(null) }
    var actionsOpen by remember { mutableStateOf(false) }
    var editOpen by remember { mutableStateOf(false) }
    var deleteOpen by remember { mutableStateOf(false) }
    var favorite by remember { mutableStateOf(project.favorite) }
    var archived by remember { mutableStateOf(project.archived) }
    suspend fun load() {
        val m = BotsApi.projectMessages(project.id)
        val t = BotsApi.listProjectThreads(project.id)
        (m as? BotsResult.Ok)?.let { messages = it.value }
        (t as? BotsResult.Ok)?.let { threads = it.value }
        error = (m as? BotsResult.Failed)?.message ?: (t as? BotsResult.Failed)?.message
        loading = false
    }
    LaunchedEffect(project.id) { while (true) { load(); delay(10_000) } }
    suspend fun setFlag(key: String, value: Boolean) {
        val current = BotsApi.coworkProject(project.id).orNull()
        val meta = current?.str("metadata")?.let { runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() } ?: JsonObject(emptyMap())
        BotsApi.patchCoworkProject(project.id, buildJsonObject { put("metadata", JsonObject(meta + (key to JsonPrimitive(value))).toString()) })
        store.refreshProjects()
    }
    val bots = project.botIds.mapNotNull(store::bot).ifEmpty { threads.map { it.botId }.distinct().mapNotNull(store::bot) }
    Column(Modifier.fillMaxSize().statusBarsPadding().testTag("botProjectPage")) {
        LibraryHeader(title = "Project", onBack = nav::back, backTag = "botsBack", trailing = {
            LibraryIconButton(com.allternit.core.design.R.drawable.ph_dots_three, "Project actions", "botProjectActions") { actionsOpen = true }
        })
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(bottom = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Column(Modifier.padding(horizontal = 20.dp)) {
                Text(title, color = c.textPrimary, fontSize = 26.sp, fontFamily = AllternitSerif, modifier = Modifier.testTag("botProjectTitle"))
                if (details.isNotEmpty()) Text(details, color = c.textSecondary, fontSize = 14.sp)
                val w = threads.count { it.group == "waiting" }
                val k = threads.count { it.group == "working" }
                Text("$k working · $w need${if (w == 1) "s" else ""} you · ${threads.size} threads", color = c.textTertiary, fontSize = 12.5.sp, modifier = Modifier.padding(top = 4.dp))
            }
            SectionHeader("Coordinator · ${store.bots.firstOrNull { it.isMainBot }?.displayName ?: "A://"}")
            when {
                loading -> LibrarySpinner("Loading the project")
                error != null && messages.isEmpty() && threads.isEmpty() -> ListState(false, error, true, "Couldn't load the project", "") { scope.launch { load() } }
                messages.isEmpty() -> Text("No messages yet. Tell the coordinator what you need below.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp))
                else -> messages.takeLast(12).forEach { m ->
                    val mine = (m.str("role") ?: m.str("from")) == "user"
                    Text(
                        m.str("text") ?: m.str("content").orEmpty(), color = c.textPrimary, fontSize = 14.sp,
                        modifier = Modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(if (mine) c.fillNeutral else c.bgPanel).padding(12.dp),
                    )
                }
            }
            SectionHeader("Bots")
            Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                bots.forEach { b ->
                    Column(
                        Modifier.clip(RoundedCornerShape(12.dp)).testTag("botProjectBot${b.id}").clickable {
                            scope.launch {
                                val sid = store.newThreadSession(b)
                                if (sid is BotsResult.Ok) {
                                    LibraryNet.send("POST", "threads", buildJsonObject { put("botId", b.id); put("title", "New thread"); put("sessionId", sid.value); put("createdBy", "user"); put("projectId", project.id) })
                                    nav.open(BotRoute.Chat(b.id, sid.value))
                                } else sendError = "${b.displayName} couldn't start a thread. Try again."
                            }
                        }.padding(6.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                    ) {
                        BotAvatar(b, 44.dp)
                        Text(b.displayName, color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                    }
                }
                Column(Modifier.clip(RoundedCornerShape(12.dp)).testTag("botProjectAddBot").clickable { nav.open(BotRoute.Hub) }.padding(6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Box(Modifier.size(44.dp).clip(CircleShape).border(1.dp, c.borderStrong, CircleShape), contentAlignment = Alignment.Center) { Text("+", color = c.textSecondary, fontSize = 18.sp) }
                    Text("Add a bot", color = c.textSecondary, fontSize = 12.sp)
                }
            }
            SectionHeader("Threads")
            BotThreadsPanel(store, threads, null, null, onOpen = { nav.openThread(it) }, onReopen = { t ->
                scope.launch {
                    LibraryNet.send("POST", "threads/${BotsApi.esc(t.id)}/unarchive")
                    LibraryNet.send("PATCH", "threads/${BotsApi.esc(t.id)}", buildJsonObject { put("status", "idle") })
                    load()
                }
            })
        }
        sendError?.let { ErrorLine(it) }
        host.composer(BotComposerSpec(
            placeholder = "Tell the coordinator what you need…", sendDisabled = sending, prefill = prefill, onPrefillConsumed = { prefill = null },
            onSend = { req ->
                val text = req.text.trim()
                if (text.isNotEmpty()) {
                    sending = true
                    sendError = null
                    scope.launch {
                        when (val r = BotsApi.sendToCoordinator(project.id, text)) { is BotsResult.Ok -> load(); is BotsResult.Failed -> { sendError = r.message; prefill = text } }
                        sending = false
                    }
                }
            },
        ))
    }
    if (actionsOpen) LibrarySheet(title = title, testTag = "botProjectActionsSheet", onDismiss = { actionsOpen = false }, trailing = LibrarySheetAction("Done", bold = true) { actionsOpen = false }) {
        RowGroup(Modifier.padding(vertical = 8.dp)) {
            NavRow("Edit details", com.allternit.core.design.R.drawable.ph_pencil_simple, null, "botProjectEdit", last = false) { actionsOpen = false; editOpen = true }
            NavRow("Project settings", com.allternit.core.design.R.drawable.ph_gear, null, "botProjectSettings", last = false) { actionsOpen = false; host.openProjectInLibrary(project.id) }
            NavRow(if (favorite) "Remove from favorites" else "Add to favorites", com.allternit.core.design.R.drawable.ph_star, null, "botProjectFavorite", last = false) { favorite = !favorite; scope.launch { setFlag("favorite", favorite) } }
            NavRow(if (archived) "Unarchive" else "Archive", com.allternit.core.design.R.drawable.ph_tray, null, "botProjectArchive", last = false) { archived = !archived; scope.launch { setFlag("archived", archived) } }
            NavRow("Delete", DesignR.drawable.ph_trash, null, "botProjectDelete", destructive = true) { actionsOpen = false; deleteOpen = true }
        }
        Gap(12.dp)
    }
    if (editOpen) {
        var t by remember { mutableStateOf(title) }
        var d by remember { mutableStateOf(details) }
        LibrarySheet(title = "Edit details", testTag = "botProjectEditSheet", onDismiss = { editOpen = false }, leading = LibrarySheetAction("Cancel") { editOpen = false },
            trailing = LibrarySheetAction("Save", enabled = t.isNotBlank(), bold = true, testTag = "botProjectEditSave") {
                editOpen = false
                scope.launch {
                    val body = JsonObject(mapOf("title" to JsonPrimitive(t.trim()), "description" to if (d.isBlank()) kotlinx.serialization.json.JsonNull else JsonPrimitive(d)))
                    if (BotsApi.patchCoworkProject(project.id, body) is BotsResult.Ok) { title = t.trim(); details = d; store.refreshProjects() }
                }
            }) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                com.allternit.feature.artifacts.LibraryField("Title", t, "Project title", "botProjectEditTitle") { t = it }
                com.allternit.feature.artifacts.LibraryField("Details", d, "What this project is for", "botProjectEditDetails", singleLine = false, minHeight = 80) { d = it }
            }
        }
    }
    if (deleteOpen) ConfirmSheet("Delete \"$title\"?", "Its threads aren't deleted: they stay with their bots, outside any project. This can't be undone.", "Delete", "botProjectDeleteConfirm", onDismiss = { deleteOpen = false }) {
        deleteOpen = false
        scope.launch {
            if (LibraryNet.send("DELETE", "cowork/projects/${BotsApi.esc(project.id)}") is ApiResult.Ok) { store.refreshProjects(); nav.back() }
            else sendError = "Couldn't delete the project. Try again."
        }
    }
}

/** A project thread with no chat yet (iOS BotProjectThreadPage): its status, steps, and it opens once it has a session. */
@Composable
internal fun BotProjectThreadPage(thread: BotThread, store: BotModeStore, nav: BotNavigator) {
    val c = AllternitTheme.colors
    var current by remember { mutableStateOf(thread) }
    LaunchedEffect(thread.id) {
        while (true) {
            (LibraryNet.get("threads/${BotsApi.esc(thread.id)}") as? ApiResult.Ok)?.value?.obj?.let { o -> ((o["thread"] as? JsonObject) ?: o).let(BotThread::from)?.let { current = it } }
            current.currentSessionId?.let { sid -> nav.back(); nav.open(BotRoute.Chat(current.botId, sid)); return@LaunchedEffect }
            delay(4_000)
        }
    }
    val bot = store.bot(current.botId)
    BotScreen(current.displayTitle.ifEmpty { "Thread" }, nav, testTag = "botProjectThread") {
        Column(Modifier.fillMaxSize().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                bot?.let { BotAvatar(it, 40.dp, live = true) }
                Column {
                    Text(bot?.displayName ?: "Bot", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text(BotThreadGroup.of(current.group).title, color = groupColor(BotThreadGroup.of(current.group)), fontSize = 12.5.sp)
                }
            }
            Text(current.statusLine ?: "Waiting to start. The chat opens here as soon as the bot picks it up.", color = c.textSecondary, fontSize = 14.sp)
            current.todo.forEach { (text, state) -> Text((if (state == "done") "✓ " else "○ ") + text, color = c.textPrimary, fontSize = 13.5.sp) }
        }
    }
}
