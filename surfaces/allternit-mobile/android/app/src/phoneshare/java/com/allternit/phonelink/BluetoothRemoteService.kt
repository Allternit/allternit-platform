package com.allternit.phonelink

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.IBinder
import android.util.Log
import com.allternit.app.R
import com.allternit.phonelink.core.HidAvailability

/**
 * Keeps "Control my iPhone by Bluetooth" on while the app is in the background
 * (the person is using the iPhone, not this phone). Foreground service of type
 * connectedDevice with a visible notification and a Turn off action. While it
 * runs the HID device registration (BluetoothRemote) and the app-phone control
 * socket stay up, so Desktop's `hid.*` commands reach the iPhone. Started only
 * from the person's tap in Share screen; never on boot.
 */
class BluetoothRemoteService : Service() {
    companion object {
        private const val TAG = "BluetoothRemoteSvc"
        private const val ACTION_START = "com.allternit.phonelink.action.BT_REMOTE_START"
        private const val ACTION_STOP = "com.allternit.phonelink.action.BT_REMOTE_STOP"
        const val CHANNEL = "bluetooth_remote"
        const val NOTIFICATION_ID = 3103

        @Volatile var running = false; private set

        /** The person tapped Turn on (permissions already granted). */
        fun start(context: Context) {
            val i = Intent(context, BluetoothRemoteService::class.java).setAction(ACTION_START)
            try { context.startForegroundService(i) } catch (e: Exception) {
                Log.w(TAG, "could not start: ${e.javaClass.simpleName}: ${e.message}")
            }
        }

        fun stop(context: Context) {
            BluetoothRemote.stop()
            context.stopService(Intent(context, BluetoothRemoteService::class.java))
        }

        private fun stopIntent(context: Context) = Intent(context, BluetoothRemoteService::class.java).setAction(ACTION_STOP)
    }

    private val update: () -> Unit = { refreshNotification() }

    /** Bluetooth turned back on while the feature is on: register again. */
    private val adapterState = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.getIntExtra(android.bluetooth.BluetoothAdapter.EXTRA_STATE, -1) == android.bluetooth.BluetoothAdapter.STATE_ON) {
                BluetoothRemote.start(this@BluetoothRemoteService)
            }
            refreshNotification()
        }
    }
    private var receiverRegistered = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            BluetoothRemote.stop()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        // Android requires startForeground soon after startForegroundService, whatever happens next.
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIFICATION_ID, notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            } else {
                startForeground(NOTIFICATION_ID, notification())
            }
        } catch (e: Exception) {
            // Android 14+: refused when the Nearby devices permission is missing.
            Log.w(TAG, "startForeground refused: ${e.javaClass.simpleName}: ${e.message}")
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        AppLink.init(this)
        AppLink.ensureControl()
        BluetoothRemote.init(this)
        BluetoothRemote.addListener(update)
        if (!receiverRegistered) {
            val filter = IntentFilter(android.bluetooth.BluetoothAdapter.ACTION_STATE_CHANGED)
            if (Build.VERSION.SDK_INT >= 33) registerReceiver(adapterState, filter, RECEIVER_NOT_EXPORTED) else registerReceiver(adapterState, filter)
            receiverRegistered = true
        }
        BluetoothRemote.start(this)
        return START_NOT_STICKY
    }

    private fun refreshNotification() {
        try { getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, notification()) } catch (_: SecurityException) {}
    }

    private fun notification(): Notification {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Control my iPhone", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Shown while this phone can act as a Bluetooth keyboard and mouse for your iPhone."
            setShowBadge(false)
        })
        val s = BluetoothRemote.state()
        val text = when {
            s.connected -> "Connected to ${s.name ?: "your iPhone"}."
            s.availability == HidAvailability.UNSUPPORTED || s.availability == HidAvailability.BLUETOOTH_OFF ||
                s.availability == HidAvailability.NO_PERMISSION -> s.unavailableReason() ?: ""
            else -> "Waiting for your iPhone to connect."
        }
        val open = PendingIntent.getActivity(
            this, 4, Intent(this, ScreenShareActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(this, 5, stopIntent(this), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_link)
            .setContentTitle("Bluetooth keyboard and mouse is on")
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(Icon.createWithResource(this, R.drawable.ic_stat_link), "Turn off", stop).build())
            .apply { if (Build.VERSION.SDK_INT >= 31) setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE) }
            .build()
    }

    override fun onDestroy() {
        running = false
        BluetoothRemote.removeListener(update)
        BluetoothRemote.stop()
        if (receiverRegistered) { try { unregisterReceiver(adapterState) } catch (_: Exception) {}; receiverRegistered = false }
        super.onDestroy()
    }
}
