@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.allternit.feature.chat

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import okhttp3.OkHttpClient
import okhttp3.Request

// MARK: - Shared chrome

/** iOS AnswerCardChrome: BgPrimary, 12dp corners, a 1dp subtle border, 12dp padding. */
@Composable
internal fun CardChrome(modifier: Modifier = Modifier, border: Color? = null, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(c.bgPrimary)
            .border(BorderStroke(1.dp, border ?: c.borderSubtle), RoundedCornerShape(12.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) { content() }
}

@Composable
internal fun CardHeader(icon: Int, title: String, tint: Color = AllternitTheme.colors.textSecondary) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(14.dp))
        Text(title, color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** The capsule action button the cards share (iOS ChatErrorCardView / QuestionCardButtonStyle). */
@Composable
internal fun CapsuleButton(title: String, icon: Int? = null, primary: Boolean = false, enabled: Boolean = true, label: String? = null, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier
            .heightIn(min = 32.dp)
            .clip(CircleShape)
            .background(if (primary) c.accentPrimary else c.fillNeutral)
            .then(if (primary) Modifier else Modifier.border(1.dp, c.borderSubtle, CircleShape))
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = label ?: title }
            .padding(horizontal = 12.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        val ink = if (primary) c.onAccent else c.textPrimary
        if (icon != null) Icon(painterResource(icon), contentDescription = null, tint = ink.copy(alpha = if (enabled) 1f else 0.4f), modifier = Modifier.size(11.dp))
        Text(title, color = ink.copy(alpha = if (enabled) 1f else 0.4f), fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
    }
}

internal fun copyToClipboard(context: Context, text: String) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    cm.setPrimaryClip(ClipData.newPlainText("Allternit", text))
}

