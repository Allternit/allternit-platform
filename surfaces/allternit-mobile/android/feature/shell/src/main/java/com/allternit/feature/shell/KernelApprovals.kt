package com.allternit.feature.shell

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyErrorMessage
import java.time.Instant
import java.time.OffsetDateTime
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl

// AllternitOS kernel approvals (Stage 1 step 20), the port of iOS PR #3309 (InboxClient.KernelApproval,
// KernelResolveError, InboxSheet KernelApprovalRow / KernelApprovalSheet). They live in the Needs-you sheet;
// there is no tab. The model never approves one: only the signed-in person does, after reading the exact action.

/**
 * A pending kernel approval, from the cloud relay `GET /api/v1/kernel/relay/approvals`
 * (cloud-api routes/kernel_relay.rs). A row the app cannot read is skipped, never fails the list.
 */
internal data class KernelApproval(
    val runtimeId: String,
    val requestId: String,
    val runId: String?,
    /** Who asked (the agent or bot the kernel run belongs to). */
    val assignee: String?,
    val actionClass: String?,
    /** The exact action, as the kernel wrote it. Shown whole in the detail. */
    val actionSummary: String?,
    val actionHash: String?,
    /** Epoch millis; the wire gives ISO-8601 or epoch seconds/milliseconds. */
    val deadlineMs: Long?,
    val humanOnly: Boolean,
) {
    val id: String get() = key(runtimeId, requestId)

    companion object {
        fun key(runtimeId: String, requestId: String) = "$runtimeId/$requestId"

        fun from(o: JsonObject): KernelApproval? {
            val runtimeId = o.str("runtimeId")?.takeIf { it.isNotEmpty() } ?: return null
            val k = o["kernel"] as? JsonObject ?: return null
            val requestId = k.str("requestId")?.takeIf { it.isNotEmpty() } ?: return null
            return KernelApproval(
                runtimeId = runtimeId,
                requestId = requestId,
                runId = k.str("runId"),
                assignee = k.str("assignee"),
                actionClass = k.str("actionClass"),
                actionSummary = k.str("actionSummary"),
                actionHash = k.str("actionHash"),
                deadlineMs = deadlineMillis(k["deadline"]),
                // Unknown means human-only: the safe reading.
                humanOnly = (k["humanOnly"] as? JsonPrimitive)?.booleanOrNull ?: true,
            )
        }

        /** iOS KernelApproval.Kernel.deadline: ISO-8601 (with or without fractions), or epoch seconds / milliseconds. */
        fun deadlineMillis(raw: JsonElement?): Long? {
            val p = raw as? JsonPrimitive ?: return null
            if (p.isString) {
                val text = p.contentOrNull ?: return null
                return runCatching { OffsetDateTime.parse(text).toInstant().toEpochMilli() }.getOrNull()
                    ?: runCatching { Instant.parse(text).toEpochMilli() }.getOrNull()
            }
            val n = p.doubleOrNull ?: return null
            return (if (n > 1e11) n else n * 1000).toLong()
        }

        /** The list body `{ items: [...] }`; unreadable rows are dropped. */
        fun list(body: JsonElement?): List<KernelApproval> =
            ((body as? JsonObject)?.get("items") as? JsonArray).orEmpty().mapNotNull { (it as? JsonObject)?.let(::from) }
    }
}

private fun JsonObject.str(key: String): String? = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

/** Plain reasons a kernel approval could not be answered (iOS KernelResolveError, same words). */
internal object KernelResolveMessages {
    const val FORBIDDEN_CALLER = "Only you can answer this, from your signed-in account on this phone."
    const val TOKEN_EXPIRED = "This request ran out of time. Ask the agent to send it again."
    const val KERNEL_UNAVAILABLE = "Your computer's kernel can't be reached right now. Try again in a moment."
    const val OTHER = "That didn't go through. Try again."

    /** The same order as iOS: code or status for forbidden, then token, then unavailable. */
    fun forFailure(status: Int, code: String?): String? = when {
        code == "forbidden_caller" || status == 403 -> FORBIDDEN_CALLER
        code == "token_expired" || status == 409 -> TOKEN_EXPIRED
        code == "kernel_unavailable" || status == 503 -> KERNEL_UNAVAILABLE
        else -> null
    }

