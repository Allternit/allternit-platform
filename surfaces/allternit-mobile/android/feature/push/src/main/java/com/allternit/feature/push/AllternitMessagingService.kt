package com.allternit.feature.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import kotlinx.coroutines.launch

/**
 * FCM entry point (iOS AppDelegate `didRegisterForRemoteNotificationsWithDeviceToken` and the APNs alert itself).
 * cloud-api sends data-only messages (routes/fcm.rs), so this runs for every push, foreground or not, and
 * [PushNotifications] builds the notification with the iOS category's buttons.
 */
class AllternitMessagingService : FirebaseMessagingService() {
    private val scope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + kotlinx.coroutines.Dispatchers.IO)

    override fun onNewToken(token: String) {
        AllternitPush.onNewToken(applicationContext, token)
    }

    private fun refreshSystemSurfaces() {
        val app = applicationContext
        scope.launch {
            val snap = runCatching { com.allternit.feature.system.StatusSync.refresh(app) }.getOrNull()
            LiveRunNotification.update(app, snap)
        }
    }

    /**
     * Backend step 2.8 sends `{type:"computer_alert", computerId, rule, value}` (value = percent for cpu/disk, minutes for
     * offline). When the push has no title/body of its own, say it in plain words (same sentences as the console's
     * Needs attention). One notification per computer and rule, so a newer one replaces the older.
     */
    private fun withComputerAlertText(data: Map<String, String>): Map<String, String> {
        if (data["type"] != "computer_alert") return data
        val value = data["value"]?.toDoubleOrNull()?.toInt()
        val (title, body) = when (data["rule"]) {
            "offline" -> "A computer is offline" to (if (value != null) "It has been offline for $value minutes." else "It is offline.")
            "cpu" -> "A computer is busy" to (if (value != null) "CPU is at $value%." else "CPU is high.")
            "disk" -> "A computer is running out of space" to (if (value != null) "Disk is $value% full." else "Disk is almost full.")
            else -> "Computer alert" to "A computer needs a look."
        }
        return data.toMutableMap().apply {
            if (get("title").isNullOrBlank()) put("title", title)
            if (get("body").isNullOrBlank()) put("body", body)
            if (get("tag").isNullOrBlank()) put("tag", "computer-alert-${data["computerId"].orEmpty()}-${data["rule"].orEmpty()}")
        }
    }

    /**
     * Agent browser pushes (CONTRACT.md): `kind` is `aci_approval`, `aci_human` or `aci_blocked`. They get their
     * category (buttons), one notification per run, the computer id the deep link needs, and plain-words text when
     * the push has none. Never carries a credential value, only its name and domain.
     */
    private fun withAciText(data: Map<String, String>): Map<String, String> {
        val kind = data["kind"] ?: data["type"]
        val category = when (kind) {
            "aci_approval" -> PushNotifications.ACI_APPROVAL
            "aci_human" -> PushNotifications.ACI_HUMAN
            "aci_blocked" -> PushNotifications.ACI_BLOCKED
            else -> return data
        }
        val out = data.toMutableMap()
        out["category"] = category
        if (out["computerId"].isNullOrBlank()) data["computer_id"]?.let { out["computerId"] = it }
        if (out["url"].isNullOrBlank() && !data["computer_id"].isNullOrBlank() && !data["run_id"].isNullOrBlank()) {
            out["url"] = "/computers/${data["computer_id"]}?run=${data["run_id"]}"
        }
        if (out["tag"].isNullOrBlank()) out["tag"] = "aci-${data["run_id"].orEmpty()}"
        val (title, body) = when (category) {
            PushNotifications.ACI_APPROVAL -> {
                val name = data["credential_name"] ?: data["credential"]?.takeIf { !it.startsWith("{") }
                val domain = data["credential_domain"]
                ("Your bot needs you") to (data["message"] ?: if (name != null && domain != null) "Fill your $name login on $domain?" else "Approve what it wants to do.")
            }
            PushNotifications.ACI_HUMAN -> "Your bot needs you" to (data["reason"] ?: "It needs you to do a step.")
            else -> (data["domain"]?.let { "$it blocked your bot" } ?: "A site blocked your bot") to "Open Allternit to continue, stop or re-run."
        }
        if (out["title"].isNullOrBlank()) out["title"] = title
        if (out["body"].isNullOrBlank()) out["body"] = body
        return out
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val data = withAciText(withComputerAlertText(message.data))
        // Wave I2: every push also refreshes the status snapshot (widget, tiles, shortcuts) and the ongoing run notification.
        refreshSystemSurfaces()
        if (data["event"] == "notification.revoke") {
            PushNotifications.revoke(applicationContext, data["tags"].orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }.toSet())
            return
        }
        if (data["title"].isNullOrBlank() && data["body"].isNullOrBlank()) return
        // Gap run GS-B (iOS AllternitNotifications): a private push carries a generic body; fetch the real text by event
        // id with the device read token. Any failure shows the generic content (never dropped).
        val eventId = PrivateBodies.privateEventId(data)
        if (eventId != null) {
            val content = runCatching { PrivateBodies.fetch(applicationContext, eventId) }.getOrNull()
            if (content != null) {
                val next = data.toMutableMap()
                content.title?.let { next["title"] = it }
                content.body?.let { next["body"] = it }
                val sender = if (content.communication && content.senderName != null) PushNotifications.Sender(content.senderName, content.avatar) else null
                PushNotifications.show(applicationContext, next, sender, content.avatar)
                return
            }
        }
        PushNotifications.show(applicationContext, data)
    }
}