internal fun openUrl(context: Context, url: String) {
    if (!url.startsWith("https://") && !url.startsWith("http://") && !url.startsWith("mailto:") && !url.startsWith("sms:") && !url.startsWith("geo:")) return
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

internal fun shareText(context: Context, text: String) {
    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text)
    runCatching { context.startActivity(Intent.createChooser(send, null).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

// MARK: - Transient banner (iOS TransientErrorBanner)

@Composable
internal fun TransientBanner(message: String, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 14.dp, vertical = 8.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(c.fillNeutral)
            .padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_warning), contentDescription = null, tint = AllternitStatic.StatusError, modifier = Modifier.size(14.dp))
        Text(message, color = c.textPrimary, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Box(
            Modifier.size(32.dp).clickable(role = Role.Button, onClick = onDismiss).semantics { contentDescription = "Dismiss" },
            contentAlignment = Alignment.Center,
        ) { Icon(painterResource(DesignR.drawable.ph_x), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(12.dp)) }
    }
}

// MARK: - Error card (iOS ChatErrorCardView)

/** Same URL as feature:shell ConnectionsCatalog.CLAUDE_LINKED_PLATFORMS_URL (chat can't see the shell module). */
private const val CLAUDE_LINKED_PLATFORMS_URL = "https://platform.claude.com/linked-platforms"

@Composable
internal fun ChatErrorCard(
    error: ChatError,
    onChooseModel: () -> Unit,
    onRetry: () -> Unit,
    /** Draws only these actions (credits is left out when no Cloud model is listed). */
    availableActions: Set<ChatErrorAction> = emptySet(),
    onAction: (ChatErrorAction) -> Unit = {},
    /** Typed errors only: "Connect ChatGPT" (true) opens the device sign-in, "Open Connections" (false) the Connections page. */
    onConnect: (Boolean) -> Unit = {},
) {
    val uri = androidx.compose.ui.platform.LocalUriHandler.current
    val c = AllternitTheme.colors
    // A typed error frame (errors B): the reason, when a limit resets, and one way out. Exactly one card.
    error.typed?.let { info ->
        CardChrome(Modifier.widthIn(max = 320.dp), border = AllternitStatic.StatusWarning.copy(alpha = 0.4f)) {
            ErrorBody(info, ChatErrorInfo.Lane.Chat, onChooseModel, onConnect, onRetry, center = false)
        }
        return
    }
    var details by remember { mutableStateOf(false) }
    CardChrome(Modifier.widthIn(max = 320.dp), border = AllternitStatic.StatusWarning.copy(alpha = 0.4f)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(DesignR.drawable.ph_warning), contentDescription = null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(16.dp))
            Text(error.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
        Text(error.detail, color = c.textSecondary, fontSize = 12.sp, lineHeight = 16.sp)
        val shown = error.claudeActions.filter { it in availableActions }
        shown.forEach { a ->
            CapsuleButton(a.title, primary = a == ChatErrorAction.UseCredits, label = a.title, modifier = Modifier.testTag(a.tag)) { onAction(a) }
        }
        // iOS: a reached spend limit also points at Claude's Linked platforms page, where it is raised.
        if (error.claudeLinkCode == "claude_link_spend_limit") {
            CapsuleButton("Linked platforms", label = "Open Claude's Linked platforms page", modifier = Modifier.testTag("error-linked-platforms")) {
                runCatching { uri.openUri(CLAUDE_LINKED_PLATFORMS_URL) }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (error.isModelError) CapsuleButton("Choose a model", icon = DesignR.drawable.ph_cpu, onClick = onChooseModel)
            CapsuleButton("Retry", icon = com.allternit.core.design.R.drawable.ph_arrow_clockwise, primary = !error.isModelError, onClick = onRetry)
        }
        Row(
            Modifier.clickable(role = Role.Button) { details = !details },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text("Details", color = c.textSecondary, fontSize = 11.sp)
            Icon(painterResource(if (details) com.allternit.core.design.R.drawable.ph_caret_up else com.allternit.core.design.R.drawable.ph_caret_down), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(9.dp))
        }
        if (details) Text(error.raw, color = c.textSecondary, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
    }
}

// MARK: - Permission ask (iOS PermissionAskView)

@Composable
fun PermissionAskCard(ask: PermissionAsk, onReply: (String, (String) -> Unit) -> Unit) {
    val c = AllternitTheme.colors
    var problem by remember(ask.id) { mutableStateOf<String?>(null) }
    var sending by remember(ask.id) { mutableStateOf(false) }
    if (ask.answered != null) return
    CardChrome {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(com.allternit.core.design.R.drawable.ph_hand), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
            Text("Waiting for approval", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
        }
        Text(ask.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        ask.patterns.firstOrNull { it.isNotEmpty() && it != "*" }?.let {
            Text(it, color = c.textSecondary, fontSize = 13.sp, fontFamily = FontFamily.Monospace, maxLines = 3, overflow = TextOverflow.Ellipsis)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            fun reply(value: String) {
                sending = true
                problem = null
                onReply(value) { msg ->
                    sending = false
                    problem = msg
                }
            }
            CapsuleButton("Deny", enabled = !sending, label = "Deny: ${ask.title}") { reply("reject") }
            if (PermissionAsks.allowsAlways(ask.id)) CapsuleButton("Always", enabled = !sending, label = "Always: ${ask.title}") { reply("always") }
            CapsuleButton("Allow", primary = true, enabled = !sending, label = "Allow: ${ask.title}") { reply("once") }
        }
        problem?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
    }
}

// MARK: - Question (iOS QuestionCardView)

@Composable
fun QuestionCard(ask: QuestionAsk, onAnswer: (List<List<String>>, (String) -> Unit) -> Unit, onSkip: ((String) -> Unit) -> Unit) {
    val c = AllternitTheme.colors
    if (ask.closed) return
    val picks = remember(ask.id) { mutableStateListOf<Set<String>>().apply { repeat(ask.questions.size) { add(emptySet()) } } }
    var problem by remember(ask.id) { mutableStateOf<String?>(null) }
    val planAt = ask.questions.indexOfFirst { it.plan != null }
    if (planAt >= 0) {
        // A plan question draws a plan card; the sheet answers it (Approve = Yes, Revise / Ask = No).
        val q = ask.questions[planAt]
        var open by remember(ask.id) { mutableStateOf(false) }
        PlanCard(q.plan!!) { open = true }
        if (open) PlanSheet(q.plan, onDismiss = { open = false }) { action, onError ->
            val label = planReplyLabel(q.options, action)
            onAnswer(ask.questions.indices.map { if (it == planAt) listOfNotNull(label) else emptyList() }) { onError(it) }
        }
        return
    }
    CardChrome {
        CardHeader(com.allternit.core.design.R.drawable.ph_question, "Your agent is asking")
        ask.questions.forEachIndexed { index, q ->
            q.header?.takeIf { it.isNotBlank() }?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold) }
            Text(q.question, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                q.options.forEach { option ->
                    val on = option in picks[index]
                    CapsuleButton(option, primary = on, label = option) {
                        picks[index] = if (q.multiple) (if (on) picks[index] - option else picks[index] + option) else setOf(option)
                    }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            CapsuleButton("Skip") { onSkip { problem = it } }
            CapsuleButton("Send", primary = true, enabled = picks.all { it.isNotEmpty() }) {
                onAnswer(picks.map { it.toList() }) { problem = it }
            }
        }
        problem?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
    }
}

// MARK: - Tool calls (iOS ToolCallsRail + ToolDescriber)

internal object ToolDescriber {
    private fun normalize(name: String): String {
        var n = name.lowercase().removePrefix("tool-")
        if (n.startsWith("mcp__")) n = n.substringAfter("mcp__").substringAfter("__")
        return n.replace("_", "").replace("-", "").replace(".", "")
    }

    fun normalized(name: String): String = normalize(name)

    /** The verb for a call: "Searching the web" while running, "Searched the web" when done. */
    fun describe(call: ToolCallRecord): String {
        val running = call.state == ToolState.Running
        fun v(now: String, past: String) = if (running) now else past
        val n = normalize(call.toolName)
        return when {
            n.contains("websearch") || n == "search" || n == "bravesearch" -> v("Searching the web", "Searched the web")
            n.contains("fetch") || n.contains("url") -> v("Reading", "Read")
            n == "bash" || n == "shell" || n.contains("exec") || n == "runcode" -> call.input?.str("description") ?: v("Running", "Ran")
            n == "edit" || n == "multiedit" || n == "patch" -> v("Editing", "Edited")
            n == "write" -> v("Creating", "Created")
            n == "read" || n == "readfile" -> v("Reading", "Read")
            n == "grep" -> v("Searching code", "Searched code")
            n == "glob" || n == "ls" || n == "list" -> v("Finding files", "Found files")
            n == "sleep" -> v("Waiting", "Waited")
            n == "messageagent" -> v("Handing to", "Handed to")
            n == "question" -> v("Asking you", "Asked you")
            n == "task" || n == "agent" || n == "delegate" -> (call.input?.str("subagent_type") ?: "General").replaceFirstChar { it.uppercase() }
            else -> v("Using ", "Used ") + humanize(call.toolName)
        }
    }

    fun humanize(name: String): String {
        val base = name.substringAfterLast("__").replace('_', ' ').replace('-', ' ').trim()
        return base.replaceFirstChar { it.uppercase() }
    }

    fun detail(call: ToolCallRecord): String? {
        val i = call.input ?: return call.title
        return call.title ?: i.str("query") ?: i.str("url") ?: i.str("command") ?: i.str("filePath") ?: i.str("path") ?: i.str("pattern")
    }
}

internal fun formatDuration(ms: Long): String = if (ms < 1000) "${ms}ms" else String.format(java.util.Locale.US, "%.1fs", ms / 1000.0)

@Composable
internal fun ToolCallsRail(calls: List<ToolCallRecord>, streaming: Boolean, waiting: Boolean, backend: ChatBackend? = null) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    // Wave E: a reply that handed work to other agents opens the run tree (iOS AgentRunTreeSheet from MessageRow).
    val agentSteps = calls.count { isAgentStep(it) }
    var treeOpen by remember { mutableStateOf(false) }
    val treeSession = LocalRunTreeSession.current
    if (treeOpen) AgentRunTreeSheet(calls, treeSession, streaming, backend, onDismiss = { treeOpen = false })
    val failed = calls.count { it.state == ToolState.Failed }
    val n = calls.size
    val noun = if (n == 1) "tool" else "tools"
    val starts = calls.mapNotNull { it.startedAt }
    val ends = calls.mapNotNull { it.finishedAt }
    val elapsed = if (starts.isNotEmpty() && ends.isNotEmpty()) ends.max() - starts.min() else 0L
    val header = buildString {
        append(
            when {
                streaming && waiting -> "Waiting for approval"
                streaming -> "Using $n $noun"
                else -> "Used $n $noun"
            },
        )
        if (!streaming && elapsed >= 1000) append(" · ${formatDuration(elapsed)}")
        if (failed > 0) append(" · $failed failed")
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            Modifier.clickable(role = Role.Button) { open = !open },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (streaming) com.allternit.core.design.ThinkingOrb(size = com.allternit.core.design.ThinkingOrbSize.Inline, state = com.allternit.core.design.ThinkingOrbState.forToolName(calls.lastOrNull()?.toolName ?: ""))
            Text(header, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
            Icon(painterResource(if (open) com.allternit.core.design.R.drawable.ph_caret_up else com.allternit.core.design.R.drawable.ph_caret_down), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(9.dp))
            if (agentSteps > 0) Text(
                "· $agentSteps ${if (agentSteps == 1) "agent" else "agents"}", color = c.accentPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.testTag("toolRailAgents").clickable(role = Role.Button, onClickLabel = "Show the agents") { treeOpen = true },
            )
        }
        if (open) {
            calls.forEach { call ->
                val verb = ToolDescriber.describe(call)
                val detail = ToolDescriber.detail(call)
                val stateLabel = when (call.state) { ToolState.Running -> "running"; ToolState.Failed -> "failed"; ToolState.Done -> "done" }
                Row(
                    Modifier.padding(start = 6.dp).semantics(mergeDescendants = true) { contentDescription = "$verb ${detail.orEmpty()}, $stateLabel" },
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    val (icon, tint) = when (call.state) {
                        ToolState.Done -> DesignR.drawable.ph_check to c.accentPrimary
                        ToolState.Failed -> DesignR.drawable.ph_warning to AllternitStatic.StatusWarning
                        ToolState.Running -> DesignR.drawable.ph_record_fill to c.textSecondary
                    }
                    Icon(painterResource(icon), contentDescription = null, tint = tint, modifier = Modifier.size(12.dp))
                    Text(verb, color = c.textPrimary, fontSize = 13.sp)
                    if (!detail.isNullOrBlank()) Text(detail, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                call.error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.padding(start = 24.dp), maxLines = 3) }
            }
        }
    }
}

/** The single status line when a reply has no tool-call list (iOS toolStatus fallback). */
@Composable
internal fun ToolStatusLine(status: ToolStatus) {
    val c = AllternitTheme.colors
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        when (status.state) {
            ToolState.Running -> com.allternit.core.design.ThinkingOrb(size = com.allternit.core.design.ThinkingOrbSize.Inline, state = com.allternit.core.design.ThinkingOrbState.forToolName(status.toolName))
            ToolState.Done -> Icon(painterResource(DesignR.drawable.ph_check), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
            ToolState.Failed -> Icon(painterResource(DesignR.drawable.ph_warning), contentDescription = null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(16.dp))
        }
        Text(status.text, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        status.elapsedMs?.let { Text("· ${formatDuration(it)}", color = c.textSecondary.copy(alpha = 0.7f), fontSize = 11.sp) }
    }
}

// MARK: - Settings change (iOS SettingsChangeCardView, AgentSettingsResult.parse)

internal data class SettingsChange(val key: String, val outcome: String, val label: String, val scope: String?, val value: String?, val previous: String?, val reason: String?)

internal object SettingsChanges {
    private val outcomes = setOf("applied", "apply_on_device", "needs_confirmation", "locked", "invalid")
    private val labels = mapOf("appearance.theme" to "Theme")

    fun parse(call: ToolCallRecord): List<SettingsChange> {
        if (ToolDescriber.normalized(call.toolName) != "settings") return emptyList()
        val source = call.metadata?.takeIf { it["changes"] is JsonArray }
            ?: call.output?.let { runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() }
            ?: return emptyList()
        if (source.str("action") == "read") return emptyList()
        return source.arr("changes").mapNotNull { el ->
            val o = el as? JsonObject ?: return@mapNotNull null
            val key = o.str("key") ?: return@mapNotNull null
            val outcome = o.str("outcome")?.takeIf { it in outcomes } ?: return@mapNotNull null
            SettingsChange(
                key = key,
                outcome = outcome,
                label = o.str("label") ?: labels[key] ?: key.substringAfterLast('.').replace('_', ' ').replaceFirstChar { it.uppercase() },
                scope = o.str("scope") ?: if (key.startsWith("gizzi.")) "runtime" else null,
                value = display(o["value"]),
                previous = display(o["previous"]),
                reason = o.str("reason"),
            )
        }
    }

    fun display(v: JsonElement?): String? = when (v) {
        null, JsonNull -> null
        is JsonArray -> if (v.isEmpty()) "None" else v.mapNotNull { display(it) }.joinToString(", ")
        is JsonPrimitive -> v.booleanOrNull?.let { if (it) "On" else "Off" }
            ?: if (v.isString) v.content.replace('_', ' ').replaceFirstChar { it.uppercase() } else v.contentOrNull
        is JsonObject -> v.toString()
    }
}

@Composable
internal fun SettingsChangeCard(changes: List<SettingsChange>, onOpenSettings: () -> Unit) {
    val c = AllternitTheme.colors
    CardChrome(Modifier.semantics(mergeDescendants = true) { contentDescription = "Settings changes" }) {
        CardHeader(com.allternit.core.design.R.drawable.ph_gear, if (changes.size == 1) "Settings change" else "Settings changes")
        changes.forEachIndexed { index, ch ->
            if (index > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(ch.label, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                val status = when (ch.outcome) {
                    "applied" -> "Changed"
                    "apply_on_device" -> if (ch.scope == "runtime") "In Gizzi Code" else "For each device"
                    "needs_confirmation" -> "Needs your OK"
                    "locked" -> "In Settings only"
                    else -> "Not changed"
                }
                Text(status, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 3.dp))
            }
            val line = when {
                ch.previous != null && ch.value != null && ch.previous != ch.value -> "${ch.previous} → ${ch.value}"
                else -> ch.value
            }
            line?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
            val note = when (ch.outcome) {
                "invalid" -> ch.reason ?: "This change couldn't be made."
                "locked", "needs_confirmation" -> ch.reason ?: "Change this in Settings."
                "apply_on_device" -> if (ch.scope == "runtime") "Gizzi Code applies this on the computer it runs on." else "This phone doesn't keep this setting. Change it in Settings."
                else -> null
            }
            note?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
            if (ch.outcome == "locked" || ch.outcome == "needs_confirmation" || (ch.outcome == "apply_on_device" && ch.scope != "runtime")) {
                CapsuleButton("Open Settings", icon = com.allternit.core.design.R.drawable.ph_gear, label = "Open Settings for ${ch.label}", onClick = onOpenSettings)
            }
        }
    }
}

// MARK: - Connector suggestion (iOS ConnectSuggestCardView, AgentConnectorSuggestion.parse)

internal data class ConnectorSuggestion(val reason: String?, val connectors: List<Triple<String, String, String?>>, val states: Map<String, String?>)

internal object ConnectorSuggestions {
    fun parse(call: ToolCallRecord): ConnectorSuggestion? {
        if (ToolDescriber.normalized(call.toolName) != "connectorsuggest") return null
        val json = call.metadata?.takeIf { it["card"] is JsonObject }
            ?: call.output?.let { runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(it) as? JsonObject }.getOrNull() }
            ?: return null
        val card = json.obj("card") ?: json
        var rows = card.arr("connectors").mapNotNull { el ->
            val o = el as? JsonObject ?: return@mapNotNull null
            val id = o.str("id") ?: return@mapNotNull null
            Triple(id, o.str("name") ?: id, o.str("group"))
        }
        val states = card.arr("connectors").mapNotNull { (it as? JsonObject)?.let { o -> (o.str("id") ?: return@mapNotNull null) to o.str("state") } }.toMap()
        if (rows.isEmpty()) {
            val ids = (json.arr("ids").ifEmpty { json.arr("shown") }).mapNotNull { (it as? JsonPrimitive)?.contentOrNull }
            val names = call.title?.removePrefix("Suggested ")?.split(", ")
            rows = ids.mapIndexed { i, id -> Triple(id, names?.takeIf { it.size == ids.size }?.get(i) ?: id, null) }
        }
        if (rows.isEmpty()) return null
        return ConnectorSuggestion(card.str("reason") ?: json.str("reason") ?: call.input?.str("reason"), rows, states)
    }
}

@Composable
internal fun ConnectSuggestCard(s: ConnectorSuggestion, onConnect: (String) -> Unit) {
    val c = AllternitTheme.colors
    val title = if (s.connectors.size == 1) "Suggested connection" else "Suggested connections"
    CardChrome(Modifier.semantics(mergeDescendants = false) { contentDescription = listOfNotNull("Suggested connections", s.reason).joinToString(". ") }) {
        CardHeader(com.allternit.core.design.R.drawable.ph_plugs, title)
        s.reason?.let { Text(it, color = c.textPrimary, fontSize = 15.sp) }
        s.connectors.forEachIndexed { index, (id, name, group) ->
            if (index > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    group?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
                }
                when (s.states[id]) {
                    "connected" -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        Icon(painterResource(DesignR.drawable.ph_check_circle_fill), contentDescription = null, tint = AllternitStatic.StatusSuccess, modifier = Modifier.size(14.dp))
                        Text("Connected", color = c.textSecondary, fontSize = 12.sp)
                    }
                    "unavailable" -> Text("Not available here", color = c.textSecondary, fontSize = 12.sp)
                    else -> CapsuleButton("Connect", label = "Connect $name") { onConnect(id) }
                }
            }
        }
    }
}

// MARK: - Remote images (no image library in the catalog: OkHttp + BitmapFactory)

private val imageCache = android.util.LruCache<String, ImageBitmap>(24)

@Composable
internal fun RemoteImage(url: String, client: OkHttpClient, request: Request? = null, modifier: Modifier, contentScale: ContentScale = ContentScale.Fit, description: String? = null, onFail: (() -> Unit)? = null) {
    var bitmap by remember(url) { mutableStateOf<ImageBitmap?>(imageCache.get(url)) }
    LaunchedEffect(url) {
        if (bitmap != null) return@LaunchedEffect
        val loaded = withContext(Dispatchers.IO) {
            runCatching {
                client.newCall(request ?: Request.Builder().url(url).build()).execute().use { resp ->
                    if (!resp.isSuccessful) return@use null
                    val bytes = resp.body?.bytes() ?: return@use null
                    BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
                }
            }.getOrNull()
        }
        if (loaded != null) {
            imageCache.put(url, loaded)
            bitmap = loaded
        } else {
            onFail?.invoke()
        }
    }
    val b = bitmap
    if (b != null) {
        Image(b, contentDescription = description, modifier = modifier, contentScale = contentScale)
    } else {
        Box(modifier.background(AllternitTheme.colors.fillNeutral), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 1.5.dp, color = AllternitTheme.colors.textSecondary)
        }
    }
}

