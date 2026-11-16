package com.allternit.app.files

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.provider.DocumentsContract
import android.widget.LinearLayout
import android.widget.TextView
import androidx.browser.customtabs.CustomTabsIntent
import kotlin.concurrent.thread

/**
 * Sign-in for the Files drive (spike 8, production form; see [FilesAuth]):
 * Custom Tab to m.allternit.com -> https App Link back here -> Keystore-sealed token.
 * Reached from the "Sign in" button of the signed-out root in the system picker, and
 * from the share screen. Also shows the signed-in state with Sign out.
 */
class FilesSignInActivity : Activity() {
    private lateinit var status: TextView
    private lateinit var actions: LinearLayout
    private val drive by lazy { FilesDrive.get(this) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val col = FilesUi.column(this, padV = 48)
        col.addView(FilesUi.title(this, "Allternit Files"))
        status = FilesUi.body(this, "")
        actions = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(status)
        col.addView(actions)
        setContentView(col)
        if (!handle(intent)) render(null)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handle(intent)
    }

    /** The App Link callback. True when this intent was one. */
    private fun handle(i: Intent?): Boolean {
        val uri = i?.data
        if (!drive.auth.isCallback(uri)) return false
        i!!.data = null
        status.text = "Finishing sign-in…"
        actions.removeAllViews()
        thread(name = "files-signin") {
            val err = try {
                drive.auth.completeSignIn(uri!!)
                drive.refreshComputers()
                null
            } catch (e: Exception) {
                e.message ?: "Sign-in failed"
            }
            runOnUiThread {
                drive.notifyRoots()
                render(err)
                if (err == null) askNotifications()
            }
        }
        return true
    }

    private fun render(error: String?) {
        actions.removeAllViews()
        if (drive.auth.signedIn()) {
            val n = drive.computers.size
            status.text = (if (error != null) "$error\n\n" else "") + when (n) {
                0 -> "Signed in. Add a computer at ai.allternit.com and it shows up in the Files app under Allternit."
                1 -> "Signed in. ${drive.computers[0].name} is in the Files app under Allternit, and Share > Save to computer saves into it from any app."
                else -> "Signed in. Your $n computers are in the Files app under Allternit, and Share > Save to computer saves into them from any app."
            }
            actions.addView(FilesUi.matchWidth(FilesUi.button(this, "Open Files", true) { openFiles() }))
            actions.addView(FilesUi.gap(this, 12))
            actions.addView(FilesUi.matchWidth(FilesUi.button(this, "Sign out", false) {
                drive.signOut()
                render(null)
            }))
        } else {
            status.text = (if (error != null) "$error\n\n" else "") +
                "Sign in to see your computers in the Files app and save to them from any app. Browsing a sleeping computer never wakes it."
            actions.addView(FilesUi.matchWidth(FilesUi.button(this, "Sign in", true) { startSignIn() }))
        }
    }

    private fun startSignIn() {
        val url = try {
            drive.auth.beginSignIn()
        } catch (e: Exception) {
            render("This phone's secure storage is not available: ${e.message}")
            return
        }
        try {
            CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(this, url)
        } catch (e: Exception) {
            startActivity(Intent(Intent.ACTION_VIEW, url))
        }
    }

    private fun openFiles() {
        val root = drive.computers.firstOrNull()?.id
        val uri = if (root != null) DocumentsContract.buildRootUri(drive.authority, root) else DocumentsContract.buildRootsUri(drive.authority)
        val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, DocumentsContract.Root.MIME_TYPE_ITEM)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        try {
            startActivity(view)
        } catch (e: Exception) {
            // Some phones have no viewer for a root: open the picker instead.
            startActivity(Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*"))
        }
    }

    /** Wake prompts and save results arrive as notifications (Android 13+ asks). */
    private fun askNotifications() {
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 22)
        }
    }
}
