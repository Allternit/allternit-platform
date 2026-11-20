package com.allternit.feature.push

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import com.allternit.core.design.R as DesignR

/**
 * The notifications a push becomes, the port of iOS `Core/NotificationActions` (categories and buttons) and the
 * APNs alert built in cloud-api `routes/apns.rs payload`.
 *
 * Channels group the iOS categories so each can be turned off in Android settings:
 * - `approvals`: APPROVAL_INLINE (Approve, Deny, Reply) and APPROVAL_REVIEW (Review, Deny, Reply). High.
 * - `messages`: BOT_REPLY (Reply, Open) and AGENT_MESSAGE (Reply, Open), plus plain bot messages. High.
 * - `runs`: RUN_DONE (Open) and NEEDS_FIX (Retry, Snooze). Default.
 * - `calls`: rings and missed calls. High.
 * - `general`: everything else (summaries, follow-ups). Default.
 *
 * Approve, Deny, Reply, Retry and Snooze run in [NotificationActionReceiver] without opening the app and need the
 * phone unlocked on Android 12+ (iOS `.authenticationRequired`). Open and Review open the app at the push's link.
 * Each notification is posted under its push `tag`, so a newer push with the same tag replaces it (APNs
 * collapse id) and a revoke (`notification.revoke`) removes it.
 */
object PushNotifications {
    const val CHANNEL_APPROVALS = "approvals"
    const val CHANNEL_MESSAGES = "messages"
    const val CHANNEL_RUNS = "runs"
    const val CHANNEL_CALLS = "calls"
    const val CHANNEL_GENERAL = "general"

    /** Category identifiers the cloud puts in `data.category` (iOS NotificationCategoryID). */
    const val APPROVAL_INLINE = "APPROVAL_INLINE"
    const val APPROVAL_REVIEW = "APPROVAL_REVIEW"
    const val BOT_REPLY = "BOT_REPLY"
    const val RUN_DONE = "RUN_DONE"
    const val NEEDS_FIX = "NEEDS_FIX"
    const val AGENT_MESSAGE = "AGENT_MESSAGE"

    /** Button identifiers (iOS NotificationActionID). */
    const val ACTION_APPROVE = "APPROVE"
    const val ACTION_DENY = "DENY"
    const val ACTION_REPLY = "REPLY"
    const val ACTION_REVIEW = "REVIEW"
    const val ACTION_OPEN = "OPEN"
    const val ACTION_RETRY = "RETRY"
    const val ACTION_SNOOZE = "SNOOZE"

