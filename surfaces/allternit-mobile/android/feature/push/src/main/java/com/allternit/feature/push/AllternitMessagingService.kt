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

    override fun onMessageReceived(message: RemoteMessage) {
        val data = message.data
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
