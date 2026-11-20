package com.allternit.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl

/** A published thread share (iOS ThreadShareEntry). */
data class ThreadShareEntry(val token: String, val url: String)

/** iOS ThreadShareError copy. */
private object ShareCopy {
    const val SIGNED_OUT = "Sign in to Allternit Cloud to share a link."
    const val TOO_LARGE = "This chat is too large to share as a link."
    const val EMPTY = "There is nothing to share yet. Send a message first."
    const val OFFLINE = "You appear to be offline. Try again when you are connected."
}

private class ShareFailure(val text: String, val retryable: Boolean)

/**
 * `{cloud}/api/v2/thread-shares` (iOS ThreadShareClient): look up a chat's share link (404 = not
 * shared), create or update it with a snapshot of the messages and tool names, delete it.
 */
internal class ThreadShareClient(private val backend: ChatBackend) {
    private val json = ApiJson.default

    private fun url(token: String? = null, sessionId: String? = null): HttpUrl =
        backend.cloudBaseUrl.newBuilder().addPathSegments("api/v2/thread-shares").apply {
            token?.let { addPathSegment(it) }
            sessionId?.let { addQueryParameter("sessionId", it) }
        }.build()

    /** iOS buildSnapshot: user and assistant turns with their text and the names of tools used. */
    fun snapshot(messages: List<MessageRecord>): JsonArray = buildJsonArray {
        messages.forEach { m ->
            if (m.role != "user" && m.role != "assistant") return@forEach
            val text = m.content.trim()
            val tools = m.toolCalls.map { it.toolName }.filter { it.isNotEmpty() }.distinct()
            if (text.isEmpty() && tools.isEmpty()) return@forEach
            add(buildJsonObject {
                put("role", m.role)
                put("text", text)
                if (tools.isNotEmpty()) put("tools", JsonArray(tools.map { JsonPrimitive(it) }))
            })
        }
    }

    private fun entry(o: JsonObject): ThreadShareEntry? {
        val token = o.str("token") ?: return null
        return ThreadShareEntry(token, o.str("url") ?: return null)
    }

    private fun failure(r: ApiResult<*>, method: String): ShareFailure = when (r) {
        is ApiResult.HttpError -> when (r.status) {
            401 -> ShareFailure(ShareCopy.SIGNED_OUT, false)
            413 -> ShareFailure(ShareCopy.TOO_LARGE, false)
            422 -> ShareFailure(ShareCopy.EMPTY, false)
            else -> ShareFailure("Something went wrong (${r.status}). Try again.", true)
        }
        is ApiResult.Network -> ShareFailure(ShareCopy.OFFLINE, true)
        else -> ShareFailure(if (method == "PUT") "Updated, but the server reply was unreadable." else "Something went wrong. Try again.", true)
    }

    suspend fun lookup(sessionId: String): Result<ThreadShareEntry?> {
        val r = ApiCall.execute(backend.authedHttp, json, ApiCall.request("GET", url(sessionId = sessionId), null), JsonObject.serializer())
        return when {
            r is ApiResult.Ok -> Result.success(entry(r.value))
            r is ApiResult.HttpError && r.status == 404 -> Result.success(null)
            else -> Result.failure(ShareException(failure(r, "GET")))
        }
    }

    suspend fun createOrUpdate(sessionId: String, title: String, snapshot: JsonArray): Result<ThreadShareEntry> {
        val body = buildJsonObject {
            put("sessionId", sessionId)
            put("title", title)
            put("snapshot", snapshot)
        }.toString()
        val r = ApiCall.execute(backend.authedHttp, json, ApiCall.request("POST", url(), body), JsonObject.serializer())
        return (r as? ApiResult.Ok)?.value?.let(this::entry)?.let { Result.success(it) } ?: Result.failure(ShareException(failure(r, "POST")))
    }

    suspend fun update(token: String, title: String, snapshot: JsonArray): Result<ThreadShareEntry> {
        val body = buildJsonObject {
            put("snapshot", snapshot)
            put("title", title)
        }.toString()
        val r = ApiCall.execute(backend.authedHttp, json, ApiCall.request("PUT", url(token = token), body), JsonObject.serializer())
        return (r as? ApiResult.Ok)?.value?.let(this::entry)?.let { Result.success(it) } ?: Result.failure(ShareException(failure(r, "PUT")))
    }

    suspend fun delete(token: String): Result<Unit> {
        val r = ApiCall.executeUnit(backend.authedHttp, json, ApiCall.request("DELETE", url(token = token), null))
        return if (r is ApiResult.Ok) Result.success(Unit) else Result.failure(ShareException(failure(r, "DELETE")))
    }
}

private class ShareException(val failure: ShareFailure) : Exception(failure.text)

private sealed interface SharePhase {
    data object Loading : SharePhase
    data object NotShared : SharePhase
    data class Ready(val entry: ThreadShareEntry) : SharePhase
    data class Failed(val text: String, val retryable: Boolean) : SharePhase
}

