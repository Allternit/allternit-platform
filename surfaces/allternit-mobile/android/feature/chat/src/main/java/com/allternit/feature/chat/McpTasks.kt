@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package com.allternit.feature.chat

import android.content.Intent
import android.net.Uri
import android.provider.Settings
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.text.KeyboardOptions
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.ThinkingOrb
import com.allternit.core.design.ThinkingOrbSize
import com.allternit.core.design.ThinkingOrbState
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put

/*
 * Long-running MCP tasks and server questions (elicitation), shown inline in chat. Mirrors iOS
 * Features/McpTasks (PR #2959) and the web McpTaskCard / McpElicitationForm, behind the
 * `mcp.tasks` flag (on by default; FeatureFlags.MCP_TASKS):
 *   bus events `mcp.task` / `mcp.elicitation` / `mcp.elicitation.resolved`;
 *   POST /api/v1/mcp/elicitation/:id/reply {action, content?}
 *   POST /api/v1/mcp/task/:id/cancel   (ids contain ':', so they are URL-encoded).
 */

// MARK: - Models

data class McpTaskPart(
    val taskId: String,
    val status: String,
    val toolName: String?,
    /** The server the task runs on. */
    val source: String?,
    val title: String?,
    val statusMessage: String?,
    val progressDone: Double?,
    val progressTotal: Double?,
    val progressLabel: String?,
    val toolCallId: String?,
    /** Epoch milliseconds. */
    val startedAt: Double?,
    val updatedAt: Double?,
    val errorText: String?,
) {
    val isInputNeeded get() = status == "input_required"
    val isRunning get() = status == "working" || status == "submitted" || status == "running"
    val isDone get() = status == "completed"
    val isFailed get() = status == "failed"
    val isCancelled get() = status == "cancelled" || status == "canceled"
    val isFinished get() = isDone || isFailed || isCancelled

    /** 0..1 when a total is known. */
    val fraction: Float?
        get() {
            val d = progressDone ?: return null
            val t = progressTotal ?: return null
            return if (t > 0) (d / t).coerceIn(0.0, 1.0).toFloat() else null
        }

    val displayTitle: String get() = title?.takeIf { it.isNotEmpty() } ?: toolName?.takeIf { it.isNotEmpty() } ?: "Task"

    companion object {
        fun from(o: JsonObject): McpTaskPart? {
            val id = o.str("taskId") ?: return null
            val progress = o.obj("progress")
            val error = o["error"]
            val errorText = when (error) {
                is JsonPrimitive -> error.takeIf { it !is JsonNull }?.contentOrNull
                is JsonObject -> error.str("message")
                else -> null
            }
            return McpTaskPart(
                taskId = id,
                status = o.str("status") ?: "working",
                toolName = o.str("toolName"),
                source = o.str("source"),
                title = o.str("title"),
                statusMessage = o.str("statusMessage"),
                progressDone = progress?.num("done"),
                progressTotal = progress?.num("total"),
                progressLabel = progress?.str("label"),
                toolCallId = o.str("toolCallID") ?: o.str("toolCallId"),
                startedAt = o.num("startedAt"),
                updatedAt = o.num("updatedAt"),
                errorText = errorText,
            )
        }
    }
}

enum class McpFieldKind { Text, Number, Integer, Boolean, Choice }

data class McpFormOption(val value: String, val label: String)

data class McpFormField(
    val key: String,
    val title: String,
    val detail: String?,
    val kind: McpFieldKind,
    val options: List<McpFormOption>,
    val required: Boolean,
    val defaultText: String?,
    val multiline: Boolean,
    /** Form mode never collects secrets: the server must use URL mode or a connector. */
    val isSecret: Boolean,
) {
    companion object {
        private val secretWords = listOf("password", "passcode", "secret", "token", "api key", "apikey", "private key", "credential", "ssn", "card number", "cvv")

        /** Password-like names and `format: password` (iOS looksSecret). */
        fun looksSecret(text: String, format: String?): Boolean {
            if (format?.lowercase() == "password") return true
            val lowered = text.lowercase().replace("_", " ").replace("-", " ")
            return secretWords.any { lowered.contains(it) }
        }
    }
}

