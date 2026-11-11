package com.allternit.phonelink

import com.allternit.app.R
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build

object Notifications {
    const val CHANNEL_SERVICE = "phonelink_service"
    const val CHANNEL_PAIRING = "phonelink_pairing"
    const val CHANNEL_HOLD = "phonelink_hold"
    const val ID_SERVICE = 1
    const val ID_HOLD = 2

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL_SERVICE, "Phone Link running", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while Allternit can use your phone's apps."
            setShowBadge(false)
        })
        nm.createNotificationChannel(NotificationChannel(CHANNEL_HOLD, "Waiting for your approval", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while a bot's action on this phone waits for you to approve it in Allternit."
            setShowBadge(false)
        })
        nm.createNotificationChannel(NotificationChannel(CHANNEL_PAIRING, "Pairing requests", NotificationManager.IMPORTANCE_HIGH).apply {
            description = "A computer asks to pair with this phone."
        })
    }

    private fun immutable() = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE

    fun openAppIntent(context: Context): PendingIntent =
        PendingIntent.getActivity(context, 0, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), immutable())

    fun serviceNotification(context: Context, detail: String): Notification {
        val stop = PendingIntent.getBroadcast(context, 1, Intent(context, StopReceiver::class.java), immutable())
        val stopAction = Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_stat_link), "Stop", stop).build()
        return Notification.Builder(context, CHANNEL_SERVICE)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle("Allternit can use your phone's apps")
            .setContentText(detail)
            .setOngoing(true)
            .setContentIntent(openAppIntent(context))
            .addAction(stopAction)
            .apply { if (Build.VERSION.SDK_INT >= 31) setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE) }
            .build()
    }

    /** Ongoing notice while a held action waits for approval. Removes itself after [timeoutMs] as a safety net. */
    fun holdNotification(context: Context, body: String, timeoutMs: Long): Notification =
        Notification.Builder(context, CHANNEL_HOLD)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle("Waiting for your approval in Allternit")
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setTimeoutAfter(timeoutMs)
            .setContentIntent(openAppIntent(context))
            .build()

    fun showHold(context: Context, body: String, timeoutMs: Long) {
        ensureChannels(context)
        context.getSystemService(NotificationManager::class.java).notify(ID_HOLD, holdNotification(context, body, timeoutMs))
    }

    fun clearHold(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(ID_HOLD)
    }
}
