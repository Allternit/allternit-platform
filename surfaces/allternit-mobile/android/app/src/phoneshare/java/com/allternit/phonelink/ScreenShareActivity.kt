package com.allternit.phonelink

import android.Manifest
import android.app.Activity
import android.bluetooth.BluetoothAdapter
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
import android.widget.TextView
import com.allternit.app.R
import com.allternit.phonelink.core.HidAvailability
import com.allternit.phonelink.core.HidState
import com.allternit.phonelink.core.SharePhase

/**
 * Screen sharing (both builds): connect this phone to your Allternit account
 * with the one-time code, approve the pairing there, then share the screen and
 * optionally turn on taps. Reached from the "Share screen" shortcut on the app
 * icon, the request notification, the Phone Link screen (sideloaded build) and
 * the link allternit://phone-share?code=… (the code is only filled in, never
 * sent until the person taps Connect).
 *
 * Taps need the Accessibility service; Google Play policy (and plain honesty)
 * wants a prominent disclosure first, so "Turn on taps" shows it and only
 * "Agree and open Settings" opens Accessibility settings.
 *
 * "Control my iPhone by Bluetooth" (Phone Use route `bt-android`) is a row
 * here that opens its own panel in this screen: turn it on, pair the iPhone
 * once from its Settings › Bluetooth, pick or disconnect the iPhone.
 */
class ScreenShareActivity : Activity() {
    private companion object {
        const val REQ_BLUETOOTH = 43
        const val REQ_DISCOVERABLE = 44
    }

    private lateinit var root: LinearLayout
    private lateinit var codeInput: EditText
    private var showingDisclosure = false
    private var showingBluetooth = false
    /** Pairing instructions stay up after the person asked to make this phone visible. */
    private var pairingHelp = false
    private var turnOnAfterPermission = false
    private val refresh: () -> Unit = { runOnUiThread { render() } }

