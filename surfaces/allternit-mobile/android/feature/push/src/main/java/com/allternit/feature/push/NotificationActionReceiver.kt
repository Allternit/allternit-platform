package com.allternit.feature.push

import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import com.allternit.core.auth.AllternitAuth
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.chat.RuntimeCall
import java.net.URLEncoder
import java.util.TimeZone
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/**
 * The notification buttons that run without opening the app: Approve, Deny, Reply, Retry, Snooze (iOS
 * `NotificationActions.resolve` / `handle`). Same routes as the in-app inbox (`POST /api/v1/inbox/:id/:verb`
 * through the computer) and the bot chat (`POST /api/v1/agent-sessions/:id/messages`), with `surface:
 * "notification"`. Finishes within ~25 s (the receiver's goAsync window), then:
 * - done or already handled → the notification goes;
 * - changed (409) → replaced by "This changed. Review it again.";
 * - offline, signed out or failed → a follow-up line that opens the item.
 */
class NotificationActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.getStringExtra(EXTRA_ACTION) ?: return
        val tag = intent.getStringExtra(EXTRA_TAG).orEmpty()
        val title = intent.getStringExtra(EXTRA_TITLE).orEmpty()
        val data = PushNotifications.PASSED_KEYS.mapNotNull { k -> intent.getStringExtra("data.$k")?.let { k to it } }.toMap()
        val reply = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(PushNotifications.KEY_REPLY_TEXT)?.toString()
        val app = context.applicationContext
        val pending = goAsync()
        scope.launch {
            try {
                val result = try {
                    withTimeout(25_000) { resolve(app, action, data, reply) }
                } catch (_: TimeoutCancellationException) {
                    Result.Offline
                }
                apply(app, result, tag, title, data)
            } finally {
                pending.finish()
            }
        }
    }

    private enum class Result { Done, AlreadyHandled, Changed, Offline, SignedOut, Failed, Invalid }

    private suspend fun resolve(context: Context, action: String, data: Map<String, String>, reply: String?): Result {
        val inboxId = data["inboxId"].orEmpty()
        val sessionId = data["sessionId"] ?: data["threadSessionId"]
        val (path, body) = when (action) {
            PushNotifications.ACTION_APPROVE -> inboxPath(inboxId, "approve") to buildJsonObject {
                put("surface", "notification")
                data["contentHash"]?.takeIf { it.isNotBlank() }?.let { put("contentHash", it) }
            }
            PushNotifications.ACTION_DENY -> inboxPath(inboxId, "approve") to buildJsonObject {
                put("decision", "deny")
                put("surface", "notification")
            }
            PushNotifications.ACTION_RETRY -> inboxPath(inboxId, "retry") to JsonObject(emptyMap())
            PushNotifications.ACTION_SNOOZE -> inboxPath(inboxId, "snooze") to buildJsonObject { put("minutes", 60) }
            PushNotifications.ACTION_REPLY -> {
                val text = reply?.trim().orEmpty()
                if (sessionId.isNullOrBlank() || text.isEmpty()) return Result.Invalid
                "/api/v1/agent-sessions/${esc(sessionId)}/messages" to buildJsonObject {
                    put("text", text)
                    putJsonObject("metadata") { put("time_zone", TimeZone.getDefault().id) }
                }
            }
            else -> return Result.Invalid
        }
        if (action != PushNotifications.ACTION_REPLY && inboxId.isEmpty()) return Result.Invalid
        // A cold process (the app was closed): start auth so the bearer is there.
        AllternitAuth.start(context)
        if (AllternitAuth.tokenProvider.bearerToken().isNullOrBlank()) return Result.SignedOut
        return when (val r = ChatBackends.create(context).runtimeJson("POST", path, body.toString(), targetRuntimeId = data["runtimeId"])) {
            is RuntimeCall.Ok -> Result.Done
            is RuntimeCall.Http -> when (r.status) {
                404 -> Result.AlreadyHandled
                409 -> Result.Changed
                401, 403 -> Result.SignedOut
                502, 503, 504 -> Result.Offline
                else -> Result.Failed
            }
            RuntimeCall.NoComputer -> Result.Offline
            else -> Result.Offline
        }
    }

    private fun apply(context: Context, result: Result, tag: String, title: String, data: Map<String, String>) {
        when (result) {
            Result.Done, Result.AlreadyHandled -> PushNotifications.cancel(context, tag)
            Result.Changed -> {
                PushNotifications.cancel(context, tag)
                PushNotifications.followUp(context, "This changed. Review it again.", title, data)
            }
            Result.Offline -> PushNotifications.followUp(context, "Your computer is offline. Open Allternit to retry.", title, data)
            Result.SignedOut -> PushNotifications.followUp(context, "Open Allternit to sign in, then try again.", title, data)
            Result.Failed -> PushNotifications.followUp(context, "That didn't go through. Open Allternit to try again.", title, data)
            Result.Invalid -> Unit
        }
    }

    private fun inboxPath(id: String, verb: String) = "/api/v1/inbox/${esc(id)}/$verb"

    private fun esc(s: String) = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    companion object {
        const val EXTRA_ACTION = "com.allternit.push.action"
        const val EXTRA_TAG = "com.allternit.push.tag"
        const val EXTRA_TITLE = "com.allternit.push.title"
        private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    }
}
