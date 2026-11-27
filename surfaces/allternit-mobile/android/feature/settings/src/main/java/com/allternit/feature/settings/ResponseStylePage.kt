package com.allternit.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.RadioButtonDefaults
import androidx.compose.material3.Text
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
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitRadii
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyErrorMessage
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.core.stream.AgentChatFrames
import com.allternit.core.stream.ChatStreamEvent
import com.allternit.core.stream.SseParser
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.chat.MarkdownText
import java.io.IOException
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put

/*
 * Settings > Response style (ResponseStylePanel.tsx, iOS ResponseStyleSettingsPage) with the "Try this style" card
 * (Feature A, step A4). The styles and custom instructions are a draft until Save. The card sends one real turn
 * through the API chat bridge with `stylePreview` set, so the answer uses what is on screen now.
 */

private data class StyleOption(val id: String, val label: String, val detail: String)

private val STYLE_OPTIONS = listOf(
    StyleOption("concise", "Concise", "The answer in one line, then up to 3 bullets."),
    StyleOption("balanced", "Balanced", "The answer in one line, then short sections."),
    StyleOption("detailed", "Detailed", "The full layout, with reasoning, examples and edge cases."),
    StyleOption("learning", "Learning", "Guides you with questions instead of giving the answer right away."),
    StyleOption("custom", "Custom", "Your instructions below set the length. The layout stays the same."),
)

private fun styleLabel(id: String) = STYLE_OPTIONS.firstOrNull { it.id == id }?.label ?: "Balanced"

/** Settings > Response style. [push] opens a page by id (the signed-out card sends people to "profile"). */
@Composable
internal fun ResponseStylePage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    SPage("Response style", tag = "responseStylePage", onBack = back) {
        SNote("Controls how agents respond across every chat. Saved to your account and synced into each agent's workspace automatically.")
        if (host.account == null) {
            ResponseStyleBody(host, "balanced", "", push)
        } else {
            val (load, reload) = rememberLoad("response-style") { loadPrefs() }
            LoadView(load, reload, "load your response style") { (style, instructions) -> ResponseStyleBody(host, style, instructions, push) }
        }
    }
}

private suspend fun loadPrefs(): Load<Pair<String, String>> = when (val r = Net.get("agent-preferences")) {
    is ApiResult.Ok -> {
        val root = r.value as? JsonObject
        val prefs = (root?.get("preferences") as? JsonObject) ?: root
        val style = (prefs?.get("response_style") as? JsonPrimitive)?.contentOrNull?.takeIf { s -> STYLE_OPTIONS.any { it.id == s } } ?: "balanced"
        val instructions = (prefs?.get("custom_instructions") as? JsonPrimitive)?.contentOrNull.orEmpty()
        Load.Ready(style to instructions)
    }
    is ApiResult.HttpError -> if (r.status == 428 || r.code == "compute_required") Load.NoComputer else Load.Failed(r.friendly("load your response style"))
    else -> Load.Failed(r.friendly("load your response style"))
}

@Composable
private fun ResponseStyleBody(host: SettingsHost, initialStyle: String, initialInstructions: String, push: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var savedStyle by remember { mutableStateOf(initialStyle) }
    var savedInstructions by remember { mutableStateOf(initialInstructions) }
    var style by remember { mutableStateOf(initialStyle) }
    var instructions by remember { mutableStateOf(initialInstructions) }
    var saving by remember { mutableStateOf(false) }
    var saveError by remember { mutableStateOf<String?>(null) }
    val signedIn = host.account != null
    val dirty = style != savedStyle || instructions.trim() != savedInstructions.trim()

    SLabel("Style")
    SCard {
        STYLE_OPTIONS.forEachIndexed { i, o ->
            if (i > 0) SDivider()
            val selected = style == o.id
            Row(
                Modifier.fillMaxWidth().heightIn(min = 64.dp).testTag("responseStyle_${o.id}")
                    .clickable(role = Role.RadioButton) { style = o.id }
                    .semantics { this.selected = selected }
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                RadioButton(selected, onClick = null, modifier = Modifier.padding(12.dp), colors = RadioButtonDefaults.colors(selectedColor = c.textPrimary, unselectedColor = c.textTertiary))
                Column(Modifier.weight(1f).padding(end = 8.dp)) {
                    Text(o.label, color = c.textPrimary, fontSize = 16.sp)
                    Text(o.detail, color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp)
                }
            }
        }
    }

    SLabel("Custom instructions")
    SField("Instructions", instructions, "responseStyleInstructions", { instructions = it }, singleLine = false, enabled = signedIn)
    SNote("Extra guidance appended to the response style. Applied to every agent chat.")

    SLabel("Try it")
    TryThisStyleCard(
        signedIn = signedIn,
        style = style,
        instructions = instructions.trim(),
        notSaved = dirty,
        onSignIn = { push("profile") },
    )

    SError(saveError)
    Box(Modifier.padding(top = 12.dp)) {
        SButton(if (saving) "Saving" else "Save", tag = "responseStyleSave", enabled = signedIn && dirty && !saving) {
            saving = true; saveError = null
            val sentStyle = style; val sentInstructions = instructions.trim()
            scope.launch {
                val r = Net.send("PUT", "agent-preferences", buildJsonObject { put("response_style", sentStyle); put("custom_instructions", sentInstructions) })
                if (r is ApiResult.Ok) { savedStyle = sentStyle; savedInstructions = sentInstructions } else saveError = r.friendly("save your response style")
                saving = false
            }
        }
    }
}