    private val dark: Boolean
        get() = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
    private val bg get() = if (dark) Color.rgb(28, 28, 30) else Color.WHITE
    private val ink get() = if (dark) Color.rgb(236, 236, 238) else Color.rgb(30, 30, 30)
    private val muted get() = if (dark) Color.rgb(160, 160, 166) else Color.rgb(90, 90, 90)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        AppLink.init(this)
        root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val pad = dp(20)
            setPadding(pad, pad, pad, pad)
            setBackgroundColor(bg)
        }
        codeInput = EditText(this).apply {
            hint = "Code from Allternit"
            contentDescription = "One-time code from Allternit"
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_CAP_CHARACTERS or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
            imeOptions = EditorInfo.IME_ACTION_DONE
            isSingleLine = true
            setTextColor(ink)
            setHintTextColor(muted)
            setOnEditorActionListener { _, id, _ -> if (id == EditorInfo.IME_ACTION_DONE) { connect(); true } else false }
        }
        prefill(intent)
        setContentView(ScrollView(this).apply { setBackgroundColor(bg); addView(root) })
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        prefill(intent)
        render()
    }

    /** allternit://phone-share?code=ABCD-1234: fill the box only. */
    private fun prefill(intent: Intent?) {
        val code = intent?.data?.takeIf { it.scheme == "allternit" && it.host == "phone-share" }?.getQueryParameter("code")
        val clean = code?.filter { it.isLetterOrDigit() || it == '-' }?.take(32)
        if (!clean.isNullOrEmpty() && !AppLink.isPaired) codeInput.setText(clean)
    }

    override fun onResume() {
        super.onResume()
        AppLink.addListener(refresh)
        BluetoothRemote.init(this)
        BluetoothRemote.addListener(refresh)
        AppLink.ensureControl()
        AppLink.shareStateChanged()
        if (AppLink.isPaired && Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            // The "Someone wants to see your screen" notice needs it.
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 42)
        }
        render()
    }

    override fun onPause() {
        AppLink.removeListener(refresh)
        BluetoothRemote.removeListener(refresh)
        super.onPause()
        AppLink.shareStateChanged()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_BLUETOOTH && turnOnAfterPermission) {
            turnOnAfterPermission = false
            if (BluetoothRemote.hasPermissions(this)) BluetoothRemoteService.start(this)
        }
        render()
    }

    @Deprecated("Back without AndroidX")
    override fun onBackPressed() {
        if (showingDisclosure) { showingDisclosure = false; render() }
        else if (showingBluetooth) { showingBluetooth = false; pairingHelp = false; render() }
        else {
            @Suppress("DEPRECATION")
            super.onBackPressed()
        }
    }

    // ── Views ───────────────────────────────────────────────────────────────

    private fun dp(v: Int) = (v * resources.displayMetrics.density).toInt()

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

    private fun row(vararg views: View) = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        views.forEach { addView(it) }
    }

    private fun gap() = View(this).apply { minimumHeight = dp(16) }

    private fun accessibilityEnabled(): Boolean {
        val enabled = Settings.Secure.getString(contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES) ?: return false
        return enabled.split(':').any { it.startsWith("$packageName/") }
    }

    private fun render() {
        root.removeAllViews()
        if (showingDisclosure) { renderDisclosure(); return }
        if (showingBluetooth && AppLink.isPaired) { renderBluetooth(); return }
        root.addView(text("Share this phone's screen", 22f, bold = true))
        root.addView(text("Let your own Allternit account see this screen, and tap on it if you allow that. " +
            "Nothing is shared until you tap Share screen and say yes to Android's prompt.", 14f, color = muted))
        if (AppLink.isPaired) renderPaired() else renderClaim()
        root.addView(gap())
    }

    private fun renderClaim() {
        root.addView(gap())
        root.addView(text("Connect to your Allternit account", 17f, bold = true))
        root.addView(text("In Allternit (web, desktop or app), open Settings › Connections › Phones › Add an Android phone. " +
            "Enter the code it shows. The code connects this phone to the account that made it.", 14f))
        (codeInput.parent as? ViewGroup)?.removeView(codeInput)
        val pairing = AppLink.pairing
        codeInput.isEnabled = !pairing
        root.addView(codeInput, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        if (pairing) {
            val code = AppLink.pairCode
            if (code == null) root.addView(text("Connecting…", 14f, bold = true))
            else {
                root.addView(text("Approve this phone in Allternit. Check that it shows the same code:", 14f))
                root.addView(text("${code.take(3)} ${code.drop(3)}", 28f, bold = true))
            }
            root.addView(button("Cancel") { AppLink.cancelPairing() })
        } else {
            root.addView(button("Connect") { connect() })
        }
        AppLink.pairError?.let { root.addView(text(it, 14f, bold = true)) }
    }

    private fun connect() {
        val code = codeInput.text?.toString()?.trim().orEmpty()
        if (code.length < 4 || AppLink.pairing) return
        AppLink.pair(code, null)
    }

    private fun renderPaired() {
        val creds = AppLink.credentials()
        root.addView(text("Connected as \"${creds?.name ?: "this phone"}\"" +
            if (AppLink.controlConnected) "" else " · reconnecting", 14f, color = muted))

        root.addView(gap())
        root.addView(text("Screen", 17f, bold = true))
        val state = when (AppLink.session.phase) {
            SharePhase.OFF -> when (AppLink.session.lastEnded) {
                null -> "Not sharing."
                "idle" -> "Not sharing. It stopped after 10 minutes with nobody watching."
                "locked" -> "Not sharing. It stopped when the phone locked."
                else -> "Not sharing."
            }
            SharePhase.PAUSED -> "Ready. Nobody is watching right now."
            SharePhase.LIVE -> "Sharing. Someone in your Allternit account is watching."
        }
        root.addView(text(state, 15f, bold = true))
        if (AppLink.session.isOn) {
            root.addView(button("Stop sharing") { ProjectionService.stopShare(this, "stopped") })
        } else {
            root.addView(button("Share screen") { startActivity(Intent(this, ProjectionConsentActivity::class.java)) })
        }
        root.addView(text("Android shows a chip in the status bar while sharing; tap it to stop at any time. " +
            "Apps that hide their content (banking, passwords) show as black.", 13f, color = muted))

        root.addView(gap())
        root.addView(text("Taps", 17f, bold = true))
        when {
            AppLink.ops != null -> {
                root.addView(text("On. A viewer you allow in Allternit can tap and type on this phone while you share.", 14f))
                root.addView(button("Turn off in Settings") { openAccessibilitySettings() })
            }
            accessibilityEnabled() -> root.addView(text("Turned on. Waiting for Android to start it.", 14f))
            else -> {
                root.addView(text("Off. Viewers can watch but not tap.", 14f))
                root.addView(button("Turn on taps") { showingDisclosure = true; render() })
            }
        }

        root.addView(gap())
        root.addView(text("Control my iPhone by Bluetooth", 17f, bold = true))
        root.addView(text(bluetoothSummary(), 14f))
        root.addView(button("Set up") { showingBluetooth = true; render() })

        root.addView(gap())
        root.addView(button("Disconnect this phone") { BluetoothRemoteService.stop(this); AppLink.unpair(this); render() })
        root.addView(text("Removes the connection from this phone. Remove it in Allternit too (Settings › Connections › Phones).", 13f, color = muted))
    }

    /** Google Play's Accessibility API policy: say what is collected and why, then ask, before the settings jump. */
    private fun renderDisclosure() {
        root.addView(text("Before you turn on taps", 22f, bold = true))
        root.addView(text(getString(R.string.share_a11y_disclosure), 15f))
        root.addView(text("If Android says the setting is restricted: open Settings › Apps › Allternit, tap ⋮ (top right), " +
            "then Allow restricted settings, and try again.", 13f, color = muted))
        root.addView(row(
            button("Agree and open Settings") { showingDisclosure = false; openAccessibilitySettings() },
            button("Not now") { showingDisclosure = false; render() },
        ))
    }

    // ── Control my iPhone by Bluetooth (Phone Use `bt-android`) ──────────────

    private fun bluetoothSummary(): String {
        val s = BluetoothRemote.state()
        return when {
            BluetoothRemote.deviceUnsupported() || s.availability == HidAvailability.UNSUPPORTED -> HidState.NOT_AVAILABLE_MESSAGE
            s.connected -> "On. Connected to ${s.name ?: "your iPhone"}."
            BluetoothRemote.isOn -> "On. Waiting for your iPhone to connect."
            else -> "Off. Use this phone as a Bluetooth keyboard and mouse for your iPhone."
        }
    }

    private fun renderBluetooth() {
        root.addView(text("Control my iPhone by Bluetooth", 22f, bold = true))
        root.addView(text("This phone becomes a Bluetooth keyboard and mouse for your own iPhone. Your Allternit assistant " +
            "(Desktop or cloud) can then move the pointer, tap and type on the iPhone, and sees the result through the " +
            "Allternit screen broadcast on the iPhone. Anything that sends a message, spends money or changes security " +
            "still waits for your approval.", 14f, color = muted))
        root.addView(gap())
        if (BluetoothRemote.deviceUnsupported() || BluetoothRemote.state().availability == HidAvailability.UNSUPPORTED) {
            root.addView(text(HidState.NOT_AVAILABLE_MESSAGE, 15f, bold = true))
            root.addView(button("Back") { showingBluetooth = false; render() })
            return
        }
        val s = BluetoothRemote.state()
        if (!BluetoothRemote.isOn) {
            root.addView(text(when (s.availability) {
                HidAvailability.BLUETOOTH_OFF -> "Bluetooth is off on this phone. Turn it on, then tap Turn on."
                HidAvailability.NO_PERMISSION -> "Off. Allternit asks for Nearby devices so it can be a Bluetooth keyboard and mouse."
                else -> "Off."
            }, 15f, bold = true))
            root.addView(row(
                button("Turn on") { turnOnBluetoothRemote() },
                button("Back") { showingBluetooth = false; render() },
            ))
            return
        }

        root.addView(text(when {
            s.connected -> "Connected to ${s.name ?: "your iPhone"}."
            s.availability == HidAvailability.BLUETOOTH_OFF -> "Bluetooth is off on this phone. Turn it on to reconnect."
            else -> "On. Waiting for your iPhone to connect."
        }, 15f, bold = true))
        if (s.connected) {
            root.addView(text("On the iPhone, turn on AssistiveTouch so a pointer shows: Settings › Accessibility › Touch › AssistiveTouch.", 13f, color = muted))
            root.addView(button("Disconnect iPhone") { BluetoothRemote.disconnect(); render() })
        }

        root.addView(gap())
        root.addView(text("Pair a new iPhone", 17f, bold = true))
        if (pairingHelp) {
            val me = BluetoothRemote.ownName()?.let { "\"$it\"" } ?: "this phone"
            root.addView(text("1. On your iPhone, open Settings › Bluetooth.\n" +
                "2. Under Other Devices, tap $me. Accept the pairing on both phones.\n" +
                "3. Turn on AssistiveTouch: Settings › Accessibility › Touch › AssistiveTouch.\n" +
                "4. In the Allternit app on the iPhone, start the screen broadcast.", 14f))
            root.addView(text("This phone stays visible for 2 minutes.", 13f, color = muted))
        }
        root.addView(button("Make this phone visible") { makeDiscoverable() })

        val bonded = BluetoothRemote.bondedDevices()
        if (bonded.isNotEmpty()) {
            root.addView(gap())
            root.addView(text("Already paired", 17f, bold = true))
            val current = BluetoothRemote.connectedDevice()?.address
            bonded.forEach { d ->
                val name = BluetoothRemote.safeName(d) ?: d.address
                if (d.address == current) root.addView(text("$name · connected", 14f))
                else root.addView(button("Use $name") { BluetoothRemote.connect(d); render() })
            }
        }

        root.addView(gap())
        root.addView(row(
            button("Turn off") { BluetoothRemoteService.stop(this); pairingHelp = false; render() },
            button("Back") { showingBluetooth = false; pairingHelp = false; render() },
        ))
    }

    private fun turnOnBluetoothRemote() {
        val needed = BluetoothRemote.neededPermissions().filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (needed.isNotEmpty()) {
            turnOnAfterPermission = true
            requestPermissions(needed.toTypedArray(), REQ_BLUETOOTH)
            return
        }
        BluetoothRemoteService.start(this)
        render()
    }

    /** Android's own "make visible" prompt; the iPhone starts the pairing from its Settings › Bluetooth. */
    private fun makeDiscoverable() {
        pairingHelp = true
        try {
            @Suppress("DEPRECATION")
            startActivityForResult(
                Intent(BluetoothAdapter.ACTION_REQUEST_DISCOVERABLE).putExtra(BluetoothAdapter.EXTRA_DISCOVERABLE_DURATION, 120),
                REQ_DISCOVERABLE,
            )
        } catch (_: Exception) {}
        render()
    }

    private fun openAccessibilitySettings() {
        try { startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) } catch (_: Exception) {}
    }
}
