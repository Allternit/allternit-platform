package com.allternit.phonelink

import com.allternit.app.R
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Build
import com.allternit.phonelink.core.PetOfferContract

object Notifications {
    const val CHANNEL_SERVICE = "phonelink_service"
    const val CHANNEL_PAIRING = "phonelink_pairing"
    const val CHANNEL_HOLD = "phonelink_hold"
    const val CHANNEL_PET_OFFER = "phonelink_pet_offer"
    const val ID_SERVICE = 1
    const val ID_HOLD = 2
    /** Pet-offer notifications use ids from here up (one per app, so a new offer replaces that app's old one). */
    private const val ID_PET_OFFER_BASE = 1000

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
        // High importance so the offer shows as a heads-up. The text is always the generic line.
        nm.createNotificationChannel(NotificationChannel(CHANNEL_PET_OFFER, "Error help", NotificationManager.IMPORTANCE_HIGH).apply {
            description = "Your bot offers to help when an app on this phone shows an error. Turn it on or off in Phone Link."
            lockscreenVisibility = Notification.VISIBILITY_PRIVATE
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

    // ── Pet offers ──────────────────────────────────────────────────────────

    fun petOfferId(muteEntry: String): Int = ID_PET_OFFER_BASE + (muteEntry.lowercase().hashCode() and 0x3fff)

    /**
     * True when this app may post notifications: on Android 13+ that needs the POST_NOTIFICATIONS
     * runtime grant, and the user can block the app or the "Error help" channel at any time.
     */
    fun canPostPetOffer(context: Context): Boolean {
        if (Build.VERSION.SDK_INT >= 33 &&
            context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return false
        val nm = context.getSystemService(NotificationManager::class.java)
        if (!nm.areNotificationsEnabled()) return false
        val ch = nm.getNotificationChannel(CHANNEL_PET_OFFER)
        return ch == null || ch.importance != NotificationManager.IMPORTANCE_NONE
    }

    /**
     * Heads-up offer: "Your bot spotted an error. Fix it?" with Fix it and Mute. The body is the
     * contract's generic line only: never the screen text and never the app's name.
     * [fixItUrl] carries the offer in its `#pet-offer=` fragment and opens the Allternit app (the TWA).
     * Returns false when notifications are off (nothing is posted, nothing crashes).
     */
    fun showPetOffer(context: Context, fixItUrl: String, muteEntry: String, timeoutMs: Long): Boolean {
        ensureChannels(context)
        if (!canPostPetOffer(context)) return false
        val id = petOfferId(muteEntry)
        // Straight to the activity (Android 12+ blocks starting an activity from a notification's receiver).
        val open = Intent(Intent.ACTION_VIEW, Uri.parse(fixItUrl))
            .setClassName(context, "com.google.androidbrowserhelper.trusted.LauncherActivity")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        val fixIt = PendingIntent.getActivity(context, id, open, immutable())
        val mute = PendingIntent.getBroadcast(context, id, PetOfferReceiver.muteIntent(context, muteEntry, id), immutable())
        val icon = Icon.createWithResource(context, R.drawable.ic_stat_link)
        val redacted = Notification.Builder(context, CHANNEL_PET_OFFER)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(PetOfferContract.NOTIFICATION_TITLE)
            .setContentText(PetOfferContract.NOTIFICATION_BODY)
            .build()
        val n = Notification.Builder(context, CHANNEL_PET_OFFER)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle(PetOfferContract.NOTIFICATION_TITLE)
            .setContentText(PetOfferContract.NOTIFICATION_BODY)
            .setCategory(Notification.CATEGORY_RECOMMENDATION)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(redacted)
            .setContentIntent(fixIt)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setTimeoutAfter(timeoutMs)
            .addAction(Notification.Action.Builder(icon, PetOfferContract.FIX_IT_LABEL, fixIt).build())
            .addAction(Notification.Action.Builder(icon, "Mute", mute).build())
            .build()
        return try {
            context.getSystemService(NotificationManager::class.java).notify(id, n)
            true
        } catch (e: SecurityException) {
            false
        }
    }

    fun cancelPetOffer(context: Context, id: Int) {
        context.getSystemService(NotificationManager::class.java).cancel(id)
    }
}
