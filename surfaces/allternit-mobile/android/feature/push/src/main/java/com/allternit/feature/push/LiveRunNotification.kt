package com.allternit.feature.push

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import com.allternit.core.design.R as DesignR
import com.allternit.feature.system.StatusSnapshot

/**
 * The ongoing notification for a running agent or task: the Android shape of the iOS Live Activity (`BotLiveActivity`,
 * `BotActivityAttributes.ContentState`). It shows the focus bot, what it is doing now, a progress bar when the runtime
 * knows the progress ("+N others" when more bots work), and Approve / Deny when the bot asks something that may be
 * answered inline (the same buttons and the same receiver as the push notifications). It follows the status snapshot, so
 * it starts, updates and ends with the same events the app already gets: every push and every app foreground.
 * Android has no Dynamic Island, and the lock-screen and Dynamic Island layouts of the Meeting and Loop activities have no
 * Android surface; this one notification stands in for them.
 */
object LiveRunNotification {
    const val CHANNEL_LIVE = "live"
    private const val TAG = "live-run"
    private const val ID = PushNotifications.NOTIFICATION_ID + 1

    fun update(context: Context, snap: StatusSnapshot?) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        val focus = snap?.focus
        val live = snap != null && !snap.paused && (focus != null || snap.activeBots > 0)
        if (!live || !AllternitPush.notificationsAllowed(context)) {
            nm.cancel(TAG, ID)
            return
        }
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL_LIVE, "Running now", NotificationManager.IMPORTANCE_LOW).apply {
                description = "A bot or task that is running, with its progress."
                setShowBadge(false)
            },
        )
        val title = focus?.botName?.takeIf { it.isNotBlank() } ?: "Your bots are working"
        val others = (snap!!.activeBots - 1).coerceAtLeast(0).takeIf { focus != null && it > 0 }?.let { "+$it others" }
        val step = when {
            focus == null -> "${snap.activeBots} working"
            focus.isPrivate -> "Working in private"
            else -> focus.step
        }
        val line = listOf(step, others).filter { !it.isNullOrBlank() }.joinToString(" · ").ifBlank { "Working" }
        val data = buildMap {
            focus?.botId?.takeIf { it.isNotBlank() }?.let { put("botId", it) }
            focus?.sessionId?.let { put("sessionId", it) }
            focus?.inboxId?.let { put("inboxId", it); put("event", "approval.requested") }
            focus?.contentHash?.let { put("contentHash", it) }
        }
        val builder = Notification.Builder(context, CHANNEL_LIVE)
            .setSmallIcon(com.allternit.core.design.R.drawable.ph_bell)
            .setContentTitle(title)
            .setContentText(line)
            .setCategory(Notification.CATEGORY_PROGRESS)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(false)
        val progress = focus?.progress
        if (progress != null) builder.setProgress(100, (progress.coerceIn(0.0, 1.0) * 100).toInt(), false)
        else builder.setProgress(0, 0, true)
        PushNotifications.openIntent(context, "$TAG-open", data)?.let { builder.setContentIntent(it) }
        val inline = focus != null && focus.approveInline && !focus.question.isNullOrBlank() && !focus.inboxId.isNullOrBlank() && !focus.contentHash.isNullOrBlank()
        if (inline) {
            builder.setContentText(focus!!.question)
            PushNotifications.actionsFor(context, PushNotifications.APPROVAL_INLINE, TAG, title, data)
                .filter { it.title.toString() != "Reply" }
                .forEach { builder.addAction(it) }
        }
        nm.notify(TAG, ID, builder.build())
    }
}
