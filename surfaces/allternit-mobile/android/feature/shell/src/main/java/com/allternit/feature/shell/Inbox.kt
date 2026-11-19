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
                )
            }
            return InboxItem(
                id = o.s("id") ?: return null,
                kind = o.s("kind").orEmpty(),
                botId = o.s("botId").orEmpty(),
                botName = o.s("botName").orEmpty(),
                person = o.s("person"),
                summary = o.s("summary").orEmpty(),
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
    val failures = mutableStateMapOf<String, String>()
    val busy = mutableStateMapOf<String, Boolean>()

    suspend fun refresh() {
        when (val r = backend.runtimeJson("GET", "/api/v1/inbox?scope=needs_you")) {
            is RuntimeCall.Ok -> {
                items = ((r.body as? JsonObject)?.get("needsYou") as? JsonArray).orEmpty()
                    .mapNotNull { (it as? JsonObject)?.let(InboxItem::from) }
                isAvailable = true
                error = null
            }
            RuntimeCall.NoComputer -> {
                items = emptyList()
                isAvailable = false
                error = null
            }
            is RuntimeCall.Http -> if (r.status == 404 || r.status == 503) {
                items = emptyList()
                isAvailable = false
                error = null
            } else {
                error = statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "load the inbox")
            }
            is RuntimeCall.Failed -> error = r.message
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
    suspend fun snooze(item: InboxItem) = act(item, "snooze", buildJsonObject { put("minutes", 60) })
}

// MARK: - Sheet (iOS InboxSheet)

private val kindWords = mapOf(
    "email_draft" to "Email", "approval" to "Approval", "held_send" to "Held message", "thread" to "Waiting",
    "missed_call" to "Missed call", "unanswered" to "Unanswered", "failed_send" to "Not delivered",
    "vendor_result" to "Vendor result", "memory_import" to "Memory", "attention" to "Needs fixing",
    "suggestion" to "Suggested",
)

private val trustWords = mapOf(
    "owner" to "you", "team" to "your team", "contact" to "a contact", "vendorbot" to "a vendor bot",
    "vendor_bot" to "a vendor bot", "tooloutput" to "a tool result", "tool_output" to "a tool result",
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
 * action on the row. iOS swipes (right for the main action, left to snooze or dismiss) are the
 * row's ⋯ button here: Android lists have no two-button swipe. [focusedId] is the item a link or
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
                    title = "No inbox on this computer yet",
                    message = "The inbox comes from your computer. Connect one, or update Allternit on it, to see what needs you.",
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
                ) { Icon(painterResource(R.drawable.shell_bell_slash), contentDescription = null, tint = c.textSecondary) }
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
                        items(store.items, key = { it.id }) { item ->
                            InboxRow(
                                item = item,
                                store = store,
                                focused = item.id == focusedId,
                                onPrimary = { primary(item) },
                                onDeny = { scope.launch { if (item.isPeerHeld) store.dismiss(item) else store.deny(item) } },
                                onSnooze = { scope.launch { store.snooze(item) } },
                                onDismissItem = { scope.launch { store.dismiss(item) } },
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
        PhoneHoldReview(
            item = item,
            onApprove = {
                reviewing = null
                scope.launch { store.approveReviewed(item) }
            },
            onDeny = {
                reviewing = null
                scope.launch { store.deny(item) }
            },
            onClose = { reviewing = null },
        )
    }
}

@Composable
private fun InboxRow(
    item: InboxItem,
    store: InboxStore,
    focused: Boolean,
    onPrimary: () -> Unit,
    onDeny: () -> Unit,
    onSnooze: () -> Unit,
    onDismissItem: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val busy = store.busy[item.id] == true
    var menu by remember { mutableStateOf(false) }
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
        modifier = Modifier
            .fillMaxWidth()
            .background(if (focused) c.accentPrimary.copy(alpha = 0.12f) else Color.Transparent)
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
            Box {
                Box(
                    Modifier
                        .size(32.dp)
                        .clip(CircleShape)
                        .clickable(role = Role.Button, onClickLabel = "More") { menu = true }
                        .semantics { contentDescription = "More for ${item.summary}" },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(painterResource(R.drawable.shell_dots), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(16.dp))
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }, shape = RoundedCornerShape(16.dp), containerColor = c.bgPrimary) {
                    MenuRow(R.drawable.shell_clock, "Snooze", testTag = "inboxSnooze") {
                        menu = false
                        onSnooze()
                    }
                    MenuRow(DesignR.drawable.ph_x, "Dismiss", testTag = "inboxDismiss") {
                        menu = false
                        onDismissItem()
                    }
                }
            }
        }
        Text(item.summary, color = c.textPrimary, fontSize = 15.sp)
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
        Row(Modifier.padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            InboxButton(
                title = if (busy) "Working…" else if (item.isPhoneAction) "Review" else item.primaryLabel,
                prominent = true,
                enabled = !busy,
                testTag = "inboxPrimary${item.id}",
            ) {
                Haptics.perform(view, Haptics.Kind.Tap)
                onPrimary()
            }
            if (!item.isPhoneAction && item.canApprove && item.primaryAction == "approve") {
                InboxButton(title = denyLabel, prominent = false, enabled = !busy, testTag = "inboxDeny${item.id}", onClick = onDeny)
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
 * A phone hold's Review (iOS PhoneHoldReviewSheet, short form): what the bot wants to do on which
 * phone, then Approve or Deny. Approve sends the hash this pane showed.
 */
@Composable
private fun PhoneHoldReview(item: InboxItem, onApprove: () -> Unit, onDeny: () -> Unit, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val p = item.phone
    val lines = listOfNotNull(
        p?.phoneName?.let { "Phone: $it" },
        p?.app?.let { "App: $it" },
        (p?.goal ?: p?.intent)?.let { "Wants to: $it" },
        p?.element?.let { "Tap: $it" },
        p?.to?.let { "To: $it" },
        p?.text?.let { "Text: “$it”" },
    )
    AlertDialog(
        onDismissRequest = onClose,
        containerColor = c.bgPrimary,
        title = { Text("Review", color = c.textPrimary) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag("inboxReview")) {
                Text(item.summary, color = c.textPrimary, fontSize = 15.sp)
                lines.forEach { Text(it, color = c.textSecondary, fontSize = 13.sp) }
            }
        },
        confirmButton = { TextButton(onClick = onApprove, modifier = Modifier.testTag("inboxReviewApprove")) { Text("Approve", color = c.textPrimary) } },
        dismissButton = { TextButton(onClick = onDeny, modifier = Modifier.testTag("inboxReviewDeny")) { Text("Deny", color = AllternitStatic.StatusError) } },
    )
}
