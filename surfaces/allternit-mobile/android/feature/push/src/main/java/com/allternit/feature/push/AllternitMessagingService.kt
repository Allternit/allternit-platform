package com.allternit.feature.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * FCM entry point (iOS AppDelegate `didRegisterForRemoteNotificationsWithDeviceToken` and the APNs alert itself).
 * cloud-api sends data-only messages (routes/fcm.rs), so this runs for every push, foreground or not, and
 * [PushNotifications] builds the notification with the iOS category's buttons.
 */
class AllternitMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        AllternitPush.onNewToken(applicationContext, token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        val data = message.data
        if (data["event"] == "notification.revoke") {
            PushNotifications.revoke(applicationContext, data["tags"].orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }.toSet())
            return
        }
        if (data["title"].isNullOrBlank() && data["body"].isNullOrBlank()) return
        PushNotifications.show(applicationContext, data)
    }
}
