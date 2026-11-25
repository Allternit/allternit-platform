package com.allternit.feature.shell

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
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.gestures.AnchoredDraggableDefaults
import androidx.compose.foundation.gestures.AnchoredDraggableState
import androidx.compose.foundation.gestures.DraggableAnchors
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.anchoredDraggable
import androidx.compose.foundation.gestures.animateTo
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalContext
import kotlinx.coroutines.delay
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.IntOffset
import com.allternit.core.design.Motion
import kotlin.math.roundToInt
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import java.net.URLEncoder
import java.time.OffsetDateTime
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put

// MARK: - Model (iOS Core/API/InboxClient.swift)

/** One "Needs you" item: runtime `GET /api/v1/inbox?scope=needs_you` → `{ needsYou: [...] }` (camelCase). */
internal data class InboxItem(
    val id: String,
    val kind: String,
    val botId: String,
    val botName: String,
    val person: String?,
    val summary: String,
    val at: String,
    /** approve · open · reply · callback · retry */
    val primaryAction: String,
    val primaryLabel: String,
    val actions: List<String>,
    val severity: String?,
    val attentionKind: String?,
    val trust: String?,
    val sourceKind: String?,
    val text: String?,
    val contentHash: String?,
    val subkind: String?,
    val phone: PhoneHold?,
) {
    val canApprove: Boolean get() = "approve" in actions
    val canRetry: Boolean get() = "retry" in actions
    val isPeerHeld: Boolean get() = attentionKind == "peer_held"
    val isCritical: Boolean get() = kind == "attention" && severity == "critical"
    /** A phone approval hold: Review only, never approved from the row. */
    val isPhoneAction: Boolean get() = subkind == "phone_action"
    /** The hold id behind an `approval:<id>` item. */
    val holdId: String get() = if (id.startsWith("approval:")) id.removePrefix("approval:") else id

    /** `detail.phone` of a phone hold (every field optional). */
    data class PhoneHold(
        val phoneName: String?,
        val app: String?,
        val intent: String?,
        val goal: String?,
        val to: String?,
        val text: String?,
        val element: String?,
        val step: Int?,
        val tool: String? = null,
    )

    companion object {
        fun from(o: JsonObject): InboxItem? {
            val primary = o["primary"] as? JsonObject
            val provenance = o["provenance"] as? JsonObject
            val phone = ((o["detail"] as? JsonObject)?.get("phone") as? JsonObject)?.let { p ->
                PhoneHold(
                    phoneName = p.s("phoneName"),
                    app = p.s("app"),
                    intent = p.s("intent"),
                    goal = p.s("goal"),
                    to = p.s("to"),
                    text = p.s("text"),
                    element = (p["element"] as? JsonObject)?.s("label"),
                    step = (p["step"] as? JsonPrimitive)?.intOrNull,
                    tool = p.s("tool"),
                )
            }
            return InboxItem(
                id = o.s("id") ?: return null,
                kind = o.s("kind").orEmpty(),
                botId = o.s("botId").orEmpty(),
                botName = o.s("botName").orEmpty(),
                person = o.s("person"),
                summary = displayName(o.s("summary")).orEmpty(),
                at = o.s("at").orEmpty(),
                primaryAction = primary?.s("action") ?: "open",
                primaryLabel = primary?.s("label") ?: "Open",
                actions = (o["actions"] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.contentOrNull },
                severity = o.s("severity"),
                attentionKind = o.s("attentionKind"),
                trust = provenance?.s("trust"),
                sourceKind = provenance?.s("sourceKind"),
                text = o.s("text"),
                contentHash = o.s("contentHash"),
                subkind = o.s("subkind"),
                phone = phone,
            )
        }
    }
}

