package com.allternit.phonelink

import android.app.NotificationManager
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.IBinder
import android.util.Log

/**
 * Foreground service of type mediaProjection that holds one screen share
 * (ProjectionEncoder). Started only by ProjectionConsentActivity with the
 * consent result, after the person tapped Start on Android's prompt; never
 * restarted by the system (the token is single use, so START_NOT_STICKY).
 * Stops on the notification's Stop, when Android ends the projection (the
 * status-bar chip, revoke, lock on Android 15+), after 10 unwatched minutes,
 * at shutdown, or when the phone is unpaired. Each sends ENDED first where it can.
 */
class ProjectionService : Service() {
    companion object {
        private const val TAG = "Projection"
        private const val ACTION_START = "com.allternit.phonelink.action.SHARE_START"
        private const val ACTION_STOP = "com.allternit.phonelink.action.SHARE_STOP"
        private const val EXTRA_CODE = "result_code"
        private const val EXTRA_DATA = "result_data"

        @Volatile private var current: ProjectionService? = null

        fun start(context: Context, resultCode: Int, data: Intent) {
            val i = Intent(context, ProjectionService::class.java)
                .setAction(ACTION_START)
                .putExtra(EXTRA_CODE, resultCode)
                .putExtra(EXTRA_DATA, data)
            try {
                context.startForegroundService(i)
            } catch (e: Exception) {
                Log.w(TAG, "could not start the share: ${e.javaClass.simpleName}: ${e.message}")
                AppLink.consentDenied()
            }
        }

        fun stopIntent(context: Context): Intent =
            Intent(context, ProjectionService::class.java).setAction(ACTION_STOP)

        /** Ends a running share (no-op when none runs). */
        fun stopShare(context: Context, reason: String) {
            current?.end(reason) ?: run { if (AppLink.session.isOn) AppLink.session.stop(reason) }
        }
    }

    private var encoder: ProjectionEncoder? = null

    private val shutdown = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) { end("shutdown") }
    }
    private var shutdownRegistered = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        AppLink.init(this)
        val req = intent ?: run { if (encoder == null) stopSelf(); return START_NOT_STICKY }
        // Stop arrives through startService (the notification's PendingIntent): no startForeground for it.
        if (req.action == ACTION_STOP) { end("stopped"); return START_NOT_STICKY }
        if (req.action != ACTION_START) { if (encoder == null) stopSelf(); return START_NOT_STICKY }
        // Android requires startForeground soon after startForegroundService, whatever happens next.
        val n = ShareNotifications.sharing(this, live = false)
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(ShareNotifications.ID_SHARING, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
            } else {
                startForeground(ShareNotifications.ID_SHARING, n)
            }
        } catch (e: Exception) {
            Log.w(TAG, "startForeground refused: ${e.javaClass.simpleName}: ${e.message}")
            AppLink.consentDenied()
            stopSelf()
            return START_NOT_STICKY
        }
        if (encoder != null) return START_NOT_STICKY // already sharing; a second token is dropped
        val code = req.getIntExtra(EXTRA_CODE, 0)
        val data: Intent? = if (Build.VERSION.SDK_INT >= 33) {
            req.getParcelableExtra(EXTRA_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION") req.getParcelableExtra<Intent>(EXTRA_DATA)
        }
        val projection = try {
            data?.let { getSystemService(MediaProjectionManager::class.java).getMediaProjection(code, it) }
        } catch (e: Exception) {
            Log.w(TAG, "getMediaProjection failed: ${e.javaClass.simpleName}: ${e.message}")
            null
        }
        if (projection == null) {
            AppLink.consentDenied()
            stopSelf()
            return START_NOT_STICKY
        }
        current = this
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(shutdown, IntentFilter(Intent.ACTION_SHUTDOWN), RECEIVER_NOT_EXPORTED)
        } else {
            registerReceiver(shutdown, IntentFilter(Intent.ACTION_SHUTDOWN))
        }
        shutdownRegistered = true
        encoder = ProjectionEncoder(
            context = this,
            projection = projection,
            session = AppLink.session,
            ingestOpener = { l -> AppLink.link.openIngest(l) },
            onChanged = { live -> onShareChanged(live) },
            onEnded = { reason -> onEncoderEnded(reason) },
        ).also { it.start() }
        return START_NOT_STICKY
    }

    private fun onShareChanged(live: Boolean) {
        try {
            getSystemService(NotificationManager::class.java).notify(ShareNotifications.ID_SHARING, ShareNotifications.sharing(this, live))
        } catch (_: SecurityException) {}
        AppLink.shareStateChanged()
    }

    fun end(reason: String) {
        val e = encoder
        if (e == null) { stopSelf(); return }
        e.end(reason)
    }

    private fun onEncoderEnded(reason: String) {
        Log.i(TAG, "share ended: $reason")
        encoder = null
        if (current === this) current = null
        AppLink.shareStateChanged()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        encoder?.end("stopped")
        encoder = null
        if (current === this) current = null
        if (shutdownRegistered) { try { unregisterReceiver(shutdown) } catch (_: Exception) {} ; shutdownRegistered = false }
        super.onDestroy()
    }
}
