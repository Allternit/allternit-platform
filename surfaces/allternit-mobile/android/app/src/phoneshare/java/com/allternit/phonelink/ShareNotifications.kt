package com.allternit.phonelink

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.os.Build
import com.allternit.app.R

/** Screen-share notifications (both builds): the "tap to share" request and the ongoing "sharing" notice. */
object ShareNotifications {
    const val CHANNEL_REQUEST = "screen_share_request"
    const val CHANNEL_SHARING = "screen_sharing"
    const val ID_REQUEST = 3101
    const val ID_SHARING = 3102

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL_REQUEST, "Screen share requests", NotificationManager.IMPORTANCE_HIGH).apply {
            description = "Someone in your Allternit account wants to see this phone's screen."
        })
        nm.createNotificationChannel(NotificationChannel(CHANNEL_SHARING, "Sharing your screen", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while this phone's screen is shared to your Allternit account."
            setShowBadge(false)
        })
    }

    private fun flags() = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE

    private fun canPost(context: Context): Boolean =
        Build.VERSION.SDK_INT < 33 ||
            context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    /**
     * "Someone wants to see your screen — tap to share". The tap opens Android's
     * own screen-share prompt (ProjectionConsentActivity); nothing starts until
     * the person accepts it there.
     */
    fun showRequest(context: Context) {
        ensureChannels(context)
        if (!canPost(context)) return
        val open = PendingIntent.getActivity(
            context, ID_REQUEST,
            Intent(context, ProjectionConsentActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            flags(),
        )
        val n = Notification.Builder(context, CHANNEL_REQUEST)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle("Someone wants to see your screen")
            .setContentText("Tap to share")
            .setCategory(Notification.CATEGORY_CALL)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setTimeoutAfter(2 * 60_000L)
            .build()
        try { context.getSystemService(NotificationManager::class.java).notify(ID_REQUEST, n) } catch (_: SecurityException) {}
    }

    fun cancelRequest(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(ID_REQUEST)
    }

    /** The foreground-service notice while sharing, with Stop. */
    fun sharing(context: Context, live: Boolean): Notification {
        ensureChannels(context)
        val stop = PendingIntent.getService(context, 1, ProjectionService.stopIntent(context), flags())
        val open = PendingIntent.getActivity(
            context, 2, Intent(context, ScreenShareActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), flags(),
        )
        return Notification.Builder(context, CHANNEL_SHARING)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(if (live) "Sharing your screen" else "Screen share ready")
            .setContentText(if (live) "Someone in your Allternit account is watching." else "Nobody is watching. It stops after 10 minutes.")
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_stat_link), "Stop", stop).build())
            .apply { if (Build.VERSION.SDK_INT >= 31) setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE) }
            .build()
    }
}
