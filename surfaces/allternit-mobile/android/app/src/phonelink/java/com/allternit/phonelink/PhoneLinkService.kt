package com.allternit.phonelink

import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Foreground service: keeps the process at foreground priority and shows the
 * persistent "Allternit can use your phone's apps · Stop" notification while
 * the server runs. Exported only behind android.permission.DUMP so
 * `adb shell am start-foreground-service` works and other apps cannot start it.
 */
class PhoneLinkService : Service() {
    companion object {
        fun start(context: Context) {
            try {
                context.startForegroundService(Intent(context, PhoneLinkService::class.java))
            } catch (e: Exception) {
                // ForegroundServiceStartNotAllowedException (API 31+) and friends: the server
                // still runs in-process (held up by the accessibility service), just without
                // the notification until the app is opened.
                ServerController.log("foreground service start refused: ${e.javaClass.simpleName}: ${e.message}")
            }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, PhoneLinkService::class.java))
        }

        @Volatile private var running = false

        fun refreshNotification(context: Context) {
            if (!running) return
            try {
                context.getSystemService(NotificationManager::class.java)
                    .notify(Notifications.ID_SERVICE, Notifications.serviceNotification(context, ServerController.statusLine()))
            } catch (_: SecurityException) {}
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Notifications.ensureChannels(this)
        ServerController.init(this)
        val n = Notifications.serviceNotification(this, ServerController.statusLine())
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(Notifications.ID_SERVICE, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(Notifications.ID_SERVICE, n)
        }
        running = true
        if (ServerController.settings.userStopped) {
            // Started after the user pressed Stop (e.g. a stale intent): honour Stop.
            stopSelf()
            return START_NOT_STICKY
        }
        ServerController.startServer()
        refreshNotification(this)
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }
}
