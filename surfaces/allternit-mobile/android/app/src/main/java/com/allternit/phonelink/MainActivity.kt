package com.allternit.phonelink

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.provider.Settings
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import java.text.DateFormat
import java.util.Date

/**
 * Status screen: is the server up, which computers are paired, Stop / Start,
 * Revoke. Plain framework views on purpose (no AppCompat dependency).
 *
 * TODO(twa): wrap m.allternit.com in this same app as a Trusted Web Activity
 * (androidx.browser + a LauncherActivity), so one install gives both the
 * Allternit phone UI and Phone Link. Needs:
 *   1. https://m.allternit.com/.well-known/assetlinks.json listing
 *      com.allternit.phonelink with the release signing cert SHA-256
 *      (delegate_permission/common.handle_all_urls);
 *   2. an asset_statements string resource pointing back at m.allternit.com;
 *   3. this status screen moved behind a "Phone Link" entry (TWA becomes the
 *      launcher activity), keeping it reachable from the notification.
 * Not built now: the release signing key and the assetlinks deploy are
 * decisions for the release, and a debug-signed TWA falls back to a Custom Tab
 * with the URL bar showing.
 */
class MainActivity : Activity() {
    private lateinit var root: LinearLayout
    private val refresh: () -> Unit = { runOnUiThread { render() } }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ServerController.init(this)
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val pad = (20 * resources.displayMetrics.density).toInt()
            setPadding(pad, pad, pad, pad)
            setBackgroundColor(Color.WHITE)
        }
        setContentView(ScrollView(this).apply { setBackgroundColor(Color.WHITE); addView(root) })
    }

    override fun onResume() {
        super.onResume()
        ServerController.addListener(refresh)
        render()
    }

    override fun onPause() {
        ServerController.removeListener(refresh)
        super.onPause()
    }

    private fun text(s: String, size: Float = 15f, bold: Boolean = false, color: Int = Color.rgb(30, 30, 30)) =
        TextView(this).apply {
            text = s
            textSize = size
            setTextColor(color)
            if (bold) typeface = Typeface.DEFAULT_BOLD
            setPadding(0, dp(6), 0, dp(6))
        }

    private fun button(label: String, onClick: () -> Unit) = Button(this).apply {
        text = label
        isAllCaps = false
        setOnClickListener { onClick() }
        layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT)
    }

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

    private fun accessibilityOn(): Boolean {
        val enabled = Settings.Secure.getString(contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES) ?: return false
        return enabled.split(':').any { it.startsWith("$packageName/") }
    }

    private fun render() {
        root.removeAllViews()
        val s = ServerController.settings
        root.addView(text("Allternit Phone Link", 22f, bold = true))
        root.addView(text("Lets Allternit bots use the apps already signed in on this phone. Nothing runs on the lock screen, in Settings or in this app.", 14f, color = Color.rgb(90, 90, 90)))

        root.addView(text(ServerController.statusLine(), 16f, bold = true))
        val a11y = when {
            ServerController.ops != null -> "Accessibility: connected"
            accessibilityOn() -> "Accessibility: enabled, waiting for Android to connect it"
            else -> "Accessibility: off — run setup from Allternit Desktop (USB cable, once)"
        }
        root.addView(text(a11y, 14f))
        root.addView(text("This phone: ${s.deviceName}  ·  id ${s.deviceId.take(8)}", 13f, color = Color.rgb(90, 90, 90)))
        if (!s.provisioned) root.addView(text("Not set up yet. Open Allternit Desktop › Phone Link and plug this phone in once.", 14f))

        val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        if (ServerController.isRunning) row.addView(button("Stop") { ServerController.stopByUser(this) })
        else row.addView(button("Start") { ServerController.startByUser(this) })
        root.addView(row)

        val pending = ServerController.pairing.pendingRequests
        if (pending.isNotEmpty()) {
            root.addView(text("Waiting for your answer", 17f, bold = true))
            for (p in pending) {
                val via = if (p.local) "USB cable / this phone" else "network"
                root.addView(text("\"${p.clientName}\" wants to pair ($via)${p.fingerprint?.let { " · key $it" } ?: ""}", 14f))
                val r = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
                r.addView(button("Allow") { ServerController.pairing.answer(p.requestId, true) })
                r.addView(button("Deny") { ServerController.pairing.answer(p.requestId, false) })
                root.addView(r)
            }
        }

        val sessions = ServerController.auth.activeSessions
        root.addView(text("Paired computers (${sessions.size})", 17f, bold = true))
        if (sessions.isEmpty()) root.addView(text("None", 14f, color = Color.rgb(90, 90, 90)))
        val fmt = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT)
        for (sess in sessions) {
            val where = listOfNotNull(sess.lastAddress, sess.lastVia).joinToString(" · ")
            root.addView(text("${sess.clientName}\nlast used ${fmt.format(Date(sess.lastActiveAtMs))}${if (where.isNotEmpty()) " · $where" else ""}", 14f))
            root.addView(button("Revoke") { ServerController.auth.revokeSession(sess.id); render() })
        }
        if (sessions.size > 1) root.addView(button("Revoke all") { ServerController.auth.revokeAll(); render() })
        root.addView(View(this).apply { minimumHeight = dp(24) })
    }
}