data class McpElicitationPart(
    /** "<taskId>:<key>". */
    val id: String,
    val isUrlMode: Boolean,
    val source: String?,
    val message: String,
    val taskId: String?,
    val toolCallId: String?,
    val url: String?,
    val fields: List<McpFormField>,
) {
    val hasSecretField get() = fields.any { it.isSecret }

    companion object {
        fun from(o: JsonObject): McpElicitationPart? {
            val id = o.str("id") ?: return null
            val url = o.str("url")?.takeIf { it.startsWith("https://", true) || it.startsWith("http://", true) }
            val schema = o.obj("requestedSchema")
            val required = (schema?.get("required") as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.toSet()
            val built = (schema?.obj("properties")).orEmpty().mapNotNull { (key, el) ->
                val prop = el as? JsonObject ?: return@mapNotNull null
                val title = prop.str("title") ?: key
                val options: List<McpFormOption> = when {
                    prop["oneOf"] is JsonArray -> prop.arr("oneOf").mapNotNull { one ->
                        val ob = one as? JsonObject ?: return@mapNotNull null
                        ob.str("const")?.let { McpFormOption(it, ob.str("title") ?: it) }
                    }
                    prop["enum"] is JsonArray -> {
                        val names = prop.arr("enumNames")
                        prop.arr("enum").mapIndexedNotNull { i, v ->
                            val value = (v as? JsonPrimitive)?.contentOrNull ?: return@mapIndexedNotNull null
                            McpFormOption(value, (names.getOrNull(i) as? JsonPrimitive)?.contentOrNull ?: value)
                        }
                    }
                    else -> emptyList()
                }
                val kind = when {
                    options.isNotEmpty() -> McpFieldKind.Choice
                    prop.str("type") == "boolean" -> McpFieldKind.Boolean
                    prop.str("type") == "integer" -> McpFieldKind.Integer
                    prop.str("type") == "number" -> McpFieldKind.Number
                    else -> McpFieldKind.Text
                }
                McpFormField(
                    key = key,
                    title = title,
                    detail = prop.str("description"),
                    kind = kind,
                    options = options,
                    required = key in required,
                    defaultText = (prop["default"] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull,
                    multiline = ((prop["maxLength"] as? JsonPrimitive)?.doubleOrNull ?: 0.0) > 200,
                    isSecret = McpFormField.looksSecret("$key $title", prop.str("format")),
                )
            }
            // Required first, then by name (JSON objects carry no order).
            val fields = built.sortedWith(compareBy({ if (it.required) 0 else 1 }, { it.key }))
            return McpElicitationPart(
                id = id,
                isUrlMode = o.str("mode") == "url",
                source = o.str("source"),
                message = o.str("message").orEmpty(),
                taskId = o.str("taskId"),
                toolCallId = o.str("toolCallID") ?: o.str("toolCallId"),
                url = url,
                fields = fields,
            )
        }
    }
}

private fun JsonObject.num(key: String): Double? = (this[key] as? JsonPrimitive)?.takeIf { it !is JsonNull }?.doubleOrNull

// MARK: - Store

/**
 * Latest state of every task and open question, by id. Chat messages keep the ids; the cards
 * read here so a status update redraws the same card (iOS McpTasksStore).
 */
object McpTasksStore {
    val tasks = mutableStateMapOf<String, McpTaskPart>()
    val elicitations = mutableStateMapOf<String, McpElicitationPart>()
    val resolved = mutableStateMapOf<String, String>()

    fun apply(part: McpTaskPart) { tasks[part.taskId] = part }
    fun apply(part: McpElicitationPart) { elicitations[part.id] = part }
    fun markResolved(id: String, action: String) { resolved[id] = action }
    fun isResolved(id: String) = resolved.containsKey(id)

}

// MARK: - Client

/** `POST mcp/elicitation/:id/reply {action, content?}`; action: accept | decline | cancel, content only with accept. */
internal suspend fun ChatBackend.replyElicitation(id: String, action: String, content: JsonObject?): ApiResult<Unit> {
    val body = buildJsonObject {
        put("action", action)
        if (content != null) put("content", content)
    }.toString()
    return ApiCall.executeUnit(authedHttp, mcpJson, ApiCall.request("POST", mcpUrl("elicitation", id, "reply"), body))
}

/** `POST mcp/task/:id/cancel`. */
internal suspend fun ChatBackend.cancelMcpTask(id: String): ApiResult<Unit> =
    ApiCall.executeUnit(authedHttp, mcpJson, ApiCall.request("POST", mcpUrl("task", id, "cancel"), "{}"))

/** Every character outside the unreserved set is percent-encoded (ids carry ':'). */
internal fun escapeMcpId(id: String): String = Uri.encode(id, "-._~")

// MARK: - Section drawn under an assistant reply

/** A reply's MCP tasks (orb-and-beam cards) and any server question with no task card to sit in. */
@Composable
internal fun McpTasksSection(taskIds: List<String>, formIds: List<String>, backend: ChatBackend) {
    taskIds.forEach { McpTaskCard(it, backend) }
    formIds.mapNotNull { McpTasksStore.elicitations[it] }
        .filter { form -> form.taskId == null || McpTasksStore.tasks[form.taskId] == null }
        .forEach { McpElicitationForm(it, backend) }
}

// MARK: - Task card

/** Task card, "orb and beam": the thinking orb, a status line and a beam. Stop cancels on the server. */
@Composable
internal fun McpTaskCard(taskId: String, backend: ChatBackend) {
    val task = McpTasksStore.tasks[taskId] ?: return
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    var stopping by remember(taskId) { mutableStateOf(false) }
    var stopError by remember(taskId) { mutableStateOf<String?>(null) }
    val reduced = rememberReduceMotion()
    // iOS: success haptic when a task finishes done, warning when it fails or is stopped. Only for a task seen running.
    var wasFinished by remember(taskId) { mutableStateOf(task.isFinished) }
    LaunchedEffect(task.isFinished) {
        if (task.isFinished && !wasFinished) Haptics.perform(view, if (task.isDone) Haptics.Kind.Success else Haptics.Kind.Warning)
        wasFinished = task.isFinished
    }

    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(c.bgSecondary)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            when {
                task.isDone -> Icon(painterResource(com.allternit.core.design.R.drawable.ph_check_circle_fill), null, tint = c.textSecondary, modifier = Modifier.size(24.dp))
                task.isFailed -> Icon(painterResource(com.allternit.core.design.R.drawable.ph_warning_circle_fill), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(24.dp))
                task.isCancelled -> Icon(painterResource(com.allternit.core.design.R.drawable.ph_stop_circle), null, tint = c.textSecondary, modifier = Modifier.size(24.dp))
                else -> ThinkingOrb(
                    state = if (task.isInputNeeded) ThinkingOrbState.Listening else ThinkingOrbState.Working,
                    size = ThinkingOrbSize.Inline,
                    paused = task.isInputNeeded,
                    modifier = Modifier.size(24.dp),
                )
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(task.displayTitle, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                McpStatusLine(task)
            }
            if (!task.isFinished) {
                Box {
                    CapsuleButton(if (stopping) "Stopping" else "Stop", label = "Stop ${task.displayTitle}", enabled = !stopping) {
                        Haptics.perform(view, Haptics.Kind.Tap)
                        stopping = true
                        stopError = null
                        scope.launch {
                            if (backend.cancelMcpTask(task.taskId) !is ApiResult.Ok) {
                                stopError = "Could not stop this task. Try again."
                                Haptics.perform(view, Haptics.Kind.Warning)
                            }
                            stopping = false
                        }
                    }
                }
            }
        }
        if (task.isRunning || task.isInputNeeded) {
            McpBeam(task.fraction, paused = task.isInputNeeded, reduced = reduced)
        }
        task.progressLabel?.takeIf { task.isRunning && it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
        task.source?.takeIf { it.isNotEmpty() }?.let { Text("Runs on $it", color = c.textSecondary, fontSize = 12.sp) }
        if (task.isFailed) task.errorText?.takeIf { it.isNotEmpty() }?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
        stopError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
        // The question this task is waiting on, in place.
        McpTasksStore.elicitations.values
            .filter { it.taskId == task.taskId && !McpTasksStore.isResolved(it.id) }
            .sortedBy { it.id }
            .forEach { McpElicitationForm(it, backend) }
    }
}

@Composable
private fun rememberReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

@Composable
private fun McpStatusLine(task: McpTaskPart) {
    val c = AllternitTheme.colors
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    LaunchedEffect(task.taskId, task.isFinished) {
        while (!task.isFinished) {
            now = System.currentTimeMillis()
            delay(1_000)
        }
    }
    val status = when {
        task.isInputNeeded -> "Needs your input"
        task.isDone -> "Done"
        task.isFailed -> "Failed"
        task.isCancelled -> "Stopped"
        else -> task.statusMessage?.takeIf { it.isNotEmpty() } ?: "Running"
    }
    val elapsed = task.startedAt?.let { start ->
        val end = if (task.isFinished) (task.updatedAt ?: now.toDouble()) else now.toDouble()
        val seconds = ((end - start) / 1000).toInt().coerceAtLeast(0)
        if (seconds >= 3600) "%d:%02d:%02d".format(seconds / 3600, (seconds % 3600) / 60, seconds % 60)
        else "%d:%02d".format(seconds / 60, seconds % 60)
    }
    Text(
        listOfNotNull(status, elapsed).joinToString(" · "),
        color = if (task.isInputNeeded) c.textPrimary else c.textSecondary,
        fontSize = 12.sp,
    )
}

/** Thin beam: fills to [fraction], or a soft segment sweeps when the length is unknown. Static when reduced or paused. */
@Composable
private fun McpBeam(fraction: Float?, paused: Boolean, reduced: Boolean) {
    val c = AllternitTheme.colors
    Box(Modifier.fillMaxWidth().height(3.dp).clip(CircleShape).background(c.fillNeutral)) {
        if (fraction != null) {
            Box(Modifier.fillMaxHeight().fillMaxWidth(fraction).background(c.textPrimary.copy(alpha = 0.75f)))
        } else {
            BoxWithConstraints(Modifier.fillMaxWidth().fillMaxHeight()) {
                val segment = maxWidth * 0.3f
                val travel = maxWidth - segment
                val progress = if (reduced || paused) 0f else {
                    val t = rememberInfiniteTransition(label = "mcp-beam")
                    val v by t.animateFloat(0f, 1f, infiniteRepeatable(tween(1200, easing = LinearEasing), RepeatMode.Reverse), label = "sweep")
                    v
                }
                Box(
                    Modifier
                        .offset(x = travel * progress)
                        .width(segment)
                        .fillMaxHeight()
                        .background(c.textPrimary.copy(alpha = if (paused) 0.25f else 0.6f)),
                )
            }
        }
    }
}

// MARK: - Form

/**
 * A server's question, answered in place. Form mode shows the requested fields; URL mode opens the
 * page in the browser. Form mode never collects secrets: a secret-looking field turns the card
 * into a pointer to the server's own page and leaves only Decline.
 */
@Composable
internal fun McpElicitationForm(request: McpElicitationPart, backend: ChatBackend) {
    if (McpTasksStore.isResolved(request.id)) return
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val text = remember(request.id) {
        mutableStateMapOf<String, String>().apply {
            request.fields.forEach { f ->
                when {
                    f.kind == McpFieldKind.Boolean -> Unit
                    f.defaultText != null -> put(f.key, f.defaultText)
                    f.kind == McpFieldKind.Choice && f.required && f.options.isNotEmpty() -> put(f.key, f.options.first().value)
                }
            }
        }
    }
    val flags = remember(request.id) {
        mutableStateMapOf<String, Boolean>().apply { request.fields.filter { it.kind == McpFieldKind.Boolean }.forEach { put(it.key, it.defaultText == "true") } }
    }
    var busy by remember(request.id) { mutableStateOf<String?>(null) }
    var error by remember(request.id) { mutableStateOf<String?>(null) }

    fun send(action: String) {
        busy = action
        error = null
        Haptics.perform(view, Haptics.Kind.Tap)
        val content: JsonObject? = if (action == "accept" && !request.isUrlMode) buildJsonObject {
            request.fields.forEach { f ->
                when (f.kind) {
                    McpFieldKind.Boolean -> put(f.key, flags[f.key] ?: false)
                    McpFieldKind.Number, McpFieldKind.Integer -> text[f.key]?.toDoubleOrNull()?.let { put(f.key, it) }
                    else -> text[f.key]?.takeIf { it.isNotEmpty() }?.let { put(f.key, it) }
                }
            }
        } else null
        scope.launch {
            when (backend.replyElicitation(request.id, action, content)) {
                is ApiResult.Ok -> {
                    McpTasksStore.markResolved(request.id, action)
                    Haptics.perform(view, Haptics.Kind.Success)
                }
                else -> {
                    error = "Could not send your answer. Try again."
                    Haptics.perform(view, Haptics.Kind.Warning)
                }
            }
            busy = null
        }
    }

    val canSubmit = request.fields.all { f ->
        !f.required || f.kind == McpFieldKind.Boolean || !text[f.key].isNullOrBlank()
    }

    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(c.bgPrimary)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp))
            .padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        CardHeader(com.allternit.core.design.R.drawable.ph_question, request.source?.let { "$it is asking" } ?: "The server is asking")
        if (request.message.isNotEmpty()) Text(request.message, color = c.textPrimary, fontSize = 15.sp)
        if (request.isUrlMode) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                CapsuleButton("Decline", enabled = busy == null) { send("decline") }
                request.url?.let { url ->
                    Text(Uri.parse(url).host.orEmpty(), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    CapsuleButton("Open page", primary = true, enabled = busy == null) {
                        Haptics.perform(view, Haptics.Kind.Tap)
                        runCatching { androidx.browser.customtabs.CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url)) }
                            .onFailure { runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) } }
                    }
                    CapsuleButton("Done", enabled = busy == null) { send("accept") }
                }
            }
        } else if (request.hasSecretField) {
            Text(
                "This asks for a secret. Allternit never collects secrets in a form. Use the server's own sign-in page or a connector instead.",
                color = c.textSecondary,
                fontSize = 13.sp,
            )
            CapsuleButton("Decline", enabled = busy == null) { send("decline") }
        } else {
            request.fields.forEach { field -> McpFieldRow(field, text, flags) }
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                CapsuleButton("Decline", enabled = busy == null) { send("decline") }
                Spacer(Modifier.weight(1f))
                if (busy == "accept") CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                CapsuleButton("Send", primary = true, enabled = canSubmit && busy == null) { send("accept") }
            }
        }
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
    }
}