private fun JsonObject.s(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

// MARK: - Store (iOS InboxStore)

/**
 * "Needs you" state for the drawer's Inbox button and sheet (web src/lib/inbox/inbox-store.ts).
 * An action removes the row at once; a failure puts it back with the reason. Calls go to the
 * user's computer through the relay (iOS InstanceConnection.runtimeAPIData).
 */
internal class InboxStore(private val backend: ChatBackend) {
    var items by mutableStateOf<List<InboxItem>>(emptyList())
        private set
    var hasLoaded by mutableStateOf(false)
        private set
    /** false when the computer's runtime has no inbox yet (404 / 503), or there is no computer. */
    var isAvailable by mutableStateOf(true)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    /** True when the last answer was "no computer" (no device, 428, compute_required), not a timeout. */
    var noComputer by mutableStateOf(false)
        private set
    val failures = mutableStateMapOf<String, String>()
    val busy = mutableStateMapOf<String, Boolean>()

    suspend fun refresh() {
        when (val r = backend.runtimeJson("GET", "/api/v1/inbox?scope=needs_you")) {
            is RuntimeCall.Ok -> {
                items = ((r.body as? JsonObject)?.get("needsYou") as? JsonArray).orEmpty()
                    .mapNotNull { (it as? JsonObject)?.let(InboxItem::from) }
                isAvailable = true
                noComputer = false
                error = null
            }
            RuntimeCall.NoComputer -> {
                items = emptyList()
                isAvailable = false
                noComputer = true
                error = null
            }
            is RuntimeCall.Http -> if (r.status == 428 || isComputeGateText(r.message)) {
                items = emptyList()
                isAvailable = false
                noComputer = true
                error = null
            } else if (r.status == 404 || r.status == 503) {
                items = emptyList()
                isAvailable = false
                error = null
            } else {
                error = statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "load the inbox")
            }
            is RuntimeCall.Failed -> if (isComputeGateText(r.message)) {
                // 428 / compute_required: no computer is connected, which is not a timeout.
                items = emptyList()
                isAvailable = false
                noComputer = true
                error = null
            } else error = r.message
        }
        hasLoaded = true
    }

    /** `POST /api/v1/inbox/:id/:verb`. A 404 means it was already handled elsewhere: not a failure. */
    private suspend fun post(id: String, verb: String, body: JsonObject): String? {
        val escaped = URLEncoder.encode(id, "UTF-8").replace("+", "%20")
        return when (val r = backend.runtimeJson("POST", "/api/v1/inbox/$escaped/$verb", body.toString())) {
            is RuntimeCall.Ok -> null
            is RuntimeCall.Http -> if (r.status == 404) null else statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "update that item")
            RuntimeCall.NoComputer -> NoComputerCopy.DETAIL
            is RuntimeCall.Failed -> r.message
        }
    }

    suspend fun act(item: InboxItem, verb: String, body: JsonObject = JsonObject(emptyMap())) {
        if (busy[item.id] == true) return
        busy[item.id] = true
        failures.remove(item.id)
        val index = items.indexOfFirst { it.id == item.id }
        items = items.filterNot { it.id == item.id }
        val failure = post(item.id, verb, body)
        if (failure != null) {
            items = items.toMutableList().apply { if (index >= 0) add(minOf(index, size), item) else add(item) }
            failures[item.id] = failure
        }
        busy.remove(item.id)
    }

    /** A notification or the Review pane already answered this item. */
    fun removeLocally(id: String) {
        items = items.filterNot { it.id == id }
    }

    // MARK: Phone holds (iOS PhoneTaskClient.hold / crop, PhoneHoldReviewView.decide)

    /** `GET /api/v1/phones/holds/:id` state: pending, approved, denied, changed, expired; "gone" on 404. Null when unknown. */
    suspend fun holdState(holdId: String): String? {
        val escaped = URLEncoder.encode(holdId, "UTF-8").replace("+", "%20")
        return when (val r = backend.runtimeJson("GET", "/api/v1/phones/holds/$escaped")) {
            is RuntimeCall.Ok -> when (val raw = (r.body as? JsonObject)?.s("state")) {
                "approved" -> "approved"
                "denied", "rejected" -> "denied"
                "changed" -> "changed"
                "expired" -> "expired"
                else -> if (raw == null) null else "pending"
            }
            is RuntimeCall.Http -> if (r.status == 404) "gone" else null
            else -> null
        }
    }

    /** The review image (`GET /api/v1/phones/holds/:id/crop`); null when there is none or it can't load. */
    suspend fun crop(holdId: String): android.graphics.Bitmap? {
        val escaped = URLEncoder.encode(holdId, "UTF-8").replace("+", "%20")
        val (status, bytes) = backend.runtimeBytes("/api/v1/phones/holds/$escaped/crop", "image/*") ?: return null
        if (status != 200 || bytes.isEmpty()) return null
        return android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
    }

    /** Approve (with the hash the pane showed) or deny from the Review pane. */
    sealed interface Decision {
        data object Done : Decision
        /** 409: the hold moved since the bot asked. */
        data object Conflict : Decision
        data class Failed(val message: String) : Decision
    }

    suspend fun decideReviewed(item: InboxItem, approve: Boolean): Decision {
        val escaped = URLEncoder.encode(item.id, "UTF-8").replace("+", "%20")
        val body = if (approve) buildJsonObject {
            put("surface", "app")
            item.contentHash?.takeIf { it.isNotEmpty() }?.let { put("contentHash", it) }
        } else buildJsonObject { put("decision", "deny") }
        return when (val r = backend.runtimeJson("POST", "/api/v1/inbox/$escaped/approve", body.toString())) {
            is RuntimeCall.Ok -> Decision.Done.also { removeLocally(item.id) }
            is RuntimeCall.Http -> when (r.status) {
                409 -> Decision.Conflict
                404 -> Decision.Done.also { removeLocally(item.id) }
                else -> Decision.Failed(statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "answer that"))
            }
            RuntimeCall.NoComputer -> Decision.Failed(NoComputerCopy.DETAIL)
            is RuntimeCall.Failed -> Decision.Failed(r.message)
        }
    }

    suspend fun approve(item: InboxItem) = act(item, "approve")
    /** From the open Review pane: `surface: "app"` and the hash it showed, so a changed hold answers 409. */
    suspend fun approveReviewed(item: InboxItem) = act(item, "approve", buildJsonObject {
        put("surface", "app")
        item.contentHash?.takeIf { it.isNotEmpty() }?.let { put("contentHash", it) }
    })
    suspend fun deny(item: InboxItem) = act(item, "approve", buildJsonObject { put("decision", "deny") })
    /** Also how a held message from an outside agent is refused. */
    suspend fun dismiss(item: InboxItem) = act(item, "dismiss")
    suspend fun retry(item: InboxItem) = act(item, "retry")
    suspend fun snooze(item: InboxItem, minutes: Int = 60) = act(item, "snooze", buildJsonObject { put("minutes", minutes) })
}