// MARK: - run_code files (iOS RunCodeFilesView + UploadFileCard)

internal data class RunFile(val name: String, val mediaType: String, val uploadId: String, val size: Long?)

@Composable
internal fun RunCodeFilesView(call: ToolCallRecord, backend: ChatBackend) {
    val c = AllternitTheme.colors
    val files = (call.files as? JsonArray).orEmpty().mapNotNull { el ->
        val o = el as? JsonObject ?: return@mapNotNull null
        RunFile(o.str("name") ?: "file", o.str("mediaType") ?: "application/octet-stream", o.str("uploadId") ?: return@mapNotNull null, o.long("size"))
    }
    val run = call.run
    if (files.isEmpty() && run == null) return
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (run != null) {
            val timedOut = run.bool("timed_out") == true
            val exit = run.long("exit_code")
            val skipped = run.arr("skipped").mapNotNull { it as? JsonObject }
            val ok = !timedOut && exit == 0L
            val summary = buildString {
                append(if (timedOut) "Timed out" else "Exit ${exit ?: "?"}")
                if (skipped.isNotEmpty()) append(" · ${skipped.size} file${if (skipped.size == 1) "" else "s"} skipped")
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(painterResource(if (ok) DesignR.drawable.ph_check else DesignR.drawable.ph_warning), contentDescription = null, tint = if (ok) c.accentPrimary else AllternitStatic.StatusWarning, modifier = Modifier.size(12.dp))
                Text(summary, color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
            }
            skipped.forEach { s -> Text("${s.str("name").orEmpty()}: ${s.str("reason").orEmpty()}", color = c.textSecondary, fontSize = 11.sp) }
        }
        files.forEach { f ->
            val isImage = f.mediaType in setOf("image/png", "image/jpeg", "image/gif", "image/webp")
            var failed by remember(f.uploadId) { mutableStateOf(false) }
            if (isImage && !failed) {
                RemoteImage(
                    url = "upload:${f.uploadId}",
                    client = backend.authedHttp,
                    request = backend.uploadRequest(f.uploadId),
                    modifier = Modifier.fillMaxWidth().heightIn(max = 320.dp).clip(RoundedCornerShape(12.dp)),
                    description = "${f.name}",
                    onFail = { failed = true },
                )
            } else {
                UploadFileCard(f, backend)
            }
        }
    }
}

