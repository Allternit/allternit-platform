package com.allternit.ondevice

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * The "<model> is running on this phone" notification while a model is loaded, with Stop.
 *
 * No foreground service (0.7.2, 2026-11-18): the model lives in the app's process and runs
 * while the app is open, the way Google AI Edge Gallery and PocketPal work. When the person
 * switches apps, Android may pause or end the process; the model then loads again on the
 * next use. The engine also unloads after 5 minutes idle. Without a foreground service the
 * Play build needs no foreground-service declaration.
 */
object EngineService {
    const val NOTIFICATION_ID = 0x0D1CE
    private const val CHANNEL = "ondevice_running"

    /** Stop on the notification: unload the model and clear the notification. */
    class StopReceiver : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            OnDeviceEngine.get(context).requestUnload("user")
            stopRunning(context)
        }
    }

    fun showRunning(context: Context, name: String, quant: String, backend: String) {
        context.getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, notification(context, name, quant, backend))
    }

    fun stopRunning(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
    }

    private fun notification(context: Context, name: String, quant: String, backend: String): Notification {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, context.getString(R.string.ondevice_channel_name), NotificationManager.IMPORTANCE_LOW)
                .apply { description = context.getString(R.string.ondevice_channel_desc) },
        )
        val stop = PendingIntent.getBroadcast(
            context, 1, Intent(context, StopReceiver::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val where = context.getString(if (backend == "cpu") R.string.ondevice_cpu else R.string.ondevice_gpu)
        val open = context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
            PendingIntent.getActivity(context, 2, it, PendingIntent.FLAG_IMMUTABLE)
        }
        return Notification.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_ondevice_model)
            .setContentTitle(context.getString(R.string.ondevice_running_title, name, quant))
            .setContentText(context.getString(R.string.ondevice_running_text, where))
            .setStyle(Notification.BigTextStyle().bigText(context.getString(R.string.ondevice_running_text, where)))
            .setOngoing(true)
            .setShowWhen(false)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, context.getString(R.string.ondevice_stop), stop).build())
            .build()
    }
}
