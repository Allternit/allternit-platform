package com.allternit.feature.shell

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.ChatSendRequest
import com.allternit.feature.chat.ChatViewModel
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put

// MARK: - Continue a CLI session (iOS NativeSessionPickerSheet)

/** `GET native-sessions/harnesses` row. */
internal data class NativeHarness(val id: String, val label: String, val present: Boolean)

/** `GET native-sessions` row. [updatedAt] is epoch milliseconds. */
internal data class NativeCatalogSession(
    val harness: String,
    val sessionId: String,
    val path: String,
    val cwd: String?,
    val title: String?,
    val updatedAt: Double,
) {
    val id: String get() = "$harness:$sessionId"
}

private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

/**
 * The CLI-session catalog (allternit-ai lib/agents/native-sessions-api.ts). Plain `/api/v1`
 * paths: cloud-api relays them to the default computer, as on iOS.
 */
internal object NativeSessionsClient {
    private val config = ApiConfig()

    private fun url(vararg segments: String, query: Pair<String, String>? = null) =
        config.apiBaseUrl.newBuilder().apply {
            segments.forEach { addPathSegment(it) }
            query?.let { addQueryParameter(it.first, it.second) }
        }.build()

    private suspend fun get(url: okhttp3.HttpUrl): ApiResult<JsonObject> =
        ApiCall.execute(ShellApi.okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())

    suspend fun harnesses(): ApiResult<List<NativeHarness>> = when (val r = get(url("native-sessions", "harnesses"))) {
        is ApiResult.Ok -> ApiResult.Ok(
            (r.value["harnesses"] as? JsonArray).orEmpty().mapNotNull { el ->
                val o = el as? JsonObject ?: return@mapNotNull null
                val id = o.str("id") ?: return@mapNotNull null
                NativeHarness(id, o.str("label") ?: id, (o["present"] as? JsonPrimitive)?.booleanOrNull ?: false)
            },
        )
        is ApiResult.HttpError -> r
        is ApiResult.Network -> r
        is ApiResult.Decode -> r
    }

    suspend fun sessions(harness: String?): ApiResult<List<NativeCatalogSession>> =
        when (val r = get(url("native-sessions", query = harness?.let { "harness" to it }))) {
            is ApiResult.Ok -> ApiResult.Ok(
                (r.value["sessions"] as? JsonArray).orEmpty().mapNotNull { el ->
                    val o = el as? JsonObject ?: return@mapNotNull null
                    NativeCatalogSession(
                        harness = o.str("harness") ?: return@mapNotNull null,
                        sessionId = o.str("sessionId") ?: return@mapNotNull null,
                        path = o.str("path").orEmpty(),
                        cwd = o.str("cwd"),
                        title = o.str("title"),
                        updatedAt = (o["updatedAt"] as? JsonPrimitive)?.doubleOrNull ?: 0.0,
                    )
                }.sortedByDescending { it.updatedAt },
            )
            is ApiResult.HttpError -> r
            is ApiResult.Network -> r
            is ApiResult.Decode -> r
        }

    /** `POST native-sessions/pickup`: snapshots the CLI session into Allternit; returns the new session id. */
    suspend fun pickup(item: NativeCatalogSession, surface: String): ApiResult<String> {
        val body = buildJsonObject {
            put("harness", item.harness)
            put("sessionId", item.sessionId)
            put("surface", surface)
            item.cwd?.let { put("cwd", it) }
        }.toString()
        val r = ApiCall.execute(ShellApi.okHttp, ApiJson.default, ApiCall.request("POST", url("native-sessions", "pickup"), body), JsonObject.serializer())
        return when (r) {
            is ApiResult.Ok -> (r.value["session"] as? JsonObject)?.str("id")?.let { ApiResult.Ok(it) }
                ?: ApiResult.Decode(200, kotlinx.serialization.SerializationException("session.id missing"))
            is ApiResult.HttpError -> r
            is ApiResult.Network -> r
            is ApiResult.Decode -> r
        }
    }

    /** The web's errors for a backend without the catalog ("update Desktop"). */
    fun friendly(r: ApiResult<*>): String = when (r) {
        is ApiResult.HttpError -> if (r.status == 404) UPDATE else statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "")
        is ApiResult.Decode -> UPDATE
        is ApiResult.Network -> "Could not connect to the server. Check your connection and try again."
        is ApiResult.Ok -> ""
    }

    private const val UPDATE = "Your computer's Allternit doesn't have CLI sessions yet. Update Allternit Desktop, then try again."
}

/**
 * "Continue CLI session": a read-only catalog of Claude Code, Codex, Grok… sessions on the
 * user's computer. Picking one snapshots it into Allternit (the original file isn't changed)
 * and opens it. [surface] is "chat" or "code"; [signedIn] false shows the sign-in state.
 */
