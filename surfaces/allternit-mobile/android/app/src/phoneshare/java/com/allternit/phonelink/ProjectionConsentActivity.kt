package com.allternit.phonelink

import android.app.Activity
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Bundle

/**
 * Asks Android's own screen-share prompt (MediaProjection consent). Opened by
 * the person: the Share screen button, or a tap on the "Someone wants to see
 * your screen" notification. Only a "Start" there starts ProjectionService;
 * the consent result is handed straight to it (Android 14+: one use per token).
 */
class ProjectionConsentActivity : Activity() {
    companion object { private const val REQUEST_CAPTURE = 7101 }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        AppLink.init(this)
        ShareNotifications.cancelRequest(this)
        if (AppLink.session.isOn) { finish(); return }
        if (!AppLink.isPaired) {
            startActivity(Intent(this, ScreenShareActivity::class.java))
            finish(); return
        }
        if (savedInstanceState != null) return // the prompt is already up (recreated)
        AppLink.consentRequested()
        val mpm = getSystemService(MediaProjectionManager::class.java)
        try {
            startActivityForResult(mpm.createScreenCaptureIntent(), REQUEST_CAPTURE)
        } catch (e: Exception) {
            AppLink.consentDenied()
            finish()
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != REQUEST_CAPTURE) return
        if (resultCode == RESULT_OK && data != null) {
            ProjectionService.start(this, resultCode, data)
        } else {
            AppLink.consentDenied()
        }
        finish()
    }
}
