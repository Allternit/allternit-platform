package com.allternit.feature.settings

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import com.allternit.core.design.AllternitTheme

/*
 * Privacy > "Lock with screen lock" (iOS AppLockStore: Face ID with the passcode as the fallback). Android asks for the
 * phone's own screen lock (fingerprint, face, PIN, pattern) through the system confirm-credential screen. Off by default.
 * The lock covers the app at launch and each time it comes back from the background.
 */

private fun keyguard(context: Context) = context.getSystemService(Context.KEYGUARD_SERVICE) as KeyguardManager

/** The Privacy row. Needs a screen lock on the phone: without one the row says so and stays off. */
@Composable
internal fun AppLockRow() {
    val context = LocalContext.current
    var on by remember { mutableStateOf(SettingsPrefs.appLock(context)) }
    val secure = keyguard(context).isDeviceSecure
    SCard {
        SToggleRow(
            "Lock with screen lock", "privacy_appLock", on && secure,
            detail = if (secure) "Ask for your fingerprint, face or phone passcode when you open Allternit." else "Set a screen lock in your phone's settings first.",
            enabled = secure, onChange = { on = it; SettingsPrefs.setAppLock(context, it) },
        )
    }
}

/** Covers everything while locked. Mount it last inside the app's root Box. */
@Composable
fun AppLockGate() {
    val context = LocalContext.current
    val enabledNow = SettingsPrefs.appLock(context) && keyguard(context).isDeviceSecure
    var locked by remember { mutableStateOf(enabledNow) }
    var failed by remember { mutableStateOf(false) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) { locked = false; failed = false } else failed = true
    }
    fun ask() {
        val intent = keyguard(context).createConfirmDeviceCredentialIntent("Unlock Allternit", null)
        if (intent == null) locked = false else launcher.launch(intent)
    }
    val activity = context as? ComponentActivity
    DisposableEffect(activity) {
        val observer = object : DefaultLifecycleObserver {
            override fun onStop(owner: LifecycleOwner) {
                if (SettingsPrefs.appLock(context) && keyguard(context).isDeviceSecure) locked = true
            }
            override fun onStart(owner: LifecycleOwner) {
                if (locked) ask()
            }
        }
        activity?.lifecycle?.addObserver(observer)
        if (locked) ask()
        onDispose { activity?.lifecycle?.removeObserver(observer) }
    }
    if (locked) {
        val c = AllternitTheme.colors
        Box(Modifier.fillMaxSize().background(c.bgPrimary).testTag("appLockCover"), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("Allternit is locked", color = c.textPrimary)
                if (failed) Text("That didn't work. Try again.", color = c.textSecondary, modifier = Modifier.padding(top = 6.dp))
                Text("Unlock", color = c.textPrimary, modifier = Modifier.padding(top = 16.dp).testTag("appLockUnlock").clickable { ask() }.padding(12.dp))
            }
        }
    }
}
