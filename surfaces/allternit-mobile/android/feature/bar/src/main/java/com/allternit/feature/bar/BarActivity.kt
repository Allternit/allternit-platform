package com.allternit.feature.bar

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme

/**
 * The island bar's Activity (lane 36): translucent, over the app or the home screen. Opened by "Do with an app" in the
 * composer's + sheet, by `allternit://bar?...` (MainActivity.readSystemLink forwards it here) and, in lane 37, by the
 * widget and the ongoing notification through [BarDeepLink.intent]. With `bar.island` off it closes at once.
 */
class BarActivity : ComponentActivity() {
    private val model get() = BarViewModel.shared

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!BarFlag.isOn(this)) {
            finish()
            return
        }
        enableEdgeToEdge()
        AllternitAuth.start(applicationContext)
        if (savedInstanceState == null) read(intent)
        setContent {
            AllternitTheme {
                BackHandler {
                    if (model.step == BarViewModel.Step.Tools || model.step == BarViewModel.Step.Blanks || model.step == BarViewModel.Step.Review) {
                        model.back()
                    } else {
                        close()
                    }
                }
                BarScreen(model, onClose = ::close)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        read(intent)
    }

    private fun read(intent: Intent?) {
        val request = BarDeepLink.parse(intent?.data)
        if (request != null && (request.app != null || request.bot != null || request.values.isNotEmpty())) model.open(request)
    }

    private fun close() {
        model.close()
        finish()
        @Suppress("DEPRECATION")
        overridePendingTransition(0, android.R.anim.fade_out)
    }
}