@Composable
internal fun NativeSessionPickerSheet(
    surface: String,
    signedIn: Boolean,
    onPicked: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var harnesses by remember { mutableStateOf<List<NativeHarness>>(emptyList()) }
    var sessions by remember { mutableStateOf<List<NativeCatalogSession>>(emptyList()) }
    var harness by remember { mutableStateOf("all") }
    var query by remember { mutableStateOf("") }
    var isLoading by remember { mutableStateOf(true) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var pickError by remember { mutableStateOf<String?>(null) }
    var picking by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableStateOf(0) }

    LaunchedEffect(harness, reload, signedIn) {
        if (!signedIn) return@LaunchedEffect
        isLoading = true
        loadError = null
        coroutineScope {
            val h = async { NativeSessionsClient.harnesses() }
            val s = async { NativeSessionsClient.sessions(if (harness == "all") null else harness) }
            val hr = h.await()
            val sr = s.await()
            if (hr is ApiResult.Ok && sr is ApiResult.Ok) {
                harnesses = hr.value
                sessions = sr.value
            } else {
                loadError = NativeSessionsClient.friendly(if (hr !is ApiResult.Ok) hr else sr)
            }
        }
        isLoading = false
    }

    val needle = query.trim().lowercase()
    val filtered = (if (needle.isEmpty()) sessions else sessions.filter { item ->
        listOf(item.harness, item.sessionId, item.title.orEmpty(), item.cwd.orEmpty(), item.path).joinToString(" ").lowercase().contains(needle)
    }).take(200)

    fun pick(item: NativeCatalogSession) {
        picking = item.id
        pickError = null
        scope.launch {
            when (val r = NativeSessionsClient.pickup(item, surface)) {
                is ApiResult.Ok -> {
                    Haptics.perform(view, Haptics.Kind.Success)
                    onDismiss()
                    onPicked(r.value)
                }
                else -> pickError = "Couldn't continue that session: ${NativeSessionsClient.friendly(r)}"
            }
            picking = null
        }
    }

    ShellSheet(
        title = "Continue a CLI session",
        onDismiss = onDismiss,
        testTag = "nativeSessionPicker",
        leading = SheetAction("Cancel", onDismiss, testTag = "nativeSessionCancel"),
    ) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.92f)) {
            if (!signedIn) {
                FriendlyState(
                    style = FriendlyStyle.Empty,
                    title = "Sign in to continue a CLI session",
                    message = "CLI sessions are read from the computer paired to your account.",
                    modifier = Modifier.padding(20.dp),
                ) { Icon(painterResource(DesignR.drawable.ph_identification_card), contentDescription = null, tint = c.textSecondary) }
                return@Column
            }
            SearchField(query, "Search id, title, folder…", "nativeSessionSearch") { query = it }
            Text(
                "Sessions from Claude Code, Codex, Grok and other CLIs on your computer. Picking one copies it into Allternit; the original isn't changed.",
                color = c.textSecondary,
                fontSize = 13.sp,
                modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp),
            )
            val present = harnesses.filter { it.present }
            if (present.isNotEmpty()) {
                Row(
                    Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 4.dp),
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    FilterChip("All CLIs", harness == "all", "nativeHarnessall") { harness = "all" }
                    present.forEach { h -> FilterChip(h.label, harness == h.id, "nativeHarness${h.id}") { harness = h.id } }
                }
            }
            pickError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp)) }
            when {
                isLoading && sessions.isEmpty() -> Row(Modifier.padding(20.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Looking on your computer…", color = c.textSecondary, fontSize = 15.sp)
                }
                loadError != null && sessions.isEmpty() -> FriendlyState(
                    style = FriendlyStyle.Offline,
                    title = "Couldn't read CLI sessions",
                    message = loadError.orEmpty(),
                    actionTitle = "Retry",
                    onAction = { reload++ },
                    modifier = Modifier.padding(20.dp),
                ) { Icon(painterResource(DesignR.drawable.ph_desktop), contentDescription = null, tint = c.textSecondary) }
                filtered.isEmpty() -> FriendlyState(
                    style = FriendlyStyle.Empty,
                    title = if (query.isEmpty()) "No CLI sessions found on your computer." else "Nothing matches.",
                    message = "",
                    art = EmptyArtKind.Search,
                    panel = true,
                    modifier = Modifier.padding(20.dp),
                ) {}
                else -> LazyColumn(Modifier.fillMaxSize()) {
                    items(filtered, key = { it.id }) { item ->
                        NativeSessionRow(item, picking = picking == item.id, enabled = picking == null) { pick(item) }
                    }
                }
            }
        }
    }
}

