package com.allternit.app.spike.s1

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
 * SPIKE S1 foreground service: runs the loopback HTTP server ([S1HttpServer]).
 * It serves the offline test page (mode A) and the candidate-2 endpoints.
 * W9's llama.cpp engine would live in a foreground service like this one.
 */
class S1Service : Service() {
    private var server: S1HttpServer? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "S1 bridge spike", NotificationManager.IMPORTANCE_LOW),
        )
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("S1 bridge spike")
            .setContentText("Serving http://localhost:$PORT")
            .build()
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(1801, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        } else {
            startForeground(1801, n)
        }
        instance = this
        if (server == null) {
            server = S1HttpServer(assets, PORT, assetLinks()).also {
                serverStatus = try {
                    it.start(); "listening on ${it.boundAddresses()}"
                } catch (e: Exception) {
                    Log.e(S1Bridge.TAG, "server failed", e); "FAILED: $e"
                }
                Log.i(S1Bridge.TAG, "server $serverStatus")
            }
        }
        return START_STICKY
    }

    /**
     * The same statement m.allternit.com would need for W9, built for whatever
     * package and certificate this APK has (debug: com.allternit.app.s1 + the
     * debug keystore). use_as_origin is the relation the postMessage channel needs.
     */
    private fun assetLinks(): String {
        val info = packageManager.getPackageInfo(packageName, PackageManager.GET_SIGNING_CERTIFICATES)
        val certs = info.signingInfo?.apkContentsSigners.orEmpty()
        val fps = certs.joinToString(",") { sig ->
            val d = MessageDigest.getInstance("SHA-256").digest(sig.toByteArray())
            "\"" + d.joinToString(":") { "%02X".format(it) } + "\""
        }
        val json = """[{"relation":["delegate_permission/common.handle_all_urls",""" +
            """"delegate_permission/common.use_as_origin"],"target":{"namespace":"android_app",""" +
            """"package_name":"$packageName","sha256_cert_fingerprints":[$fps]}}]"""
        Log.i(S1Bridge.TAG, "assetlinks $json")
        return json
    }

    override fun onDestroy() {
        instance = null
        server?.stop()
        server = null
        serverStatus = "stopped"
        super.onDestroy()
    }

    companion object {
        const val PORT = 8765
        const val PAGE_URL = "http://localhost:$PORT/s1.html"
        private const val CHANNEL = "s1_spike"
        @Volatile var serverStatus = "not started"
        @Volatile var instance: S1Service? = null

        fun setDropAssetLinks(on: Boolean) {
            instance?.server?.dropAssetLinks = on
        }
    }
}
