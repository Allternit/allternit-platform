package com.allternit.app.spike.s1

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/** adb-only controls (guarded by android.permission.DUMP in the manifest). */
class S1CommandReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            "com.allternit.app.s1.REQUEST_CHANNEL" -> S1Bridge.requestChannel("adb")
            "com.allternit.app.s1.STATUS" -> Unit
            "com.allternit.app.s1.DAL_DROP" -> S1Service.setDropAssetLinks(intent.getBooleanExtra("on", true))
        }
        Log.i(S1Bridge.TAG, "STATUS ${S1Bridge.status()} server=${S1Service.serverStatus}")
    }
}