// MARK: - Sheet (iOS InboxSheet)

/** Later choices: label and minutes. Tomorrow = the next 9:00 local (web laterOptions). */
internal fun inboxLaterOptions(now: java.time.LocalDateTime = java.time.LocalDateTime.now()): List<Pair<String, Int>> {
    var next = now.toLocalDate().atTime(9, 0)
    if (!next.isAfter(now)) next = next.plusDays(1)
    val minutes = maxOf(1L, java.time.Duration.between(now, next).toMinutes()).toInt()
    return listOf("30 min" to 30, "1 hour" to 60, "3 hours" to 180, "Tomorrow" to minutes)
}

/** Summaries past this many characters, or with line breaks, are clamped to three lines with "See full". */
private const val INBOX_CLAMP_CHARS = 150

@Composable
private fun rememberReducedMotion(): Boolean {
    val ctx = LocalContext.current
    return remember { runCatching { android.provider.Settings.Global.getFloat(ctx.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }.getOrDefault(false) }
}

private val kindWords = mapOf(
    "email_draft" to "Email", "approval" to "Approval", "held_send" to "Held message", "thread" to "Waiting",
    "missed_call" to "Missed call", "unanswered" to "Unanswered", "failed_send" to "Not delivered",
    "vendor_result" to "Vendor result", "memory_import" to "Memory", "attention" to "Needs fixing",
    "suggestion" to "Suggested",
)

private val trustWords = mapOf(
    "owner" to "you", "team" to "your team", "contact" to "a contact", "vendorbot" to "a Connected bot",
    "vendor_bot" to "a Connected bot", "tooloutput" to "a tool result", "tool_output" to "a tool result",
    "web" to "a web page", "stranger" to "someone you do not know",
)

/** iOS InboxRow.ago: "now", "5m", "3h", "2d". */
internal fun inboxAgo(iso: String, now: Long = System.currentTimeMillis()): String {
    val at = runCatching { OffsetDateTime.parse(iso).toInstant().toEpochMilli() }.getOrNull() ?: return ""
    val minutes = ((now - at) / 60_000).coerceAtLeast(0)
    if (minutes < 1) return "now"
    if (minutes < 60) return "${minutes}m"
    val hours = Math.round(minutes / 60.0)
    return if (hours < 24) "${hours}h" else "${Math.round(hours / 24.0)}d"
}

/**
 * The drawer's Inbox, "Needs you": everything across the bots that waits on you, with the main
 * action on the row. Swipe right for the main action, left for Snooze and Dismiss (iOS
 * swipeActions, same tints). [focusedId] is the item a link or
 * notification asked for. [onOpenBot] opens where the item lives; [onOpenSettings] opens Settings
 * for memories waiting to move to the Memory Drive.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun InboxSheet(
    store: InboxStore,
    focusedId: String? = null,
    onOpenBot: (String) -> Unit,
    onOpenSettings: () -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }
    var reviewing by remember { mutableStateOf<InboxItem?>(null) }
    LaunchedEffect(store) {
        store.refresh()
        focusedId?.let { id -> store.items.firstOrNull { it.id == id && it.isPhoneAction }?.let { reviewing = it } }
    }
    // Items that arrive while the sheet is open wait behind "Show N new" so the list does not jump
    // under the finger. Urgent items and the one a link points at show at once.
    var known by remember { mutableStateOf<Set<String>?>(null) }
    LaunchedEffect(store.hasLoaded) { if (store.hasLoaded && known == null) known = store.items.map { it.id }.toSet() }
    val knownIds = known
    val held = if (knownIds == null) emptyList() else store.items.filter { it.id !in knownIds && !it.isCritical && it.id != focusedId }
    val shownItems = if (held.isEmpty()) store.items else store.items.filterNot { it in held }
    val focusedIsGone = focusedId != null && store.hasLoaded && store.error == null && store.items.none { it.id == focusedId }

    fun open(item: InboxItem) {
        onDismiss()
        if (item.kind == "memory_import") onOpenSettings() else onOpenBot(item.botId)
    }

    fun primary(item: InboxItem) {
        when {
            item.isPhoneAction -> reviewing = item
            item.primaryAction == "approve" && item.canApprove -> scope.launch { store.approve(item) }
            item.primaryAction == "retry" && item.canRetry -> scope.launch { store.retry(item) }
            else -> open(item)
        }
    }

    ShellSheet(
        title = "Needs you",
        onDismiss = onDismiss,
        testTag = "inboxSheet",
        trailing = SheetAction("Done", onDismiss, bold = true, testTag = "inboxDone"),
    ) {
        Box(Modifier.fillMaxWidth().fillMaxHeight(0.92f)) {
            when {
                !store.hasLoaded -> Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Checking what needs you…", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(top = 12.dp))
                }
                !store.isAvailable -> FriendlyState(
                    style = FriendlyStyle.Offline,
                    title = if (store.noComputer) NoComputerCopy.TITLE else "No inbox on this computer yet",
                    message = if (store.noComputer) NoComputerCopy.DETAIL else "The inbox comes from your computer. Connect one, or update Allternit on it, to see what needs you.",
                    art = if (store.noComputer) com.allternit.core.design.EmptyArtKind.ConnectScene else com.allternit.core.design.EmptyArtKind.Envelope,
                    modifier = Modifier.align(Alignment.Center).padding(20.dp),
                ) { Icon(painterResource(DesignR.drawable.ph_desktop), contentDescription = null, tint = c.textSecondary) }
                store.error != null && store.items.isEmpty() -> FriendlyState(
                    style = FriendlyStyle.Error,
                    title = "Couldn't load the inbox",
                    message = store.error.orEmpty(),
                    actionTitle = "Retry",
                    onAction = { scope.launch { store.refresh() } },
                    modifier = Modifier.align(Alignment.Center).padding(20.dp),
                ) { Icon(painterResource(DesignR.drawable.ph_warning), contentDescription = null, tint = c.textSecondary) }
                store.items.isEmpty() -> FriendlyState(
                    style = FriendlyStyle.Empty,
                    title = if (focusedIsGone) "Already handled" else "You're all caught up",
                    message = "Approvals, held messages, missed calls, unanswered people and anything that failed show up here.",
                    modifier = Modifier.align(Alignment.Center).padding(20.dp).testTag("inboxEmpty"),
                    art = com.allternit.core.design.EmptyArtKind.Bell,
                ) { Icon(painterResource(com.allternit.core.design.R.drawable.ph_bell_slash), contentDescription = null, tint = c.textSecondary) }
                else -> PullToRefreshBox(
                    isRefreshing = refreshing,
                    onRefresh = {
                        scope.launch {
                            refreshing = true
                            store.refresh()
                            refreshing = false
                        }
                    },
                    modifier = Modifier.fillMaxSize(),
                ) {
                    val listState = rememberLazyListState()
                    LaunchedEffect(store.items.size) {
                        val i = store.items.indexOfFirst { it.id == focusedId }
                        if (i >= 0) listState.scrollToItem(i)
                    }
                    LazyColumn(Modifier.fillMaxSize().testTag("inboxList"), state = listState) {
                        if (focusedIsGone) item(key = "handled") {
                            Text(
                                "Already handled",
                                color = c.textSecondary,
                                fontSize = 13.sp,
                                textAlign = TextAlign.Center,
                                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp).semantics { contentDescription = "That item was already handled" },
                            )
                        }
                        if (held.isNotEmpty()) item(key = "newPill") {
                            Box(Modifier.fillMaxWidth().padding(vertical = 8.dp), contentAlignment = Alignment.Center) {
                                InboxButton(title = "Show ${held.size} new", prominent = true, enabled = true, testTag = "inboxShowNew") {
                                    known = (known ?: emptySet()) + held.map { it.id }
                                }
                            }
                        }
                        items(shownItems, key = { it.id }) { item ->
                            InboxEntry(
                                item = item,
                                primaryTitle = if (item.isPhoneAction) "Review" else item.primaryLabel,
                                store = store,
                                focused = item.id == focusedId,
                                onPrimary = { primary(item) },
                                onDeny = { scope.launch { if (item.isPeerHeld) store.dismiss(item) else store.deny(item) } },
                                onSnooze = { scope.launch { store.snooze(item) } },
                                onDismissItem = { scope.launch { store.dismiss(item) } },
                                onLater = { minutes -> scope.launch { store.snooze(item, minutes) } },
                            )
                            HorizontalDivider(color = c.borderSubtle, modifier = Modifier.padding(start = 20.dp))
                        }
                        item(key = "foot") { Spacer(Modifier.heightIn(min = 24.dp)) }
                    }
                }
            }
        }
    }

    reviewing?.let { item ->
        PhoneHoldReviewSheet(item = item, store = store, onClose = { reviewing = null })
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun InboxRow(
    item: InboxItem,
    store: InboxStore,
    focused: Boolean,
    onPrimary: () -> Unit,
    onDeny: () -> Unit,
    onDone: () -> Unit,
    onLater: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var laterOpen by remember(item.id) { mutableStateOf(false) }
    var full by remember(item.id) { mutableStateOf(false) }
    val clampable = item.summary.length > INBOX_CLAMP_CHARS || item.summary.contains('\n')
    val busy = store.busy[item.id] == true
    val label = when {
        item.isPeerHeld -> "Other agent"
        item.isPhoneAction -> "Approval · Phone"
        else -> kindWords[item.kind] ?: item.kind
    }.uppercase() + if (item.isCritical) " · URGENT" else ""
    val denyLabel = when (item.kind) {
        "email_draft" -> "Don't send"
        "suggestion" -> "Not now"
        else -> if (item.isPeerHeld) "Refuse" else "Deny"
    }
    Column(
        modifier = modifier
            .fillMaxWidth()
            .background(if (focused) c.accentPrimary.copy(alpha = 0.12f).compositeOver(c.bgPrimary) else c.bgPrimary)
            .testTag("inboxRow${item.id}")
            .padding(start = 20.dp, end = 8.dp, top = 12.dp, bottom = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(label, color = if (item.isCritical) AllternitStatic.StatusError else c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
            Text(
                item.botName + (item.person?.let { " · $it" } ?: ""),
                color = c.textSecondary,
                fontSize = 11.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(inboxAgo(item.at), color = c.textSecondary, fontSize = 11.sp)
        }
        Text(item.summary, color = c.textPrimary, fontSize = 15.sp, maxLines = if (clampable && !full) 3 else Int.MAX_VALUE, overflow = TextOverflow.Ellipsis, modifier = Modifier.testTag("inboxSummary${item.id}"))
        if (clampable) Text(
            if (full) "See less" else "See full", color = c.textSecondary, fontSize = 12.sp,
            modifier = Modifier.clickable(role = Role.Button) { full = !full }.testTag("inboxSeeFull${item.id}").padding(vertical = 2.dp),
        )
        if (item.isPeerHeld && !item.text.isNullOrEmpty()) {
            Row(Modifier.padding(start = 2.dp)) {
                Box(Modifier.width(2.dp).heightIn(min = 18.dp).background(c.borderSubtle))
                Text(item.text, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(start = 8.dp))
            }
        }
        item.trust?.let { trust ->
            val who = trustWords[trust.lowercase()] ?: trust
            val kind = item.sourceKind?.let { " ($it)" }.orEmpty()
            Text("Why: this came from $who$kind.", color = c.textSecondary, fontSize = 13.sp)
        }
        store.failures[item.id]?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
        FlowRow(Modifier.padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            InboxButton(
                title = if (busy) "Working…" else if (item.isPhoneAction) "Review" else item.primaryLabel,
                prominent = true,
                enabled = !busy,
                testTag = "inboxPrimary${item.id}",
            ) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onPrimary()
            }
            if (item.isPhoneAction || (item.canApprove && item.primaryAction == "approve")) {
                InboxButton(title = denyLabel, prominent = false, enabled = !busy, testTag = "inboxDeny${item.id}", onClick = onDeny)
            }
            InboxButton(title = "Done", prominent = false, enabled = !busy, testTag = "inboxDone${item.id}") { Haptics.perform(view, Haptics.Kind.Tap); onDone() }
            InboxButton(title = "Later", prominent = false, enabled = !busy, testTag = "inboxLater${item.id}") { laterOpen = !laterOpen }
        }
        if (laterOpen) {
                FlowRow(Modifier.semantics { contentDescription = "Remind me in" }, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                inboxLaterOptions().forEach { (label, minutes) ->
                    InboxButton(title = label, prominent = false, enabled = !busy, testTag = "inboxLater${minutes}${item.id}") { laterOpen = false; onLater(minutes) }
                }
            }
        }
    }
}

@Composable
private fun InboxButton(title: String, prominent: Boolean, enabled: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(10.dp)
    Box(
        modifier = Modifier
            .clip(shape)
            .then(if (prominent) Modifier.background(if (enabled) c.accentPrimary else c.accentPrimary.copy(alpha = 0.5f)) else Modifier.border(1.dp, c.borderStrong, shape))
            .testTag(testTag)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .heightIn(min = 34.dp)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = if (prominent) c.onAccent else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

/**
 * One row with Done: the row collapses over 200 ms, then the item is dismissed; if the runtime
 * refuses, the store puts it back and the row opens again. Animator scale 0 = no motion.
 */
@Composable
private fun InboxEntry(
    item: InboxItem,
    primaryTitle: String,
    store: InboxStore,
    focused: Boolean,
    onPrimary: () -> Unit,
    onDeny: () -> Unit,
    onSnooze: () -> Unit,
    onDismissItem: () -> Unit,
    onLater: (Int) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val reduced = rememberReducedMotion()
    var leaving by remember(item.id) { mutableStateOf(false) }
    val frac by animateFloatAsState(if (leaving) 0f else 1f, tween(if (reduced) 0 else 200), label = "inboxCollapse")
    Box(
        Modifier
            .layout { m, c ->
                val p = m.measure(c)
                layout(p.width, (p.height * frac).roundToInt()) { p.place(0, 0) }
            }
            .graphicsLayer { alpha = frac; clip = true },
    ) {
        InboxSwipeRow(
            item = item, primaryTitle = primaryTitle, store = store, focused = focused,
            onPrimary = onPrimary, onDeny = onDeny, onSnooze = onSnooze, onDismissItem = onDismissItem,
            onDone = {
                if (!leaving) {
                    leaving = true
                    scope.launch {
                        delay(if (reduced) 0L else 200L)
                        onDismissItem()
                        // A refused dismiss re-inserts the item; open the row again so it comes back.
                        delay(50)
                        leaving = false
                    }
                }
            },
            onLater = onLater,
        )
    }
}

private enum class SwipeSlot { Leading, Closed, Trailing }

/**
 * iOS `swipeActions`: swipe right to reveal the main action (accent), swipe left to reveal
 * Snooze (grey) and Dismiss (red). A revealed button acts on tap; the row's own buttons stay.
 */
@Composable
private fun InboxSwipeRow(
    item: InboxItem,
    primaryTitle: String,
    store: InboxStore,
    focused: Boolean,
    onPrimary: () -> Unit,
    onDeny: () -> Unit,
    onSnooze: () -> Unit,
    onDismissItem: () -> Unit,
    onDone: () -> Unit,
    onLater: (Int) -> Unit,
) {
    val c = AllternitTheme.colors
    val density = androidx.compose.ui.platform.LocalDensity.current
    val scope = rememberCoroutineScope()
    val slotPx = with(density) { 84.dp.toPx() }
    val state = remember(item.id) { AnchoredDraggableState(SwipeSlot.Closed) }
    LaunchedEffect(slotPx) {
        state.updateAnchors(
            DraggableAnchors {
                SwipeSlot.Leading at slotPx
                SwipeSlot.Closed at 0f
                SwipeSlot.Trailing at -2 * slotPx
            },
            state.targetValue,
        )
    }
    val fling = AnchoredDraggableDefaults.flingBehavior(state, { d: Float -> d * 0.5f }, Motion.standardSpring())
    val offset = state.offset.let { if (it.isNaN()) 0f else it }
    fun close(then: () -> Unit) {
        scope.launch { state.animateTo(SwipeSlot.Closed, Motion.standardSpring()) }
        then()
    }
    Box(Modifier.fillMaxWidth().height(IntrinsicSize.Min)) {
        // Under the row: the leading action on the left, the trailing pair on the right.
        Row(Modifier.matchParentSize()) {
            if (offset > 0f) {
                SwipeButton(primaryTitle, c.accentPrimary, c.onAccent, Modifier.width(with(density) { offset.toDp() }), "inboxSwipePrimary${item.id}") { close(onPrimary) }
            }
            Spacer(Modifier.weight(1f))
            if (offset < 0f) {
                val w = with(density) { (-offset / 2f).toDp() }
                SwipeButton("Snooze", Color(0xFF8E8E93), Color.White, Modifier.width(w), "inboxSnooze${item.id}") { close(onSnooze) }
                SwipeButton("Dismiss", AllternitStatic.StatusError, Color.White, Modifier.width(w), "inboxDismiss${item.id}") { close(onDismissItem) }
            }
        }
        InboxRow(
            item = item,
            store = store,
            focused = focused,
            onPrimary = onPrimary,
            onDeny = onDeny,
            onDone = onDone,
            onLater = onLater,
            modifier = Modifier
                .offset { IntOffset(offset.roundToInt(), 0) }
                .anchoredDraggable(state = state, orientation = Orientation.Horizontal, enabled = true, flingBehavior = fling),
        )
    }
}

@Composable
private fun SwipeButton(title: String, background: Color, ink: Color, modifier: Modifier, testTag: String, onClick: () -> Unit) {
    Box(
        modifier
            .fillMaxHeight()
            .background(background)
            .testTag(testTag)
            .clickable(role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = ink, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Clip)
    }
}

private enum class HoldPhase { Loading, Shown, Failed, Approved, Denied, Changed, Expired }

private fun holdPhase(state: String?): HoldPhase? = when (state) {
    "approved" -> HoldPhase.Approved
    "denied" -> HoldPhase.Denied
    "changed" -> HoldPhase.Changed
    "expired", "gone" -> HoldPhase.Expired
    else -> null
}

/**
 * The phone approval Review pane as a sheet over the inbox (iOS PhoneHoldReviewSheet +
 * PhoneHoldReviewView, Option B): the large screen image, the details, then Deny / Approve.
 * Approve turns on only after the image has loaded; Deny always works. States: loading, shown,
 * failed, approved, denied, changed, expired. A 409 on Approve re-reads the hold.
 */
@Composable
private fun PhoneHoldReviewSheet(item: InboxItem, store: InboxStore, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var phase by remember(item.id) { mutableStateOf(HoldPhase.Loading) }
    var image by remember(item.id) { mutableStateOf<androidx.compose.ui.graphics.ImageBitmap?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val phone = item.phone
    val phoneName = phone?.phoneName?.trim()?.takeIf { it.isNotEmpty() } ?: "your phone"
    val actionLabel = phone?.element?.trim()?.takeIf { it.isNotEmpty() }
        ?: phone?.intent?.trim()?.takeIf { it.isNotEmpty() } ?: phone?.tool ?: "this step"

    suspend fun loadCrop(keepPhase: Boolean = false) {
        if (!keepPhase) phase = HoldPhase.Loading
        val loaded = store.crop(item.holdId)?.asImageBitmap()
        image = loaded
        if (!keepPhase) phase = if (loaded == null) HoldPhase.Failed else HoldPhase.Shown
    }

    LaunchedEffect(item.id) {
        val done = holdPhase(store.holdState(item.holdId))
        if (done != null) {
            phase = done
            if (done == HoldPhase.Changed) loadCrop(keepPhase = true)
        } else {
            loadCrop()
        }
    }

    fun decide(approve: Boolean) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            when (val r = store.decideReviewed(item, approve)) {
                InboxStore.Decision.Done -> phase = if (approve) HoldPhase.Approved else HoldPhase.Denied
                InboxStore.Decision.Conflict -> phase = holdPhase(store.holdState(item.holdId)) ?: HoldPhase.Changed
                is InboxStore.Decision.Failed -> error = r.message
            }
            busy = false
        }
    }

    ShellSheet(
        title = "Review",
        onDismiss = onClose,
        testTag = "inboxReview",
        trailing = SheetAction("Done", onClose, bold = true, testTag = "inboxReviewDone"),
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("APPROVAL · PHONE", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                Text(item.summary, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text(listOfNotNull(phoneName, phone?.app?.takeIf { it.isNotBlank() }).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
            }
            Box(
                Modifier
                    .fillMaxWidth()
                    .height(380.dp)
                    .clip(RoundedCornerShape(16.dp))
                    .background(c.fillNeutral)
                    .border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)),
                contentAlignment = Alignment.Center,
            ) {
                when (phase) {
                    HoldPhase.Loading -> Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = c.textSecondary)
                        Text("Loading the phone's screen…", color = c.textSecondary, fontSize = 13.sp)
                    }
                    HoldPhase.Failed -> Column(
                        Modifier.padding(horizontal = 24.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_image), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(26.dp))
                        Text("Could not load the screen. Approve stays off until you can see it.", color = c.textPrimary, fontSize = 13.sp, textAlign = TextAlign.Center)
                        InboxButton("Try again", prominent = false, enabled = true, testTag = "inboxReviewRetry") { scope.launch { loadCrop() } }
                    }
                    else -> image?.let {
                        androidx.compose.foundation.Image(
                            bitmap = it,
                            contentDescription = "Screenshot of $phoneName at this step",
                            contentScale = androidx.compose.ui.layout.ContentScale.Fit,
                            modifier = Modifier.fillMaxSize().padding(8.dp).clip(RoundedCornerShape(10.dp)),
                        )
                    }
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                phone?.goal?.takeIf { it.isNotBlank() }?.let { HoldDetail("Goal", it) }
                phone?.to?.takeIf { it.isNotBlank() }?.let { HoldDetail("To", it) }
                phone?.text?.takeIf { it.isNotBlank() }?.let { HoldDetail("Message", it) }
                HoldDetail("Action", actionLabel)
            }
            when (phase) {
                HoldPhase.Approved -> HoldOutcome(DesignR.drawable.ph_check_circle_fill, AllternitStatic.StatusSuccess, "Approved. Your bot taps \"$actionLabel\" now.")
                HoldPhase.Denied -> HoldOutcome(DesignR.drawable.ph_x, AllternitStatic.StatusError, "Denied. The task stopped and nothing was done on your phone.")
                HoldPhase.Expired -> HoldOutcome(com.allternit.core.design.R.drawable.ph_clock, c.textSecondary, "Expired. Nobody answered in 5 minutes, so the task ended and nothing was done.")
                HoldPhase.Changed -> {
                    HoldOutcome(DesignR.drawable.ph_arrows_down_up, AllternitStatic.StatusWarning, "Changed. The screen moved since your bot asked. Check the new screen, then decide.")
                    HoldButtons(approveEnabled = false, busy = busy, onDeny = { decide(false) }, onApprove = { decide(true) })
                }
                else -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (phase != HoldPhase.Shown) Text("Approve turns on when the screen has loaded.", color = c.textSecondary, fontSize = 13.sp)
                    error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
                    HoldButtons(approveEnabled = phase == HoldPhase.Shown, busy = busy, onDeny = { decide(false) }, onApprove = { decide(true) })
                }
            }
        }
    }
}

