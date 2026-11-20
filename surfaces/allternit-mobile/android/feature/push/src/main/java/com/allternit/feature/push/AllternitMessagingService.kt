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
        PushNotifications.show(applicationContext, data)
    }
}