    const val KEY_REPLY_TEXT = "allternit.reply"
    /** One id per tag: Android keys a notification by (tag, id). */
    const val NOTIFICATION_ID = 7100

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        fun channel(id: String, name: String, importance: Int, description: String) =
            NotificationChannel(id, name, importance).apply { this.description = description }
        nm.createNotificationChannels(
            listOf(
                channel(CHANNEL_APPROVALS, "Approvals", NotificationManager.IMPORTANCE_HIGH, "A bot is waiting for your OK."),
                channel(CHANNEL_MESSAGES, "Bot messages", NotificationManager.IMPORTANCE_HIGH, "Replies from your bots and agents."),
                channel(CHANNEL_RUNS, "Runs", NotificationManager.IMPORTANCE_DEFAULT, "A task finished or needs a fix."),
                channel(CHANNEL_CALLS, "Calls", NotificationManager.IMPORTANCE_HIGH, "Rings and missed calls."),
                channel(CHANNEL_GENERAL, "Other", NotificationManager.IMPORTANCE_DEFAULT, "Summaries and everything else."),
            ),
        )
    }

    /** The channel for a push's category and `type`. */
    fun channelFor(category: String?, type: String?): String = when {
        category == APPROVAL_INLINE || category == APPROVAL_REVIEW -> CHANNEL_APPROVALS
        category == BOT_REPLY || category == AGENT_MESSAGE -> CHANNEL_MESSAGES
        category == RUN_DONE || category == NEEDS_FIX -> CHANNEL_RUNS
        type == "call" || type == "missed_call" -> CHANNEL_CALLS
        type == "message" || type == "channel_message" -> CHANNEL_MESSAGES
        else -> CHANNEL_GENERAL
    }

    /** The bot behind a communication notification (iOS INSendMessageIntent sender, "<name> (assistant)"). */
    class Sender(val name: String, val avatar: android.graphics.Bitmap?)

    /**
     * Posts the notification for one FCM data message (keys from cloud-api routes/fcm.rs `message`). [sender] makes it
     * a conversation notification (MessagingStyle with the bot as the sender), [largeIcon] shows the bot's avatar.
     */
    fun show(context: Context, data: Map<String, String>, sender: Sender? = null, largeIcon: android.graphics.Bitmap? = null) {
        if (!AllternitPush.notificationsAllowed(context)) return
        ensureChannels(context)
        val tag = data["tag"]?.takeIf { it.isNotBlank() } ?: "push-${System.currentTimeMillis()}"
        val category = data["category"]
        val title = data["title"].orEmpty().ifBlank { "Allternit" }
        val body = data["body"].orEmpty()
        val builder = Notification.Builder(context, channelFor(category, data["type"]))
            .setSmallIcon(DesignR.drawable.vc_bell)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .setWhen(System.currentTimeMillis())
            .setShowWhen(true)
        largeIcon?.let { builder.setLargeIcon(it) }
        // A private push: the lock screen hides the body when the person has "hide sensitive content" on.
        if (data["private"] == "true") builder.setVisibility(Notification.VISIBILITY_PRIVATE)
        if (sender != null && Build.VERSION.SDK_INT >= 28) {
            val person = android.app.Person.Builder()
                .setName("${sender.name} (assistant)")
                .setKey("bot:" + (data["botId"]?.takeIf { it.isNotBlank() } ?: sender.name))
                .setBot(true)
                .apply { sender.avatar?.let { setIcon(Icon.createWithBitmap(it)) } }
                .build()
            val me = android.app.Person.Builder().setName("You").build()
            builder.setStyle(Notification.MessagingStyle(me).addMessage(body, System.currentTimeMillis(), person))
            builder.setCategory(Notification.CATEGORY_MESSAGE)
        }
        // One bot's notifications group together (APNs thread-id).
        data["botId"]?.takeIf { it.isNotBlank() }?.let { builder.setGroup("bot-$it") }
        if (category == APPROVAL_INLINE || category == APPROVAL_REVIEW) builder.setCategory(Notification.CATEGORY_REMINDER)
        if (data["type"] == "call") builder.setCategory(Notification.CATEGORY_CALL)
        openIntent(context, tag, data)?.let { builder.setContentIntent(it) }
        actionsFor(context, category, tag, title, data).forEach { builder.addAction(it) }
        context.getSystemService(NotificationManager::class.java)?.notify(tag, NOTIFICATION_ID, builder.build())
    }

    /** The iOS category's buttons, in iOS order. */
    internal fun actionsFor(context: Context, category: String?, tag: String, title: String, data: Map<String, String>): List<Notification.Action> {
        fun background(action: String, label: String, reply: String? = null): Notification.Action {
            val intent = Intent(context, NotificationActionReceiver::class.java).apply {
                this.action = "com.allternit.push.$action"
                putExtra(NotificationActionReceiver.EXTRA_ACTION, action)
                putExtra(NotificationActionReceiver.EXTRA_TAG, tag)
                putExtra(NotificationActionReceiver.EXTRA_TITLE, title)
                data.forEach { (k, v) -> if (k in PASSED_KEYS) putExtra("data.$k", v) }
            }
            // A reply needs a mutable PendingIntent (the system adds the typed text); the rest stay immutable.
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or (if (reply != null) PendingIntent.FLAG_MUTABLE else PendingIntent.FLAG_IMMUTABLE)
            val pending = PendingIntent.getBroadcast(context, (tag + action).hashCode(), intent, flags)
            val builder = Notification.Action.Builder(Icon.createWithResource(context, DesignR.drawable.vc_bell), label, pending)
            if (reply != null) builder.addRemoteInput(RemoteInput.Builder(KEY_REPLY_TEXT).setLabel(reply).build())
            if (Build.VERSION.SDK_INT >= 31) builder.setAuthenticationRequired(true)
            return builder.build()
        }
        fun foreground(action: String, label: String): Notification.Action? {
            val pending = openIntent(context, tag + action, data) ?: return null
            return Notification.Action.Builder(Icon.createWithResource(context, DesignR.drawable.vc_bell), label, pending).build()
        }
        return when (category) {
            APPROVAL_INLINE -> listOf(background(ACTION_APPROVE, "Approve"), background(ACTION_DENY, "Deny"), background(ACTION_REPLY, "Reply", "Message the bot"))
            APPROVAL_REVIEW -> listOfNotNull(foreground(ACTION_REVIEW, "Review"), background(ACTION_DENY, "Deny"), background(ACTION_REPLY, "Reply", "Message the bot"))
            BOT_REPLY -> listOfNotNull(background(ACTION_REPLY, "Reply", "Message the bot"), foreground(ACTION_OPEN, "Open"))
            AGENT_MESSAGE -> listOfNotNull(background(ACTION_REPLY, "Reply", "Reply to your agent"), foreground(ACTION_OPEN, "Open"))
            RUN_DONE -> listOfNotNull(foreground(ACTION_OPEN, "Open"))
            NEEDS_FIX -> listOf(background(ACTION_RETRY, "Retry"), background(ACTION_SNOOZE, "Snooze"))
            else -> emptyList()
        }
    }

    /** The data keys a button needs (iOS reads them from `userInfo["data"]`). */
    val PASSED_KEYS = setOf("inboxId", "contentHash", "runtimeId", "sessionId", "threadSessionId", "url", "event", "botId")

    internal fun openIntent(context: Context, key: String, data: Map<String, String>): PendingIntent? {
        val intent = AllternitPush.tapIntent(context, data) ?: return null
        return PendingIntent.getActivity(context, key.hashCode(), intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    /** Removes the notification posted under [tag]. */
    fun cancel(context: Context, tag: String) {
        context.getSystemService(NotificationManager::class.java)?.cancel(tag, NOTIFICATION_ID)
    }

    /** `notification.revoke`: removes delivered notifications whose tag is in [tags] (the item left "needs you"). */
    fun revoke(context: Context, tags: Set<String>) {
        if (tags.isEmpty()) return
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        nm.activeNotifications.filter { it.tag != null && it.tag in tags }.forEach { nm.cancel(it.tag, it.id) }
    }

    /** A follow-up line with sound that opens the item when tapped (iOS postFollowUp). */
    fun followUp(context: Context, line: String, title: String, data: Map<String, String>) {
        if (!AllternitPush.notificationsAllowed(context)) return
        val tag = "followup-" + (data["inboxId"]?.takeIf { it.isNotBlank() } ?: System.currentTimeMillis().toString())
        val builder = Notification.Builder(context, CHANNEL_GENERAL)
            .setSmallIcon(DesignR.drawable.vc_bell)
            .setContentTitle(title.ifBlank { "Allternit" })
            .setContentText(line)
            .setAutoCancel(true)
        openIntent(context, tag, data)?.let { builder.setContentIntent(it) }
        context.getSystemService(NotificationManager::class.java)?.notify(tag, NOTIFICATION_ID, builder.build())
    }
}