/** A file from the agent (iOS UploadFileCard). Android saves it where the person picks (Share on iOS). */
@Composable
internal fun UploadFileCard(f: RunFile, backend: ChatBackend) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var problem by remember(f.uploadId) { mutableStateOf(false) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument(f.mediaType)) { target ->
        if (target == null) return@rememberLauncherForActivityResult
        Thread {
            val ok = runCatching {
                backend.authedHttp.newCall(backend.uploadRequest(f.uploadId)).execute().use { resp ->
                    val bytes = resp.body?.bytes()
                    if (!resp.isSuccessful || bytes == null) return@use false
                    context.contentResolver.openOutputStream(target)?.use { it.write(bytes) } != null
                }
            }.getOrDefault(false)
            if (!ok) problem = true
        }.start()
    }
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(painterResource(com.allternit.core.design.R.drawable.ph_file), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text(f.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                if (problem) "Couldn't download this file." else f.size?.let { android.text.format.Formatter.formatShortFileSize(context, it) } ?: "",
                color = if (problem) AllternitStatic.StatusError else c.textSecondary,
                fontSize = 12.sp,
            )
        }
        CapsuleButton("Save", icon = DesignR.drawable.ph_floppy_disk, label = "Save ${f.name}") { save.launch(f.name) }
    }
}