@Composable
private fun McpFieldRow(field: McpFormField, text: MutableMap<String, String>, flags: MutableMap<String, Boolean>) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(if (field.required) field.title else "${field.title} (optional)", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
        when (field.kind) {
            McpFieldKind.Boolean -> Row(verticalAlignment = Alignment.CenterVertically) {
                Text(field.detail ?: field.title, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                Switch(checked = flags[field.key] ?: false, onCheckedChange = { flags[field.key] = it; Haptics.perform(view, Haptics.Kind.Tap) })
            }
            McpFieldKind.Choice -> FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                if (!field.required) CapsuleButton("None", primary = text[field.key].isNullOrEmpty()) { text[field.key] = "" }
                field.options.forEach { o ->
                    CapsuleButton(o.label, primary = text[field.key] == o.value, label = o.label) { text[field.key] = o.value; Haptics.perform(view, Haptics.Kind.Tap) }
                }
            }
            else -> {
                val numeric = field.kind == McpFieldKind.Integer || field.kind == McpFieldKind.Number
                BasicTextField(
                    value = text[field.key].orEmpty(),
                    onValueChange = { text[field.key] = it },
                    singleLine = !field.multiline,
                    minLines = if (field.multiline) 3 else 1,
                    maxLines = if (field.multiline) 6 else 1,
                    textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp),
                    keyboardOptions = KeyboardOptions(keyboardType = if (numeric) KeyboardType.Decimal else KeyboardType.Text),
                    modifier = Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(8.dp))
                        .background(c.fillNeutral)
                        .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(8.dp))
                        .padding(horizontal = 10.dp, vertical = 8.dp)
                        .semantics { contentDescription = field.title },
                    decorationBox = { inner ->
                        if (text[field.key].isNullOrEmpty()) Text(field.detail ?: field.title, color = c.textSecondary, fontSize = 15.sp)
                        inner()
                    },
                )
            }
        }
        if (field.kind != McpFieldKind.Boolean) field.detail?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 11.sp) }
    }
}
