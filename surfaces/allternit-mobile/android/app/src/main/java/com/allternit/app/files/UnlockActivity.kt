package com.allternit.app.files

import android.app.Activity
import android.app.KeyguardManager
import android.content.Intent
import android.hardware.biometrics.BiometricManager
import android.hardware.biometrics.BiometricPrompt
import android.os.Build
import android.os.Bundle
import android.os.CancellationSignal
import android.widget.Toast

/**
 * Unlocks a `protected` root (Memory Drive) with a fresh device unlock (SPEC section 12).
 * Android 9+: BiometricPrompt (fingerprint/face, or the screen lock as fallback on 10+).
 * Android 8: the screen-lock confirmation. Success mints a signed five-minute unlock token
 * from cloud-api; requests then carry it as `x-allternit-unlock`. Started from the "Unlock" button
 * DocumentsUI shows for the locked folder (AuthenticationRequiredException).
 */
class UnlockActivity : Activity() {
    private val drive by lazy { FilesDrive.get(this) }
    private var cid: String = ""
    private var rid: String = ""
    private val signal = CancellationSignal()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        cid = intent.getStringExtra(EXTRA_COMPUTER) ?: return finish()
        rid = intent.getStringExtra(EXTRA_ROOT) ?: ""
        val name = intent.getStringExtra(EXTRA_NAME) ?: "This folder"
        val title = "Unlock $name"
        val subtitle = drive.computerName(cid)
        if (Build.VERSION.SDK_INT >= 28) {
            val b = BiometricPrompt.Builder(this).setTitle(title).setSubtitle(subtitle)
                .setDescription("$name is protected. Unlock to open it on this phone.")
            when {
                Build.VERSION.SDK_INT >= 30 ->
                    b.setAllowedAuthenticators(BiometricManager.Authenticators.BIOMETRIC_STRONG or BiometricManager.Authenticators.DEVICE_CREDENTIAL)
                Build.VERSION.SDK_INT == 29 -> b.setDeviceCredentialAllowed(true) // deprecated in 30, the API on 29
                else -> b.setNegativeButton("Cancel", mainExecutor) { _, _ -> finish() }
            }
            b.build().authenticate(signal, mainExecutor, object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult?) = unlocked()

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence?) {
                    if (errorCode != BiometricPrompt.BIOMETRIC_ERROR_USER_CANCELED && errorCode != BiometricPrompt.BIOMETRIC_ERROR_CANCELED) {
                        Toast.makeText(this@UnlockActivity, errString ?: "Couldn't unlock", Toast.LENGTH_LONG).show()
                    }
                    finish()
                }
            })
        } else {
            val km = getSystemService(KeyguardManager::class.java)
            val i = km.createConfirmDeviceCredentialIntent(title, subtitle)
            if (i == null) {
                Toast.makeText(this, "Set a screen lock on this phone to open protected folders.", Toast.LENGTH_LONG).show()
                finish()
            } else {
                startActivityForResult(i, REQ_CONFIRM)
            }
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQ_CONFIRM && resultCode == RESULT_OK) unlocked() else finish()
    }

    private fun unlocked() {
        // Minting the token is a network call: off the main thread.
        drive.io.execute {
            val ok = try {
                drive.markUnlocked(cid)
                drive.notifyChildren(DocIds.computer(cid))
                drive.root(cid, rid)?.let { drive.notifyChildren(DocIds.item(cid, rid, it.rootItemId)) }
                true
            } catch (e: Exception) {
                false
            }
            runOnUiThread {
                if (ok) {
                    setResult(RESULT_OK)
                } else {
                    Toast.makeText(this, "Couldn't unlock. Check your connection and try again.", Toast.LENGTH_LONG).show()
                }
                finish()
            }
        }
    }

    override fun onDestroy() {
        signal.cancel()
        super.onDestroy()
    }

    companion object {
        const val EXTRA_COMPUTER = "computer_id"
        const val EXTRA_ROOT = "root_id"
        const val EXTRA_NAME = "root_name"
        private const val REQ_CONFIRM = 7
    }
}