// MARK: - Answer cards (iOS AnswerCardView: lookup metadata.card, or an ```openui fence)

@Composable
internal fun AnswerCard(card: JsonObject, fallbackText: String?, backend: ChatBackend) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val template = card.str("template") ?: "other"
    val data = card.obj("data") ?: JsonObject(emptyMap())
    val title = card.str("title")
    fun credit(): (@Composable () -> Unit)? {
        val text = data.str("credit") ?: return null
        val url = data.str("creditUrl")?.takeIf { it.startsWith("https://") }
        return {
            Text(
                text,
                color = c.textSecondary,
                fontSize = 11.sp,
                modifier = if (url != null) Modifier.clickable(role = Role.Button) { openUrl(context, url) } else Modifier,
            )
        }
    }
    if (template == "writing") {
        WritingDraftCard(data)
        return
    }
    CardChrome {
        title?.let { Text(it, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
        when (template) {
            "weather" -> {
                val unit = data.str("unit") ?: "C"
                Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    data.text("temperature")?.let { Text("$it°$unit", color = c.textPrimary, fontSize = 34.sp, fontWeight = FontWeight.Light) }
                    Column {
                        data.str("location")?.let { Text(it, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
                        data.str("conditions")?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
                    }
                }
                val extra = listOfNotNull(data.text("humidity")?.let { "Humidity $it%" }, data.text("wind")?.let { "Wind $it" }).joinToString(" · ")
                if (extra.isNotEmpty()) Text(extra, color = c.textSecondary, fontSize = 12.sp)
                data.arr("forecast").mapNotNull { it as? JsonObject }.takeIf { it.isNotEmpty() }?.let { days ->
                    Column(Modifier.semantics { contentDescription = "Forecast" }, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        days.forEach { d ->
                            Row {
                                Text(d.str("day").orEmpty(), color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.width(56.dp))
                                Text(d.str("conditions").orEmpty(), color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f), maxLines = 1)
                                Text("${d.text("high") ?: "–"}° / ${d.text("low") ?: "–"}°", color = c.textPrimary, fontSize = 13.sp)
                            }
                        }
                    }
                }
            }
            "time-zones" -> data.arr("zones").mapNotNull { it as? JsonObject }.forEach { z ->
                Row {
                    Column(Modifier.weight(1f)) {
                        Text(z.str("place") ?: z.str("zone").orEmpty(), color = c.textPrimary, fontSize = 15.sp)
                        Text(listOfNotNull(z.str("date"), z.str("offset")).joinToString(" · "), color = c.textSecondary, fontSize = 12.sp)
                    }
                    Text(z.str("time").orEmpty(), color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                }
            }
            "conversion" -> {
                val from = data.obj("from")
                val to = data.obj("to")
                if (title == null) Text("Conversion", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text("${from?.text("value").orEmpty()} ${from?.str("unit").orEmpty()} = ${to?.text("value").orEmpty()} ${to?.str("unit").orEmpty()}", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                val rate = listOfNotNull(data.text("rate")?.let { "Rate $it" }, data.str("asOf")?.let { "as of $it" }).joinToString(" · ")
                if (rate.isNotEmpty()) Text(rate, color = c.textSecondary, fontSize = 12.sp)
                data.arr("also").mapNotNull { (it as? JsonObject)?.let { o -> "${o.text("value").orEmpty()} ${o.str("unit").orEmpty()}" } ?: (it as? JsonPrimitive)?.contentOrNull }
                    .takeIf { it.isNotEmpty() }?.let { Text("Also: " + it.joinToString(", "), color = c.textSecondary, fontSize = 12.sp) }
            }
            "link-preview" -> {
                val url = data.str("url")
                Column(
                    Modifier.clickable(enabled = url != null, role = Role.Button, onClickLabel = "Opens the page in your browser") { url?.let { openUrl(context, it) } },
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    data.str("image")?.takeIf { it.startsWith("https://") }?.let {
                        RemoteImage(it, backend.publicHttp, modifier = Modifier.fillMaxWidth().height(150.dp).clip(RoundedCornerShape(8.dp)), contentScale = ContentScale.Crop, description = data.str("imageAlt"))
                    }
                    Text(data.str("title") ?: url.orEmpty(), color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
                    data.str("description")?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 3, overflow = TextOverflow.Ellipsis) }
                    url?.let { Text("${Uri.parse(it).host.orEmpty()} ↗", color = c.textSecondary, fontSize = 12.sp) }
                    if (url != null) CapsuleButton("Open page", primary = true, label = "Opens the page in your browser") { openUrl(context, url) }
                }
            }
            "places" -> {
                val places = data.arr("places").mapNotNull { it as? JsonObject }
                val sub = listOfNotNull(data.str("query"), data.str("near")?.let { "near $it" }).joinToString(" · ")
                if (sub.isNotEmpty()) Text(sub, color = c.textSecondary, fontSize = 12.sp)
                if (places.isEmpty()) Text("No places found.", color = c.textSecondary, fontSize = 13.sp)
                places.forEach { p ->
                    val lat = p.double("lat")
                    val lon = p.double("lon")
                    val name = p.str("name").orEmpty()
                    val target = when {
                        lat != null && lon != null -> "geo:$lat,$lon?q=${Uri.encode(name)}"
                        else -> p.str("url")
                    }
                    Column(Modifier.fillMaxWidth().clickable(enabled = target != null, role = Role.Button, onClickLabel = "Opens Maps") { target?.let { openUrl(context, it) } }) {
                        Text(name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        p.str("address")?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
                        val meta = listOfNotNull(p.str("category"), p.text("distance")).joinToString(" · ")
                        if (meta.isNotEmpty()) Text(meta, color = c.textSecondary, fontSize = 12.sp)
                    }
                }
            }
            "image-gallery" -> {
                if (title == null) Text("Images", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    data.arr("images").mapNotNull { it as? JsonObject }.forEach { img ->
                        val url = img.str("url")?.takeIf { it.startsWith("https://") } ?: return@forEach
                        Column(Modifier.width(200.dp).clickable(role = Role.Button, onClickLabel = "Opens the image source") { openUrl(context, img.str("sourceUrl") ?: url) }) {
                            RemoteImage(url, backend.publicHttp, modifier = Modifier.size(200.dp, 150.dp).clip(RoundedCornerShape(8.dp)), contentScale = ContentScale.Crop, description = img.str("alt"))
                            img.str("caption")?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 2) }
                            val cr = listOfNotNull(img.str("credit"), img.str("license")).joinToString(" · ")
                            if (cr.isNotEmpty()) Text(cr, color = c.textSecondary, fontSize = 11.sp, maxLines = 1)
                        }
                    }
                }
            }
            else -> {
                card.str("summary")?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
                fallbackText?.takeIf { it.isNotBlank() }?.let { MarkdownText(it) }
            }
        }
        credit()?.invoke()
    }
}

/** iOS WritingDraftCard: a draft to copy or open in Mail / Messages. Nothing sends from the app. */
@Composable
internal fun WritingDraftCard(data: JsonObject) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val variant = data.str("variant") ?: "note"
    val versions = listOf("Draft" to data.str("body").orEmpty()) +
        data.arr("alternatives").mapNotNull { (it as? JsonObject)?.let { o -> (o.str("label") ?: "Version") to o.str("body").orEmpty() } }
    var picked by remember(data) { mutableStateOf(0) }
    var copied by remember { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            kotlinx.coroutines.delay(1_500)
            copied = false
        }
    }
    val body = versions.getOrNull(picked)?.second.orEmpty()
    val to = data.str("to")
    val subject = data.str("subject")
    CardChrome {
        Text(
            when (variant) { "email" -> "Email"; "text_message" -> "Text message"; "social_post" -> "Social post"; "document" -> "Document"; else -> "Note" },
            color = c.textSecondary,
            fontSize = 11.sp,
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 3.dp),
        )
        to?.let { Text("To: $it", color = c.textSecondary, fontSize = 13.sp) }
        subject?.let { Text("Subject: $it", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
        if (versions.size > 1) {
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                versions.forEachIndexed { i, (label, _) -> CapsuleButton(label, primary = i == picked, label = "Version: $label") { picked = i } }
            }
        }
        Text(body, color = c.textPrimary, fontSize = 15.sp, lineHeight = 22.sp)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            CapsuleButton(if (copied) "Copied" else "Copy", icon = DesignR.drawable.ph_copy, primary = variant != "email" && variant != "text_message") {
                copyToClipboard(context, if (subject != null) "$subject\n\n$body" else body)
                copied = true
            }
            if (variant == "email") {
                CapsuleButton("Open in Mail", primary = true, label = "Opens Mail with this draft. You send it from there.") {
                    val addr = to?.takeIf { it.contains('@') }.orEmpty()
                    openUrl(context, "mailto:$addr?subject=${Uri.encode(subject.orEmpty())}&body=${Uri.encode(body)}")
                }
            }
            if (variant == "text_message") {
                CapsuleButton("Open in Messages", primary = true, label = "Opens Messages with this draft. You send it from there.") {
                    val number = to?.takeIf { t -> t.isNotEmpty() && t.all { it.isDigit() || it in "+-() " } }.orEmpty()
                    runCatching {
                        context.startActivity(Intent(Intent.ACTION_SENDTO, Uri.parse("smsto:$number")).putExtra("sms_body", body).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                    }
                }
            }
        }
    }
}

