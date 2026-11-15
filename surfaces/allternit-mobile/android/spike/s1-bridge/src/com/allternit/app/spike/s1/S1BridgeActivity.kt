package com.allternit.app.spike.s1

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.util.Log

/**
 * SPIKE S1 launcher (debug builds only, icon "S1 bridge").
 *
 * Mode A (default): starts [S1Service] (serves the test page on
 *   http://localhost:8765 from the APK's assets, fully offline) and opens
 *   http://localhost:8765/s1.html as a TWA. The channel is requested on every
 *   NAVIGATION_FINISHED, so the page drives the whole test, reload included.
 *
 * Mode B: `--es url https://m.allternit.com/ --ez auto false`. Opens the real
 *   origin (its service-worker-cached shell when offline). The client is pasted
 *   into the page via chrome://inspect, then the channel is requested with the
 *   adb broadcast com.allternit.app.s1.REQUEST_CHANNEL.
 */
class S1BridgeActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val url = Uri.parse(
            intent.getStringExtra("url")
                ?: "${S1Service.PAGE_URL}?launch=${android.os.SystemClock.elapsedRealtime()}",
        )
        val auto = intent.getBooleanExtra("auto", true)
        Log.i(S1Bridge.TAG, "S1BridgeActivity url=$url auto=$auto")
        startForegroundService(Intent(this, S1Service::class.java))
        S1Bridge.launch(this, url, auto)
        finish()
    }
}
