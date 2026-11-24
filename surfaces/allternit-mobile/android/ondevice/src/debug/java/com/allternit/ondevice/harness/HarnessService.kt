package com.allternit.ondevice.harness

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import java.security.MessageDigest

/**
 * DEBUG ONLY. Foreground service that runs [HarnessServer] on http://localhost:8765. It also
 * serves /.well-known/assetlinks.json for this APK (package + signing cert) with
 * handle_all_urls and use_as_origin, which is what m.allternit.com's file must carry for the
 * release app. Chrome fetches it from the origin itself (spike S1, mode A).
 */
class HarnessService : Service() {
    private var server: HarnessServer? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "On-device test page", NotificationManager.IMPORTANCE_MIN))
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("On-device test page (debug)")
            .setContentText("Serving http://localhost:$PORT")
            .build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(1802, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(1802, n)
        if (server == null) {
            server = HarnessServer(assets, PORT, assetLinks()).also {
                try { it.start() } catch (e: Exception) { Log.e(TAG, "server failed", e) }
            }
        }
        return START_NOT_STICKY
    }

    private fun assetLinks(): String {
        val info = packageManager.getPackageInfo(packageName, PackageManager.GET_SIGNING_CERTIFICATES)
        val fps = info.signingInfo?.apkContentsSigners.orEmpty().joinToString(",") { sig ->
            "\"" + MessageDigest.getInstance("SHA-256").digest(sig.toByteArray()).joinToString(":") { "%02X".format(it) } + "\""
        }
        return """[{"relation":["delegate_permission/common.handle_all_urls",""" +
            """"delegate_permission/common.use_as_origin"],"target":{"namespace":"android_app",""" +
            """"package_name":"$packageName","sha256_cert_fingerprints":[$fps]}}]"""
    }

    override fun onDestroy() {
        server?.stop()
        server = null
        super.onDestroy()
    }

    companion object {
        const val TAG = "OnDeviceHarness"
        const val PORT = 8765
        const val PAGE_URL = "http://localhost:$PORT/index.html"
        private const val CHANNEL = "ondevice_harness"
    }
}