@Composable
private fun NativeSessionRow(item: NativeCatalogSession, picking: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val whenText = if (item.updatedAt > 0) {
        DateUtils.getRelativeTimeSpanString(item.updatedAt.toLong(), System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS, DateUtils.FORMAT_ABBREV_RELATIVE).toString()
    } else ""
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("nativeSession${item.id}")
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick)
            .heightIn(min = 56.dp)
            .padding(horizontal = 20.dp, vertical = 10.dp),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(
            item.harness.uppercase(),
            color = c.textSecondary,
            fontSize = 10.sp,
            fontWeight = FontWeight.Bold,
            modifier = Modifier.clip(RoundedCornerShape(6.dp)).background(c.bgSecondary).padding(horizontal = 6.dp, vertical = 3.dp),
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(item.title?.takeIf { it.isNotEmpty() } ?: item.sessionId, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOf(item.cwd ?: item.path, whenText).filter { it.isNotEmpty() }.joinToString(" · "), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (picking) {
            CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
        } else {
            Icon(painterResource(DesignR.drawable.ph_arrow_right), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        }
    }
}

@Composable
internal fun FilterChip(title: String, on: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        title,
        color = if (on) c.onAccent else c.textPrimary,
        fontSize = 13.sp,
        fontWeight = FontWeight.Medium,
        modifier = Modifier
            .clip(CircleShape)
            .background(if (on) c.accentPrimary else c.fillNeutral)
            .testTag(testTag)
            .clickable(role = Role.RadioButton, onClick = onClick)
            .semantics { selected = on }
            .padding(horizontal = 12.dp, vertical = 6.dp),
    )
}

/** The capsule search field (iOS ShellSearchView top inset). */
@Composable
internal fun SearchField(query: String, placeholder: String, testTag: String, focus: FocusRequester? = null, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .padding(horizontal = 16.dp, vertical = 8.dp)
            .fillMaxWidth()
            .height(40.dp)
            .clip(CircleShape)
            .background(c.bgPanel)
            .border(1.dp, c.borderSubtle, CircleShape)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
        Box(Modifier.weight(1f)) {
            if (query.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 16.sp, maxLines = 1)
            BasicTextField(
                value = query,
                onValueChange = onChange,
                singleLine = true,
                textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, fontFamily = ShellSans),
                cursorBrush = SolidColor(c.textPrimary),
                modifier = Modifier.fillMaxWidth().testTag(testTag).then(if (focus != null) Modifier.focusRequester(focus) else Modifier),
            )
        }
        if (query.isNotEmpty()) {
            Box(
                Modifier.size(28.dp).clip(CircleShape).clickable(role = Role.Button, onClickLabel = "Clear search") { onChange("") },
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(DesignR.drawable.ph_x), contentDescription = "Clear search", tint = c.textSecondary, modifier = Modifier.size(14.dp))
            }
        }
    }
}

// MARK: - Quick task (iOS QuickTaskSheet + QuickTaskRunner)

/**
 * Owns quick-task runs so a stream keeps going after its sheet closes (iOS QuickTaskRunner).
 * Each run is its own [ChatViewModel]: it creates a chat session (`POST agent-sessions`) and
 * streams the prompt through the relay exactly like the main chat.
 */
internal class QuickTaskRunner(private val backend: ChatBackend, private val contentResolver: android.content.ContentResolver) {
    enum class Status { Starting, Running, Finished, Failed }

    class Run(val id: Long, val text: String, val chat: ChatViewModel) {
        val sessionId: String? get() = chat.currentSessionId
        val status: Status
            get() {
                val failed = chat.computeGate != null || chat.transientError != null || chat.needsBrainPick ||
                    chat.messages.lastOrNull { it.role == "assistant" }?.error != null
                return when {
                    failed && !chat.isStreaming -> Status.Failed
                    chat.isCreatingSession || chat.currentSessionId == null -> if (failed) Status.Failed else Status.Starting
                    chat.isStreaming -> Status.Running
                    else -> Status.Finished
                }
            }
        val failure: String
            get() = chat.messages.lastOrNull { it.role == "assistant" }?.error?.message
                ?: chat.transientError
                ?: if (chat.computeGate != null) NoComputerCopy.DETAIL else if (chat.needsBrainPick) "Pick a brain for your chats, then try again." else "Something went wrong. Try again."
    }

    val runs = mutableStateListOf<Run>()
    var onSessionsChanged: () -> Unit = {}

