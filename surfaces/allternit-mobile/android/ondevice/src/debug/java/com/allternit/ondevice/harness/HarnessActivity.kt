package com.allternit.ondevice.harness

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.os.SystemClock
import android.util.Log

/**
 * DEBUG ONLY. Starts [HarnessService] (the localhost test page server) and opens the page
 * through the app's real launcher, so the run covers the production launch path:
 * ReviewLauncherActivity -> BridgedLauncherActivity -> TwaBridge session -> postMessage.
 * `--es url <url>` opens another URL instead (e.g. a PWA build served over `adb reverse`).
 */
class HarnessActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        startForegroundService(Intent(this, HarnessService::class.java))
        val url = intent.getStringExtra("url")
            ?: "${HarnessService.PAGE_URL}?launch=${SystemClock.elapsedRealtime()}"
        val launcher = packageManager.getLaunchIntentForPackage(packageName)
            ?.component ?: run { finish(); return }
        Log.i("OnDeviceHarness", "opening $url via ${launcher.className}")
        startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(url))
                .setComponent(launcher)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        finish()
    }
}