@Composable
private fun HoldDetail(title: String, value: String) {
    val c = AllternitTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(title, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium, modifier = Modifier.width(64.dp))
        Text(value, color = c.textPrimary, fontSize = 13.sp)
    }
}

@Composable
private fun HoldOutcome(icon: Int, tint: Color, text: String) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
        Text(text, color = c.textPrimary, fontSize = 15.sp)
    }
}

/** Deny (outlined) and Approve (ink on paper in both themes, iOS). */
@Composable
private fun HoldButtons(approveEnabled: Boolean, busy: Boolean, onDeny: () -> Unit, onApprove: () -> Unit) {
    val c = AllternitTheme.colors
    val shape = RoundedCornerShape(12.dp)
    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(
            Modifier.weight(1f).heightIn(min = 44.dp).clip(shape).background(c.fillNeutral).border(1.dp, c.borderSubtle, shape)
                .testTag("inboxReviewDeny").clickable(enabled = !busy, role = Role.Button, onClick = onDeny),
            contentAlignment = Alignment.Center,
        ) { Text("Deny", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
        Box(
            Modifier.weight(1f).heightIn(min = 44.dp).clip(shape).background(c.textPrimary.copy(alpha = if (approveEnabled) 1f else 0.28f))
                .testTag("inboxReviewApprove").clickable(enabled = approveEnabled && !busy, role = Role.Button, onClick = onApprove)
                .semantics { if (!approveEnabled) stateDescription = "Turns on when the screen has loaded" },
            contentAlignment = Alignment.Center,
        ) { Text(if (busy) "Working…" else "Approve", color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
    }
}

/** cloud-api's account-state refusals (428 "No data-plane node", compute_required, free_computers_retired): no computer, not a timeout. */
private fun isComputeGateText(message: String?): Boolean =
    message != null && (Regex("""\b428\b""").containsMatchIn(message) || com.allternit.core.api.NoComputerRule.isGate(null, null, message) || Regex("""\b(compute_required|free_computers_retired)\b""", RegexOption.IGNORE_CASE).containsMatchIn(message))