/** Splits ```openui fences out of a reply (iOS AnswerTextSplitter). */
internal sealed interface AnswerPiece {
    data class Text(val text: String) : AnswerPiece
    data class Card(val card: JsonObject) : AnswerPiece
    data object Writing : AnswerPiece
}

internal fun splitAnswer(text: String, streaming: Boolean): List<AnswerPiece> {
    if (!text.contains("```openui")) return listOf(AnswerPiece.Text(text))
    val out = mutableListOf<AnswerPiece>()
    var rest = text
    while (true) {
        val start = rest.indexOf("```openui")
        if (start < 0) {
            if (rest.isNotBlank()) out += AnswerPiece.Text(rest)
            break
        }
        if (start > 0 && rest.substring(0, start).isNotBlank()) out += AnswerPiece.Text(rest.substring(0, start))
        val bodyStart = rest.indexOf('\n', start).let { if (it < 0) rest.length else it + 1 }
        val end = rest.indexOf("```", bodyStart)
        if (end < 0) {
            if (streaming) out += AnswerPiece.Writing else out += AnswerPiece.Text(rest.substring(start))
            break
        }
        val json = rest.substring(bodyStart, end)
        val card = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(json) as? JsonObject }.getOrNull()
        out += if (card != null && card.str("template") != null) AnswerPiece.Card(card) else AnswerPiece.Text(rest.substring(start, end + 3))
        rest = rest.substring(end + 3)
    }
    return out
}

