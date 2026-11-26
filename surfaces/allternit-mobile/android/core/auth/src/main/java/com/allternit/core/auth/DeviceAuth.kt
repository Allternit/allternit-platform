package com.allternit.core.auth

import android.app.Activity
import android.app.KeyguardManager
import android.content.Intent
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.CancellationSignal

/**
 * Fingerprint, face or the screen lock before an agent approval is sent (iOS `ACIDeviceAuth`, LAPolicy
 * deviceOwnerAuthentication). A phone with no screen lock cannot prove anything, so it answers false and the
 * approval is not sent. [onResult] runs on the main thread.
 *
 * Android 11+: BiometricPrompt with BIOMETRIC_STRONG or DEVICE_CREDENTIAL. Android 10: device credential allowed.
 * Android 8-9: the screen-lock confirmation intent, answered through [DeviceAuth.onActivityResult].
 */
object DeviceAuth {
    const val REQUEST_CONFIRM = 7391

    private var pending: ((Boolean) -> Unit)? = null

    fun confirm(activity: Activity, title: String, subtitle: String?, onResult: (Boolean) -> Unit) {
        if (Build.VERSION.SDK_INT >= 28) {
            val b = BiometricPrompt.Builder(activity).setTitle(title)
            subtitle?.let { b.setSubtitle(it) }
            when {
                Build.VERSION.SDK_INT >= 30 ->
                    b.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL)
                Build.VERSION.SDK_INT == 29 -> @Suppress("DEPRECATION") b.setDeviceCredentialAllowed(true)
                else -> b.setNegativeButton("Cancel", activity.mainExecutor) { _, _ -> onResult(false) }
            }
            b.build().authenticate(CancellationSignal(), activity.mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult?) = onResult(true)
                override fun onAuthenticationError(errorCode: Int, errString: CharSequence?) = onResult(false)
            })
        } else {
            val km = activity.getSystemService(KeyguardManager::class.java)
            @Suppress("DEPRECATION")
            val i: Intent? = if (km?.isDeviceSecure == true) km.createConfirmDeviceCredentialIntent(title, subtitle) else null
            if (i == null) { onResult(false); return }
            pending = onResult
            @Suppress("DEPRECATION")
            activity.startActivityForResult(i, REQUEST_CONFIRM)
        }
    }

    /** Call from the host Activity's onActivityResult (Android 8-9 only). Returns true when it was ours. */
    fun onActivityResult(requestCode: Int, resultCode: Int): Boolean {
        if (requestCode != REQUEST_CONFIRM) return false
        val cb = pending
        pending = null
        cb?.invoke(resultCode == Activity.RESULT_OK)
        return true
    }
}
