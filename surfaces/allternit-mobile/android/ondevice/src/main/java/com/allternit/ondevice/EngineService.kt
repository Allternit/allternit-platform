package com.allternit.ondevice

import android.app.ForegroundServiceStartNotAllowedException
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log

/**
 * Foreground service while a model is loaded: keeps the process (and the loaded model)
 * alive when the user switches apps, and shows "<model> is running on this phone" with a
 * Stop action. Type `specialUse` (subtype in the manifest): a model held in memory for the
 * user's chat fits none of the standard types; downloads run in the system DownloadManager,
 * not here.
 *
 * Starting a foreground service from the background is restricted on Android 12+. The load
 * comes from the page while the TWA is in front, so the app's process is bound by Chrome;
 * if the system still refuses, the same notification is posted without the service and
 * the engine keeps running for as long as Chrome holds the channel.
 */
class EngineService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                OnDeviceEngine.get(this).requestUnload("user")
                stopSelfCompat()
                return START_NOT_STICKY
            }
            ACTION_SHOW -> {
                val n = notification(this, intent.getStringExtra(EXTRA_TITLE).orEmpty(),
                    intent.getStringExtra(EXTRA_QUANT).orEmpty(), intent.getStringExtra(EXTRA_BACKEND).orEmpty())
                if (Build.VERSION.SDK_INT >= 34) {
                    startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
                } else {
                    startForeground(NOTIFICATION_ID, n)
                }
                return START_NOT_STICKY
            }
            else -> {
                // Restarted by the system without our intent: no model survives a process restart.
                stopSelfCompat()
                return START_NOT_STICKY
            }
        }
    }

    private fun stopSelfCompat() {
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    companion object {
        private const val TAG = "EngineService"
        const val NOTIFICATION_ID = 0x0D1CE
        private const val CHANNEL = "ondevice_running"
        private const val ACTION_SHOW = "com.allternit.ondevice.SHOW"
        private const val ACTION_STOP = "com.allternit.ondevice.STOP"
        private const val EXTRA_TITLE = "title"
        private const val EXTRA_QUANT = "quant"
        private const val EXTRA_BACKEND = "backend"

        fun showRunning(context: Context, name: String, quant: String, backend: String) {
            val i = Intent(context, EngineService::class.java).setAction(ACTION_SHOW)
                .putExtra(EXTRA_TITLE, name).putExtra(EXTRA_QUANT, quant).putExtra(EXTRA_BACKEND, backend)
            try {
                context.startForegroundService(i)
            } catch (e: Exception) {
                val refused = Build.VERSION.SDK_INT >= 31 && e is ForegroundServiceStartNotAllowedException
                Log.w(TAG, "foreground service refused (${if (refused) "background start" else e}); posting the notification only")
                context.getSystemService(NotificationManager::class.java)
                    .notify(NOTIFICATION_ID, notification(context, name, quant, backend))
            }
        }

        fun stopRunning(context: Context) {
            context.stopService(Intent(context, EngineService::class.java))
            context.getSystemService(NotificationManager::class.java).cancel(NOTIFICATION_ID)
        }

        private fun notification(context: Context, name: String, quant: String, backend: String): Notification {
            val nm = context.getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, context.getString(R.string.ondevice_channel_name), NotificationManager.IMPORTANCE_LOW)
                    .apply { description = context.getString(R.string.ondevice_channel_desc) },
            )
            val stop = PendingIntent.getService(
                context, 1, Intent(context, EngineService::class.java).setAction(ACTION_STOP),
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
                .apply { if (Build.VERSION.SDK_INT >= 31) setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE) }
                .build()
        }
    }
}