/**
 * "Share link" for a chat (iOS ThreadShareSheet), from the chat's ⋯ menu and a reply's Share
 * button. Opening it only looks the link up; **Create link** publishes a snapshot. Then **Share
 * link**, **Update link** (re-snapshot now) and **Stop sharing** (confirmed).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ThreadShareSheet(backend: ChatBackend, sessionId: String?, title: String, messages: List<MessageRecord>, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val client = remember(backend) { ThreadShareClient(backend) }
    var phase by remember { mutableStateOf<SharePhase>(SharePhase.Loading) }
    var busy by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    var confirmStop by remember { mutableStateOf(false) }

    fun fail(e: Throwable?): SharePhase.Failed {
        val f = (e as? ShareException)?.failure
        return SharePhase.Failed(f?.text ?: "Something went wrong.", f?.retryable ?: true)
    }
    suspend fun start() {
        val id = sessionId ?: run { phase = SharePhase.Failed(ShareCopy.EMPTY, false); return }
        phase = SharePhase.Loading
        client.lookup(id).fold({ phase = it?.let { e -> SharePhase.Ready(e) } ?: SharePhase.NotShared }, { phase = fail(it) })
    }
    LaunchedEffect(sessionId) { start() }

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).testTag("threadShareSheet"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Share link", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f).semantics { heading() })
                Text("Done", color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button, onClick = onDismiss).padding(8.dp))
            }
            when (val p = phase) {
                SharePhase.Loading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Checking…", color = c.textSecondary, fontSize = 15.sp)
                }
                SharePhase.NotShared -> {
                    Text(
                        "This chat isn't shared. Creating a link makes a public snapshot of the messages and the names of tools used. Anyone with the link can see it. New messages are not added until you update the link, and you can stop sharing at any time.",
                        color = c.textSecondary,
                        fontSize = 13.sp,
                        lineHeight = 18.sp,
                    )
                    CapsuleButton("Create link", icon = DesignR.drawable.ph_link_simple, primary = true, enabled = !busy) {
                        val id = sessionId ?: return@CapsuleButton
                        val snap = client.snapshot(messages)
                        if (snap.isEmpty()) {
                            phase = SharePhase.Failed(ShareCopy.EMPTY, false)
                            return@CapsuleButton
                        }
                        busy = true
                        scope.launch {
                            client.createOrUpdate(id, title, snap).fold({ phase = SharePhase.Ready(it) }, { phase = fail(it) })
                            busy = false
                        }
                    }
                }
                is SharePhase.Failed -> {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusWarning, modifier = Modifier.size(16.dp))
                        Text(p.text, color = c.textPrimary, fontSize = 15.sp)
                    }
                    if (p.retryable) CapsuleButton("Try again", primary = true) { scope.launch { start() } }
                }
                is SharePhase.Ready -> {
                    SelectionContainer {
                        Text(
                            p.entry.url,
                            color = c.textPrimary,
                            fontSize = 14.sp,
                            fontFamily = FontFamily.Monospace,
                            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.textSecondary.copy(alpha = 0.08f)).padding(12.dp),
                        )
                    }
                    Text(
                        "Anyone with this link can see a snapshot of this chat: the messages and the names of tools used, as of the last update. New messages are not added until you update the link.",
                        color = c.textSecondary,
                        fontSize = 13.sp,
                        lineHeight = 18.sp,
                    )
                    CapsuleButton("Share link", icon = com.allternit.core.design.R.drawable.ph_export, primary = true) { shareText(context, p.entry.url) }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CapsuleButton("Update link", icon = com.allternit.core.design.R.drawable.ph_arrow_clockwise, enabled = !busy) {
                            val snap = client.snapshot(messages)
                            if (snap.isEmpty()) {
                                notice = ShareCopy.EMPTY
                                return@CapsuleButton
                            }
                            busy = true
                            scope.launch {
                                client.update(p.entry.token, title, snap).fold(
                                    {
                                        phase = SharePhase.Ready(it)
                                        notice = "Link updated with the latest messages."
                                    },
                                    { notice = (it as? ShareException)?.failure?.text ?: it.message },
                                )
                                busy = false
                            }
                        }
                        Spacer(Modifier.weight(1f))
                        Text(
                            "Stop sharing",
                            color = AllternitStatic.StatusError,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.clickable(enabled = !busy, role = Role.Button) { confirmStop = true }.padding(8.dp),
                        )
                    }
                    notice?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
                    if (confirmStop) {
                        AlertDialog(
                            onDismissRequest = { confirmStop = false },
                            title = { Text("Stop sharing this chat?") },
                            text = { Text("The link will stop working for everyone who has it.") },
                            confirmButton = {
                                TextButton(onClick = {
                                    confirmStop = false
                                    busy = true
                                    scope.launch {
                                        client.delete(p.entry.token).fold({ onDismiss() }, { notice = (it as? ShareException)?.failure?.text ?: it.message })
                                        busy = false
                                    }
                                }) { Text("Stop sharing", color = AllternitStatic.StatusError) }
                            },
                            dismissButton = { TextButton(onClick = { confirmStop = false }) { Text("Cancel") } },
                        )
                    }
                }
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}
