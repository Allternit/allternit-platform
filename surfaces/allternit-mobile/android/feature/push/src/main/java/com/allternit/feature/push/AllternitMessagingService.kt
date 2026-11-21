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

    override fun onMessageReceived(message: RemoteMessage) {
        val data = withComputerAlertText(message.data)
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