    fun isTokenExpired(status: Int, code: String?) = forFailure(status, code) == TOKEN_EXPIRED
}

/** What a resolve call came to. */
internal sealed interface KernelResolve {
    data object Done : KernelResolve
    /** [expired]: the request is gone on the kernel, so the row leaves the list. */
    data class Failed(val message: String, val expired: Boolean) : KernelResolve
}

/** Cloud-api calls for kernel approvals, with the signed-in session only (iOS InboxClient.kernelApprovals / resolveKernel). */
internal object KernelApprovalClient {
    private fun base(): HttpUrl = ApiConfig().cloudBaseUrl.toString().trimEnd('/').toHttpUrl()

    /** `GET /api/v1/kernel/relay/approvals`. Null when it could not be read: the caller keeps its last list. */
    suspend fun list(): List<KernelApproval>? {
        val url = base().newBuilder().addPathSegments("api/v1/kernel/relay/approvals").build()
        return when (val r = ApiCall.execute(ShellApi.okHttp, com.allternit.core.api.ApiJson.default, ApiCall.request("GET", url, null), JsonElement.serializer())) {
            is ApiResult.Ok -> KernelApproval.list(r.value)
            else -> null
        }
    }

    /** `POST /api/v1/kernel/relay/approvals/:runtime_id/:request_id/resolve` `{ decision, note? }`. */
    suspend fun resolve(approval: KernelApproval, approve: Boolean, note: String? = null): KernelResolve {
        val url = base().newBuilder().addPathSegments("api/v1/kernel/relay/approvals")
            .addPathSegment(approval.runtimeId).addPathSegment(approval.requestId).addPathSegment("resolve").build()
        val body = buildJsonObject {
            put("decision", if (approve) "approve" else "deny")
            if (!note.isNullOrEmpty()) put("note", note)
        }
        // A 2xx with a body the app cannot read is still an answered request (the relay may return an empty body).
        return when (val r = ApiCall.execute(ShellApi.okHttp, com.allternit.core.api.ApiJson.default, ApiCall.request("POST", url, body.toString()), JsonElement.serializer())) {
            is ApiResult.Ok, is ApiResult.Decode -> KernelResolve.Done
            is ApiResult.HttpError -> KernelResolve.Failed(
                KernelResolveMessages.forFailure(r.status, r.code) ?: statusSentence(r.status) ?: KernelResolveMessages.OTHER,
                expired = KernelResolveMessages.isTokenExpired(r.status, r.code),
            )
            is ApiResult.Network -> KernelResolve.Failed(FriendlyErrorMessage.from(r.cause.message ?: "", "answer that"), expired = false)
        }
    }
}

/** `allternit://inbox/kernel/<runtime_id>/<request_id>` (the KERNEL_APPROVAL push), from host + decoded path segments. */
internal object KernelApprovalLink {
    fun parse(segments: List<String>): String? {
        if (segments.size < 4 || segments[0] != "inbox" || segments[1] != "kernel") return null
        val runtime = segments[2].takeIf { it.isNotEmpty() } ?: return null
        val request = segments[3].takeIf { it.isNotEmpty() } ?: return null
        return KernelApproval.key(runtime, request)
    }
}

internal object KernelText {
    fun classLabel(raw: String?): String {
        if (raw.isNullOrEmpty()) return "Action"
        val spaced = raw.replace("_", " ").replace("-", " ")
        return spaced.take(1).uppercase() + spaced.drop(1)
    }

    /** "12 min left", "Ran out of time", or null when the kernel gave no deadline. */
    fun timeLeft(deadlineMs: Long?, nowMs: Long = System.currentTimeMillis()): String? {
        if (deadlineMs == null) return null
        val seconds = ((deadlineMs - nowMs) / 1000).toInt()
        if (seconds <= 0) return "Ran out of time"
        if (seconds < 90) return "$seconds sec left"
        val minutes = Math.round(seconds / 60.0).toInt()
        if (minutes < 60) return "$minutes min left"
        val hours = Math.round(minutes / 60.0).toInt()
        return if (hours < 24) "$hours hr left" else "${Math.round(hours / 24.0)} d left"
    }
}