@Composable
internal fun Spacer8() = Spacer(Modifier.height(8.dp))

internal val CardMaxWidth: Dp = 520.dp


/** Set by the shell: registers this phone for push once the notifications permission is granted (AllternitPush.registerIfAuthorized). */
object ChatHooks { var onNotificationsGranted: (() -> Unit)? = null }

/**
 * The empty-chat notifications card (iOS: empty-chat card, then PermissionPrimingSheet, then the system dialog). Shown while
 * notifications are off and the person hasn't said "Not now"; Turn on notifications runs the priming trigger.
 */
@androidx.compose.runtime.Composable
fun NotificationsCard() {
    val context = androidx.compose.ui.platform.LocalContext.current
    val c = com.allternit.core.design.AllternitTheme.colors
    val prefs = remember { context.getSharedPreferences("allternit_shell", android.content.Context.MODE_PRIVATE) }
    var granted by remember { mutableStateOf(com.allternit.feature.chat.voice.AppPermission.Notifications.isGranted(context)) }
    var dismissed by remember { mutableStateOf(prefs.getBoolean("notificationsCardDismissed", false)) }
    val ask = com.allternit.feature.chat.voice.rememberPrimedPermission(com.allternit.feature.chat.voice.AppPermission.Notifications) { ok ->
        granted = ok
        if (ok) ChatHooks.onNotificationsGranted?.invoke()
    }
    if (granted || dismissed) return
    androidx.compose.foundation.layout.Column(
        androidx.compose.ui.Modifier.padding(top = 20.dp, start = 24.dp, end = 24.dp).fillMaxWidth()
            .clip(androidx.compose.foundation.shape.RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(16.dp)
            .testTag("chatNotificationsCard"),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(8.dp),
    ) {
        Text("Let your bots reach you?", color = c.textPrimary, fontSize = 15.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
        Text("Know when a task finishes or a bot needs your approval. No marketing messages.", color = c.textSecondary, fontSize = 13.sp)
        androidx.compose.foundation.layout.Row(horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(16.dp)) {
            Text("Turn on notifications", color = c.accentPrimary, fontSize = 14.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium,
                modifier = androidx.compose.ui.Modifier.heightIn(min = 44.dp).clickable { ask() }.padding(vertical = 12.dp).testTag("chatNotificationsTurnOn"))
            Text("Not now", color = c.textSecondary, fontSize = 14.sp,
                modifier = androidx.compose.ui.Modifier.heightIn(min = 44.dp).clickable { dismissed = true; prefs.edit().putBoolean("notificationsCardDismissed", true).apply() }.padding(vertical = 12.dp).testTag("chatNotificationsNotNow"))
        }
    }
}