// MARK: - The Try this style card

private const val SAMPLE_QUESTION_HINT = "Ask anything to see this style"

private sealed interface PreviewState {
    data object Idle : PreviewState
    data object Streaming : PreviewState
    data object Done : PreviewState
    data class Error(val message: String) : PreviewState
    /** The runtime did not echo `stylePreview`: it would answer in the saved style, so the card goes away. */
    data object Hidden : PreviewState
}

/** What the stream reports to the card. */
private sealed interface PreviewEvent {
    data object Echo : PreviewEvent
    data object NoEcho : PreviewEvent
    data class Text(val delta: String) : PreviewEvent
    data object Done : PreviewEvent
    data class Failed(val status: Int?, val code: String?, val message: String?) : PreviewEvent
}

@Composable
private fun TryThisStyleCard(signedIn: Boolean, style: String, instructions: String, notSaved: Boolean, onSignIn: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var question by remember { mutableStateOf("") }
    var state by remember { mutableStateOf<PreviewState>(PreviewState.Idle) }
    var answer by remember { mutableStateOf("") }
    // What the shown answer was made with; a different draft marks it old (Run again, never an automatic resend).
    var ranWith by remember { mutableStateOf<Pair<String, String>?>(null) }
    var ranQuestion by remember { mutableStateOf("") }
    var job by remember { mutableStateOf<Job?>(null) }
    val answerReveal = remember { BringIntoViewRequester() }

    // The API says whether it knows `stylePreview`. Anything but supported:true (404, error) hides the card and sends nothing,
    // because an old API would run a normal tools-on turn.
    var supported by remember { mutableStateOf<Boolean?>(null) }
    var remaining by remember { mutableStateOf<Int?>(null) }
    LaunchedEffect(signedIn) {
        if (!signedIn) { supported = true; return@LaunchedEffect }
        val r = Net.get("agent-chat/style-preview")
        val o = (r as? ApiResult.Ok)?.value as? JsonObject
        val ok = (o?.get("supported") as? JsonPrimitive)?.booleanOrNull == true
        supported = ok
        remaining = (o?.get("remaining") as? JsonPrimitive)?.intOrNull
    }
    if (supported != true || state is PreviewState.Hidden) return
    val limitHit = remaining?.let { it <= 0 } == true

    fun run(q: String) {
        val text = q.trim()
        if (text.isEmpty() || !signedIn || limitHit) return
        remaining = remaining?.let { it - 1 }
        job?.cancel()
        answer = ""; state = PreviewState.Streaming
        ranWith = style to instructions; ranQuestion = text
        job = scope.launch {
            var failed: PreviewEvent.Failed? = null
            try {
                previewStream(text, style, instructions).collect { e ->
                    when (e) {
                        PreviewEvent.Echo -> Unit
                        PreviewEvent.NoEcho -> { state = PreviewState.Hidden; return@collect }
                        is PreviewEvent.Text -> answer += e.delta
                        PreviewEvent.Done -> Unit
                        is PreviewEvent.Failed -> failed = e
                    }
                }
            } catch (ce: CancellationException) {
                // Stop keeps what came in.
                if (state is PreviewState.Streaming) state = PreviewState.Done
                throw ce
            }
            val f = failed
            if (state is PreviewState.Hidden) return@launch
            state = if (f != null) PreviewState.Error(previewErrorText(f)) else PreviewState.Done
        }
    }

    val stale = state is PreviewState.Done && ranWith != null && ranWith != (style to instructions)
    val streaming = state is PreviewState.Streaming
    val canSend = signedIn && !limitHit && question.isNotBlank()

    SCard(Modifier.testTag("stylePreviewCard")) {
        Column(Modifier.padding(16.dp)) {
            Text("Try this style", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.Medium)
            OutlinedTextField(
                value = question, onValueChange = { question = it }, enabled = signedIn && !streaming,
                label = { Text(SAMPLE_QUESTION_HINT) }, singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                keyboardActions = KeyboardActions(onSend = { if (canSend) run(question) }),
                trailingIcon = {
                    // 48dp send target; the same spot becomes Stop while the answer streams.
                    IconButton(
                        onClick = {
                            Haptics.perform(view, Haptics.Kind.Tap)
                            if (streaming) job?.cancel() else run(question)
                        },
                        enabled = streaming || canSend,
                        modifier = Modifier.size(48.dp).testTag("stylePreviewSend"),
                    ) {
                        Icon(
                            painterResource(if (streaming) DesignR.drawable.ph_stop_fill else DesignR.drawable.ph_paper_plane_tilt),
                            contentDescription = if (streaming) "Stop" else "Send", modifier = Modifier.size(22.dp),
                        )
                    }
                },
                modifier = Modifier.fillMaxWidth().padding(top = 10.dp).testTag("stylePreviewField"),
            )

            if (!signedIn) {
                Text("Sign in to try a style", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(top = 14.dp))
                Box(Modifier.heightIn(min = 48.dp).clickable(role = Role.Button) { onSignIn() }.testTag("stylePreviewSignIn"), contentAlignment = Alignment.CenterStart) {
                    Box(Modifier.heightIn(min = 40.dp).clip(RoundedCornerShape(20.dp)).background(c.textPrimary).padding(horizontal = 24.dp), contentAlignment = Alignment.Center) {
                        Text("Sign in", color = c.bgPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                    }
                }
            }

            if (answer.isNotEmpty() || streaming) {
                Box(
                    Modifier.fillMaxWidth().padding(top = 14.dp).bringIntoViewRequester(answerReveal)
                        .alpha(if (stale) 0.55f else 1f)
                        .semantics { liveRegion = LiveRegionMode.Polite }.testTag("stylePreviewAnswer"),
                ) {
                    // The cursor is a still bar, so reduced motion has nothing to turn off.
                    MarkdownText(if (streaming) "$answer▍" else answer)
                }
            }

            (state as? PreviewState.Error)?.let { err ->
                Row(
                    Modifier.fillMaxWidth().padding(top = 14.dp).clip(RoundedCornerShape(12.dp)).background(Danger.copy(alpha = 0.12f)).padding(12.dp).testTag("stylePreviewError"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Icon(painterResource(DesignR.drawable.ph_warning_circle), contentDescription = null, tint = Danger, modifier = Modifier.size(20.dp))
                    Text(err.message, color = Danger, fontSize = 14.sp, lineHeight = 19.sp, modifier = Modifier.weight(1f))
                }
            }

            if (signedIn && limitHit) {
                Text("You have used all your style previews for this hour. Try again later.", color = Danger, fontSize = 14.sp, modifier = Modifier.padding(top = 10.dp).testTag("stylePreviewLimit"))
            }
            if (signedIn) {
                Text(
                    if (notSaved) "Preview of ${styleLabel(style)}. Not saved yet." else "Preview of your saved style.",
                    color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.padding(top = 10.dp).testTag("stylePreviewStatus"),
                )
            }

            if (stale || state is PreviewState.Error) {
                val retry = state is PreviewState.Error
                Row(
                    Modifier.padding(top = 10.dp).heightIn(min = 48.dp).clip(RoundedCornerShape(24.dp)).background(c.borderSubtle)
                        .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); run(ranQuestion.ifBlank { question }) }
                        .padding(horizontal = 24.dp).testTag(if (retry) "stylePreviewRetry" else "stylePreviewRunAgain"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(painterResource(DesignR.drawable.ph_arrow_clockwise), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
                    Text(if (retry) "Retry" else "Run again", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                }
            }
        }
    }
}

/** The same words the chat shows: usage limit and offline come through [FriendlyErrorMessage]. */
private fun previewErrorText(f: PreviewEvent.Failed): String = when {
    f.status == 429 -> f.message?.takeIf { it.isNotBlank() } ?: "You have used all your style previews for this hour. Try again later."
    f.status == 401 || f.status == 403 -> "Sign in again to try a style."
    f.code == "network" -> "You're offline. Check your connection and try again."
    else -> FriendlyErrorMessage.from(f.message ?: f.code ?: "The reply didn't finish")
}

// MARK: - The stream

/**
 * POST `/api/v1/agent-chat` with `stylePreview` and read the event stream. Frames go through the chat's own
 * [SseParser] and [AgentChatFrames]; the raw first frame is read here as well, because the echo
 * (`"stylePreview": true`) is not a chat event. No echo means an old runtime: [PreviewEvent.NoEcho] and the stream stops.
 * The turn uses a fresh throwaway id, never a chat the person has.
 */
private fun previewStream(question: String, style: String, instructions: String): Flow<PreviewEvent> = flow {
    val body = buildJsonObject {
        put("chatId", "style-preview-" + UUID.randomUUID())
        put("message", question)
        put("stylePreview", buildJsonObject { put("response_style", style); put("custom_instructions", instructions) })
    }.toString()
    val call = LibraryNet.okHttp.newCall(ApiCall.request("POST", LibraryNet.url("agent-chat"), body, accept = "text/event-stream"))
    currentCoroutineContext().job.invokeOnCompletion { call.cancel() }
    try {
        call.execute().use { response ->
            if (!response.isSuccessful) {
                val text = runCatching { response.body?.string().orEmpty() }.getOrDefault("")
                val err = ApiCall.httpError(response.code, text)
                emit(PreviewEvent.Failed(err.status, err.code, err.message))
                return@use
            }
            val source = response.body?.source() ?: run { emit(PreviewEvent.Failed(null, "bad_stream", null)); return@use }
            val parser = SseParser()
            val buffer = ByteArray(4096)
            var checkedEcho = false
            while (true) {
                val n = source.read(buffer)
                if (n < 0) break
                for (frame in parser.feed(buffer.copyOf(n))) {
                    if (!checkedEcho && frame.data.isNotBlank() && frame.data.trim() != "[DONE]") {
                        checkedEcho = true
                        val obj = runCatching { ApiCallJson.parse(frame.data) }.getOrNull()
                        val echoed = (obj?.get("stylePreview") as? JsonPrimitive)?.booleanOrNull == true
                        if (!echoed) { emit(PreviewEvent.NoEcho); return@use }
                        emit(PreviewEvent.Echo)
                    }
                    when (val e = AgentChatFrames.decode(frame)) {
                        is ChatStreamEvent.TextDelta -> emit(PreviewEvent.Text(e.text))
                        is ChatStreamEvent.StreamError -> { emit(PreviewEvent.Failed(null, e.code, e.message)); return@use }
                        is ChatStreamEvent.Failed -> { emit(PreviewEvent.Failed(e.status, e.code, e.message)); return@use }
                        is ChatStreamEvent.Finish -> {
                            if (e.error != null) emit(PreviewEvent.Failed(null, null, e.errorMessage ?: e.error)) else emit(PreviewEvent.Done)
                            return@use
                        }
                        ChatStreamEvent.Done -> { emit(PreviewEvent.Done); return@use }
                        else -> Unit
                    }
                }
            }
            emit(PreviewEvent.Done)
        }
    } catch (c: CancellationException) {
        throw c
    } catch (io: IOException) {
        if (!call.isCanceled()) emit(PreviewEvent.Failed(null, "network", io.message))
    } finally {
        call.cancel()
    }
}.flowOn(Dispatchers.IO)

private object ApiCallJson {
    fun parse(text: String): JsonObject? = com.allternit.core.api.ApiJson.default.parseToJsonElement(text.trim()) as? JsonObject
}