    /** Starts a run with the composer's model and effort; returns its id. */
    fun start(text: String, runtimeModelId: String?, effort: String?): Long {
        val trimmed = text.trim()
        val chat = ChatViewModel(backend, contentResolver)
        chat.onSessionsChanged = { onSessionsChanged() }
        val run = Run(System.nanoTime(), trimmed, chat)
        runs.add(run)
        // Keep the list bounded: finished runs only matter to their sheet.
        while (runs.size > 10) runs.removeAt(0).chat.dispose()
        chat.send(ChatSendRequest(text = trimmed, runtimeModelId = runtimeModelId, effort = effort))
        return run.id
    }

    fun dispose() {
        runs.forEach { it.chat.dispose() }
        runs.clear()
    }
}

/**
 * Quick task (allternit-ai src/shell/QuickTaskStack.tsx): a small composer that starts a chat
 * without leaving the current screen. Start creates the chat and streams the prompt; the sheet
 * then shows Starting / Working / Done and offers Open chat.
 */
@Composable
internal fun QuickTaskSheet(
    runner: QuickTaskRunner,
    models: ModelStore,
    onOpenChat: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    var text by remember { mutableStateOf("") }
    var runId by remember { mutableStateOf<Long?>(null) }
    val run = runId?.let { id -> runner.runs.firstOrNull { it.id == id } }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }

    fun start(t: String) {
        Haptics.perform(view, Haptics.Kind.Tap)
        runId = runner.start(t, models.selectedId, models.selected?.let { models.level(it) })
    }

    ShellSheet(
        title = "Quick task",
        onDismiss = onDismiss,
        testTag = "quickTaskSheet",
        leading = SheetAction(if (run == null) "Cancel" else "Close", onDismiss, testTag = "quickTaskClose"),
        trailing = if (run == null) SheetAction("Start", { start(text) }, enabled = text.isNotBlank(), bold = true, testTag = "quickTaskStart") else null,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp).heightIn(min = 260.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (run == null) {
                Box(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 96.dp)
                        .clip(RoundedCornerShape(14.dp))
                        .background(c.bgPanel)
                        .border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp))
                        .padding(14.dp),
                ) {
                    if (text.isEmpty()) Text("Describe a task…", color = c.textTertiary, fontSize = 16.sp)
                    BasicTextField(
                        value = text,
                        onValueChange = { text = it },
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, fontFamily = ShellSans),
                        cursorBrush = SolidColor(c.textPrimary),
                        maxLines = 8,
                        modifier = Modifier.fillMaxWidth().focusRequester(focus).testTag("quickTaskInput"),
                    )
                }
                Text("Starts a new chat in the background. You can keep doing what you were doing.", color = c.textSecondary, fontSize = 13.sp)
            } else {
                QuickTaskStatus(run, onOpen = { id ->
                    onDismiss()
                    onOpenChat(id)
                }, onRetry = { start(run.text) })
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun QuickTaskStatus(run: QuickTaskRunner.Run, onOpen: (String) -> Unit, onRetry: () -> Unit) {
    val c = AllternitTheme.colors
    val status = run.status
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(c.bgPanel)
            .padding(14.dp)
            .testTag("quickTaskStatus"),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(run.text, color = c.textPrimary, fontSize = 15.sp, maxLines = 4, overflow = TextOverflow.Ellipsis)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            when (status) {
                QuickTaskRunner.Status.Starting, QuickTaskRunner.Status.Running -> {
                    CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text(if (status == QuickTaskRunner.Status.Starting) "Starting…" else "Working", color = c.textSecondary, fontSize = 13.sp)
                }
                QuickTaskRunner.Status.Finished -> {
                    Icon(painterResource(DesignR.drawable.ph_check_circle_fill), contentDescription = null, tint = AllternitStatic.StatusSuccess, modifier = Modifier.size(16.dp))
                    Text("Done", color = c.textSecondary, fontSize = 13.sp)
                }
                QuickTaskRunner.Status.Failed -> {
                    Icon(painterResource(DesignR.drawable.ph_warning_circle_fill), contentDescription = null, tint = AllternitStatic.StatusError, modifier = Modifier.size(16.dp))
                    Text(FriendlyErrorMessage.from(run.failure), color = c.textSecondary, fontSize = 13.sp, maxLines = 3)
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
            run.sessionId?.let { id ->
                Box(
                    Modifier
                        .clip(CircleShape)
                        .background(c.textPrimary)
                        .testTag("quickTaskOpen")
                        .clickable(role = Role.Button) { onOpen(id) }
                        .height(36.dp)
                        .padding(horizontal = 16.dp),
                    contentAlignment = Alignment.Center,
                ) { Text("Open chat", color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
            }
            if (status == QuickTaskRunner.Status.Failed) {
                Text(
                    "Try again",
                    color = c.textPrimary,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(RoundedCornerShape(8.dp)).clickable(role = Role.Button, onClick = onRetry).padding(8.dp),
                )
            }
        }
    }
}
