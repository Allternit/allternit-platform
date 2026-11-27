package com.allternit.feature.push

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import com.allternit.core.auth.DeviceAuth
import java.net.URLEncoder
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import com.allternit.feature.chat.RuntimeCall

/**
 * Approve on an agent browser push: a see-through screen that asks for fingerprint, face or the screen lock first
 * (iOS: Face ID), then `POST /api/aci/handoff/:approval_id/approve`. Cancelling sends nothing. Deny never comes here.
 */
class AciApproveConfirmActivity : Activity() {
    private var started = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (started) return
        started = true
        val approvalId = intent.getStringExtra("data.approval_id").orEmpty()
        val tag = intent.getStringExtra(NotificationActionReceiver.EXTRA_TAG).orEmpty()
        val title = intent.getStringExtra(NotificationActionReceiver.EXTRA_TITLE).orEmpty()
        val data = PushNotifications.PASSED_KEYS.mapNotNull { k -> intent.getStringExtra("data.$k")?.let { k to it } }.toMap()
        if (approvalId.isEmpty()) return finish()
        val app = applicationContext
        DeviceAuth.confirm(this, title.ifBlank { "Approve for your bot" }, null) { ok ->
            if (!ok) { finish(); return@confirm }
            scope.launch {
                val path = "/api/aci/handoff/" + URLEncoder.encode(approvalId, "UTF-8").replace("+", "%20") + "/approve"
                val r = AciPushCalls.post(app, path, null, data["runtimeId"] ?: data["computer_id"])
                when {
                    r is RuntimeCall.Ok || (r is RuntimeCall.Http && (r.status == 404 || r.status == 409)) -> PushNotifications.cancel(app, tag)
                    r is RuntimeCall.Http && (r.status == 401 || r.status == 403) ->
                        PushNotifications.followUp(app, "Open Allternit to sign in, then try again.", title, data)
                    else -> PushNotifications.followUp(app, "That didn't go through. Open Allternit to approve it.", title, data)
                }
            }
            finish()
        }
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (DeviceAuth.onActivityResult(requestCode, resultCode)) return
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
    }

    private companion object {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    }
}
