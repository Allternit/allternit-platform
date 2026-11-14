package com.allternit.phonelink

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.InputType
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Switch
import android.widget.TextView
import com.allternit.phonelink.core.RpcDispatcher
import java.text.DateFormat
import java.util.Date

/**
 * Status screen: is the server up, which computers are paired, Stop / Start,
 * Revoke. Plain framework views on purpose (no AppCompat dependency).
 *
 * Since 0.5.0 the launcher icon opens the Allternit app (m.allternit.com as a
 * Trusted Web Activity, androidbrowserhelper's LauncherActivity). This screen
 * is reached from the Phone Link notification and the icon's "Phone Link"
 * shortcut.
 */
class MainActivity : Activity() {
    private lateinit var root: LinearLayout
    private val refresh: () -> Unit = { runOnUiThread { render() } }
    /** Kept across renders so a server update never wipes what the user is typing. */
    private lateinit var blockInput: EditText

    private val dark: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    private val bg get() = if (dark) Color.rgb(28, 28, 30) else Color.WHITE
    private val ink get() = if (dark) Color.rgb(236, 236, 238) else Color.rgb(30, 30, 30)
    private val muted get() = if (dark) Color.rgb(160, 160, 166) else Color.rgb(90, 90, 90)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ServerController.init(this)
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val pad = (20 * resources.displayMetrics.density).toInt()
            setPadding(pad, pad, pad, pad)
            setBackgroundColor(bg)
        }
        blockInput = EditText(this).apply {
            hint = "App name or package"
            contentDescription = "App name or package to never read"
            inputType = InputType.TYPE_CLASS_TEXT
            imeOptions = EditorInfo.IME_ACTION_DONE
            isSingleLine = true
            setTextColor(ink)
            setHintTextColor(muted)
            setOnEditorActionListener { _, id, _ -> if (id == EditorInfo.IME_ACTION_DONE) { addBlockFromInput(); true } else false }
        }
        setContentView(ScrollView(this).apply { setBackgroundColor(bg); addView(root) })
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        render()
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

    private fun text(s: String, size: Float = 15f, bold: Boolean = false, color: Int = ink) =
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
        root.addView(text("Lets Allternit bots use the apps already signed in on this phone. Nothing runs on the lock screen, in Settings or in this app.", 14f, color = muted))

        root.addView(text(ServerController.statusLine(), 16f, bold = true))
        val a11y = when {
            ServerController.ops != null -> "Accessibility: connected"
            accessibilityOn() -> "Accessibility: enabled, waiting for Android to connect it"
            else -> "Accessibility: off — finish setup from Allternit on your Mac or PC (Settings › Connections › Your phone)"
        }
        root.addView(text(a11y, 14f))
        root.addView(text("This phone: ${s.deviceName}  ·  id ${s.deviceId.take(8)}", 13f, color = muted))
        if (!s.provisioned) root.addView(text("Not set up yet. On your Mac or PC, open Allternit › Settings › Connections › Your phone › Let your bots use this iPhone, with this phone on the same Wi-Fi. It walks you through Wireless debugging, so no cable is needed.", 14f))

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
        if (sessions.isEmpty()) root.addView(text("None", 14f, color = muted))
        val fmt = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT)
        for (sess in sessions) {
            val where = listOfNotNull(sess.lastAddress, sess.lastVia).joinToString(" · ")
            root.addView(text("${sess.clientName}\nlast used ${fmt.format(Date(sess.lastActiveAtMs))}${if (where.isNotEmpty()) " · $where" else ""}", 14f))
            root.addView(button("Revoke") { ServerController.auth.revokeSession(sess.id); render() })
        }
        if (sessions.size > 1) root.addView(button("Revoke all") { ServerController.auth.revokeAll(); render() })
        renderErrorHelp()
        root.addView(View(this).apply { minimumHeight = dp(24) })
    }

    // ── Error help (pet offers) ─────────────────────────────────────────────

    private fun notificationsAllowed(): Boolean = Notifications.canPostPetOffer(this)

    private fun appName(entry: String): String =
        if (RpcDispatcher.PACKAGE_RE.matches(entry)) {
            try { packageManager.getApplicationLabel(packageManager.getApplicationInfo(entry, 0)).toString() } catch (e: Exception) { entry }
        } else entry

    private fun addBlockFromInput() {
        val entry = blockInput.text?.toString()?.trim().orEmpty()
        if (entry.isEmpty()) return
        PetOfferPrefs.settings(this).addBlock(entry)
        blockInput.setText("")
        render()
    }

    private fun renderErrorHelp() {
        val pet = PetOfferPrefs.settings(this)
        root.addView(text("Error help", 17f, bold = true))
        root.addView(text("When an app on this phone shows an error, your bot offers to help in a notification. " +
            "Fix it opens Allternit with the error filled in. Nothing is sent until you send it.", 14f, color = muted))

        root.addView(Switch(this).apply {
            text = "Offer help when an app shows an error"
            textSize = 15f
            setTextColor(ink)
            setPadding(0, dp(8), 0, dp(8))
            isChecked = pet.watching
            setOnCheckedChangeListener { _, on ->
                pet.setWatching(on)
                if (on && Build.VERSION.SDK_INT >= 33 &&
                    checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                    requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 41)
                }
                // Rebuild after the switch finishes its own click.
                root.post { render() }
            }
        })

        if (pet.watching) {
            val status = when {
                ServerController.ops == null -> "On · waiting for Accessibility to connect"
                !notificationsAllowed() -> "On · notifications are off for Allternit, so offers can't show"
                else -> "Watching for errors"
            }
            root.addView(text(status, 14f, bold = true))
            if (!notificationsAllowed()) {
                root.addView(button("Turn on notifications") {
                    startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, packageName))
                })
            }
        }
        root.addView(text("Password managers, banking apps and Messages are never read. Nor are password fields.", 13f, color = muted))

        val mutedApps = pet.muted
        root.addView(text("Muted apps (${mutedApps.size})", 15f, bold = true))
        if (mutedApps.isEmpty()) root.addView(text("None. Tap Mute on an offer to stop offers for that app.", 14f, color = muted))
        for (entry in mutedApps) {
            val name = appName(entry)
            root.addView(itemRow(name, "Unmute", "Unmute $name") { pet.unmute(entry); render() })
        }

        val blocked = pet.blockList
        root.addView(text("Never read (${blocked.size})", 15f, bold = true))
        if (blocked.isEmpty()) root.addView(text("No apps added.", 14f, color = muted))
        for (entry in blocked) {
            val name = appName(entry)
            root.addView(itemRow(name, "Remove", "Remove $name from never read") { pet.removeBlock(entry); render() })
        }
        (blockInput.parent as? ViewGroup)?.removeView(blockInput)
        val add = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        add.addView(blockInput, LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f))
        add.addView(button("Add") { addBlockFromInput() }.apply { contentDescription = "Add to never read" })
        root.addView(add)
    }

    private fun itemRow(label: String, action: String, description: String, onClick: () -> Unit) =
        LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            addView(text(label, 14f), LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f).apply { gravity = android.view.Gravity.CENTER_VERTICAL })
            addView(button(action, onClick).apply { contentDescription = description })
        }
}