/** A kernel approval in the Needs-you list: the action in a few lines; the whole text and the buttons are in the detail. */
@Composable
internal fun KernelApprovalRow(approval: KernelApproval, focused: Boolean, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier
            .fillMaxWidth()
            .background(if (focused) c.accentPrimary.copy(alpha = 0.12f) else c.bgPrimary)
            .testTag("kernelRow${approval.id}")
            .clickable(role = Role.Button, onClick = onOpen)
            .semantics { contentDescription = "Approval. Opens the full action to approve or deny" }
            .padding(horizontal = 20.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            Text("APPROVAL · ${KernelText.classLabel(approval.actionClass).uppercase()}", color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
            approval.assignee?.takeIf { it.isNotEmpty() }?.let {
                Text(it, color = c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
            }
            androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            KernelText.timeLeft(approval.deadlineMs)?.let { Text(it, color = c.textSecondary, fontSize = 11.sp) }
        }
        Text(
            approval.actionSummary ?: "An action is waiting for your answer.",
            color = c.textPrimary, fontSize = 15.sp, maxLines = 3, overflow = TextOverflow.Ellipsis,
        )
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            if (approval.humanOnly) Text("Only you can approve", color = c.textSecondary, fontSize = 13.sp)
            androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            Text("Review", color = c.accentPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

/**
 * The exact action, never truncated, with Approve and Deny. Both call the relay with the signed-in
 * session; neither is offered on a notification.
 */
@Composable
internal fun KernelApprovalSheet(approval: KernelApproval, store: InboxStore, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    /** null = idle, true = approving, false = denying. */
    var working by remember { mutableStateOf<Boolean?>(null) }
    var failure by remember { mutableStateOf<String?>(null) }
    val stillPending = store.kernelItems.any { it.id == approval.id }

    fun answer(approve: Boolean) {
        if (working != null) return
        working = approve
        failure = null
        scope.launch {
            val error = store.resolveKernel(approval, approve)
            working = null
            if (error != null) failure = error else onClose()
        }
    }

    ShellSheet(
        title = "Approval",
        onDismiss = onClose,
        testTag = "kernelApprovalSheet",
        trailing = SheetAction("Close", onClose, testTag = "kernelClose"),
    ) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("Exactly what will happen", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            SelectionContainer {
                Text(
                    approval.actionSummary ?: "No description came with this request. Deny it unless you know what it is.",
                    color = c.textPrimary, fontSize = 17.sp,
                    modifier = Modifier.fillMaxWidth().testTag("kernelActionText"),
                )
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                KernelFact("Kind", KernelText.classLabel(approval.actionClass))
                approval.assignee?.takeIf { it.isNotEmpty() }?.let { KernelFact("Asked by", it) }
                KernelText.timeLeft(approval.deadlineMs)?.let { KernelFact("Time", it) }
                approval.actionHash?.takeIf { it.isNotEmpty() }?.let { KernelFact("Action ID", it.take(12)) }
            }
            if (approval.humanOnly) {
                Text("Only you can approve this. An agent cannot approve it for you.", color = c.textSecondary, fontSize = 13.sp)
            }
            failure?.let {
                Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.semantics { contentDescription = "Could not answer: $it" })
            }
            if (!stillPending && working == null) Text("Already handled", color = c.textSecondary, fontSize = 13.sp)
            Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                val enabled = working == null && stillPending
                KernelButton(if (working == false) "Denying…" else "Deny", prominent = false, enabled = enabled, testTag = "kernelDeny") { answer(false) }
                KernelButton(if (working == true) "Approving…" else "Approve", prominent = true, enabled = enabled, testTag = "kernelApprove") { answer(true) }
            }
        }
    }
}

@Composable
private fun KernelFact(label: String, value: String) {
    val c = AllternitTheme.colors
    Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
        Text(label, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.width(76.dp))
        Text(value, color = c.textPrimary, fontSize = 13.sp)
    }
}

@Composable
private fun KernelButton(title: String, prominent: Boolean, enabled: Boolean, testTag: String, onClick: () -> Unit) {
    InboxButton(title = title, prominent = prominent, enabled = enabled, testTag = testTag, onClick = onClick)
}
