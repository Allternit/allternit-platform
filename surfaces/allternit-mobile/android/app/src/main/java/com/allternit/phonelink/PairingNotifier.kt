package com.allternit.phonelink

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import com.allternit.phonelink.core.PairApprover
import java.util.concurrent.ConcurrentHashMap

/**
 * Pairing approval when no one-shot grant is armed: a heads-up notification
 * with Allow / Deny (Allow needs the phone unlocked on Android 12+), and the
 * same request listed in the app. The 60 s timeout lives in AuthManager.
 */
class PairingNotifier(private val context: Context) : PairApprover {
    class Pending(val requestId: String, val clientName: String, val fingerprint: String?, val local: Boolean, val answer: (Boolean) -> Unit)

    private val pending = ConcurrentHashMap<String, Pending>()

    val pendingRequests: List<Pending> get() = pending.values.toList()

    override fun requestApproval(requestId: String, clientName: String, fingerprint: String?, local: Boolean, answer: (Boolean) -> Unit) {
        pending[requestId] = Pending(requestId, clientName, fingerprint, local, answer)
        post(requestId, clientName, fingerprint, local)
        ServerController.notifyChanged()
    }

    override fun expire(requestId: String) {
        pending.remove(requestId)
        cancel(requestId)
        ServerController.notifyChanged()
    }

    fun answer(requestId: String, allow: Boolean) {
        pending.remove(requestId)?.answer?.invoke(allow)
        cancel(requestId)
        ServerController.notifyChanged()
    }

    private fun notificationId(requestId: String) = 1000 + (requestId.hashCode() and 0x7fff)

    private fun cancel(requestId: String) {
        context.getSystemService(NotificationManager::class.java).cancel(notificationId(requestId))
    }

    private fun action(requestId: String, allow: Boolean): PendingIntent {
        val i = Intent(context, PairingActionReceiver::class.java)
            .putExtra(PairingActionReceiver.EXTRA_REQUEST, requestId)
            .putExtra(PairingActionReceiver.EXTRA_ALLOW, allow)
        return PendingIntent.getBroadcast(context, notificationId(requestId) * 2 + if (allow) 1 else 0, i,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private fun post(requestId: String, clientName: String, fingerprint: String?, local: Boolean) {
        Notifications.ensureChannels(context)
        val icon = Icon.createWithResource(context, R.drawable.ic_stat_link)
        val allow = Notification.Action.Builder(icon, "Allow", action(requestId, true))
            .apply { if (Build.VERSION.SDK_INT >= 31) setAuthenticationRequired(true) }
            .build()
        val deny = Notification.Action.Builder(icon, "Deny", action(requestId, false)).build()
        val via = if (local) "over the USB cable / this phone" else "over the network"
        val fp = fingerprint?.let { " Key fingerprint $it — check it matches the computer." } ?: ""
        val n = Notification.Builder(context, Notifications.CHANNEL_PAIRING)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle("Pair \"$clientName\" with this phone?")
            .setStyle(Notification.BigTextStyle().bigText("It asks $via to use the apps on this phone.$fp Ignore it if you didn't start this."))
            .setContentText("It asks $via to use the apps on this phone.")
            .setTimeoutAfter(60_000)
            .setAutoCancel(true)
            .setContentIntent(Notifications.openAppIntent(context))
            .addAction(allow)
            .addAction(deny)
            .build()
        try {
            context.getSystemService(NotificationManager::class.java).notify(notificationId(requestId), n)
        } catch (e: SecurityException) {
            // POST_NOTIFICATIONS not granted: the request is still answerable in the app.
        }
    }
}

class PairingActionReceiver : BroadcastReceiver() {
    companion object {
        const val EXTRA_REQUEST = "request_id"
        const val EXTRA_ALLOW = "allow"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_REQUEST) ?: return
        ServerController.init(context)
        ServerController.pairing.answer(id, intent.getBooleanExtra(EXTRA_ALLOW, false))
    }
}
