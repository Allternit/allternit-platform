package com.allternit.app.files

import android.app.Activity
import android.app.AlertDialog
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import com.allternit.app.R
import com.allternit.files.contract.FilesKeyDevice
import com.allternit.files.contract.FilesKeyInfo
import kotlin.concurrent.thread

/**
 * Files key (SPEC section 13), opened from Files settings ([FilesSignInActivity]). Same
 * flow as the iOS Devices tab and the web Devices tab:
 *
 * - this phone: Set up (first device), Ask for access, or the 6-digit code while waiting
 *   (checked every 5 seconds until another device approves);
 * - waiting devices: the code recomputed here; Approve (only when it matches) and Decline;
 * - devices with the key, each with Remove; "Make a new key" after a removal;
 * - computers: Give key.
 *
 * Buttons only, no long-press menus. Network calls run off the main thread.
 */
class FilesKeyActivity : Activity() {
    private val drive by lazy { FilesDrive.get(this) }
    private val keys by lazy { FilesKeys.get(drive) }
    private lateinit var list: LinearLayout
    private val main = Handler(Looper.getMainLooper())
    private var polling = false
    private var busy = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val col = FilesUi.column(this, padH = 0, padV = 16)
        val head = FilesUi.column(this, padH = 24, padV = 8)
        head.addView(FilesUi.title(this, "Files key"))
        head.addView(FilesUi.body(this, "Your computers keep sealed copies of chosen folders on each other. With the Files key, this phone opens a file from a copy while its computer sleeps, without waking it. The key never leaves your devices."))
        col.addView(head)
        list = FilesUi.column(this, padH = 24, padV = 8)
        col.addView(ScrollView(this).apply { addView(list) }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
        FilesUi.setContent(this, col)
    }

    override fun onResume() {
        super.onResume()
        if (!drive.auth.signedIn()) {
            list.removeAllViews()
            list.addView(FilesUi.body(this, "Sign in to Allternit Files first."))
            return
        }
        load(null)
    }

    override fun onPause() {
        super.onPause()
        polling = false
    }

    private fun load(message: String?) {
        if (list.childCount == 0) list.addView(FilesUi.body(this, "Checking…"))
        thread(name = "files-key-load") {
            val result = runCatching {
                val info = keys.info()
                info to keys.state(info)
            }
            main.post {
                if (isFinishing) return@post
                result.fold(
                    onSuccess = { (info, state) -> render(info, state, message) },
                    onFailure = { render(null, null, message ?: (it.message ?: "Couldn't reach Allternit.")) },
                )
            }
        }
    }

    private fun render(info: FilesKeyInfo?, state: FilesKeys.State?, message: String?) {
        list.removeAllViews()
        if (message != null) {
            list.addView(small(message))
            list.addView(FilesUi.gap(this, 12))
        }
        polling = state is FilesKeys.State.Waiting
        when (state) {
            null -> list.addView(button("Try again", true) { load(null) })
            is FilesKeys.State.NoKey -> {
                list.addView(FilesUi.body(this, "Your account has no Files key yet. Make it on this phone, then approve your other devices from here."))
                list.addView(button("Set up the Files key", true) { act { keys.createFirstKey(); "Files key made on this phone." } })
            }
            is FilesKeys.State.NeedsRequest -> {
                list.addView(FilesUi.body(this, "Your other devices already have the Files key. Ask one of them to share it with this phone."))
                list.addView(button("Ask for access", true) { act { keys.requestAccess(); null } })
            }
            is FilesKeys.State.Waiting -> {
                list.addView(FilesUi.body(this, "Approve this phone on another of your devices (Files settings > Devices on the web, Desktop or iPhone). It shows this code:"))
                list.addView(code(state.code, 34f))
                list.addView(small("Waiting for approval…"))
                list.addView(button("Send the request again", false) { act { keys.requestAccess(); null } })
                schedulePoll()
            }
            is FilesKeys.State.Ready -> {
                list.addView(FilesUi.row(this, "Ready", "Key ${state.keyId}", null))
                info?.let { renderHolder(it) }
            }
        }
    }

    private fun renderHolder(info: FilesKeyInfo) {
        val approvals = keys.approvals(info)
        if (approvals.isNotEmpty()) {
            section("Waiting for approval", "Approve a device only if it shows the same code as here.")
            for (a in approvals) {
                list.addView(FilesUi.body(this, a.request.deviceName))
                if (a.valid) {
                    list.addView(code(a.code, 22f))
                } else {
                    list.addView(small("This request's code doesn't match its key. Don't approve it; decline it and ask the device to request again."))
                }
                val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
                val lp = { LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f) }
                row.addView(FilesUi.button(this, "Decline", false) { act { keys.decline(a); "Declined ${a.request.deviceName}." } },
                    lp().apply { marginEnd = FilesUi.dp(this@FilesKeyActivity, 8) })
                if (a.valid) {
                    row.addView(FilesUi.button(this, "Approve", true) { confirmApprove(a) }, lp())
                }
                list.addView(row)
                list.addView(FilesUi.gap(this, 12))
            }
        }
        if (info.rotationNeeded) {
            section("New key needed", "A device was removed. Make a new Files key so it can't open copies made from now on.")
            list.addView(button("Make a new key", true) { act { rotate(info) } })
        }
        if (info.devices.isNotEmpty()) {
            section("Devices with the Files key", null)
            for (d in info.devices) deviceRow(d)
        }
        val computers = drive.computersInFiles()
        if (computers.isNotEmpty()) {
            section("Computers", "A computer needs the key to keep sealed copies for your other computers.")
            for (c in computers) {
                val has = c.id in info.deviceIds
                list.addView(FilesUi.row(this, c.name, if (has) "Has the key" else "No key yet", null))
                list.addView(button(if (has) "Send the key again" else "Give key", false) {
                    act { if (has) keys.deliver(c.id, c.id) else keys.giveComputer(c.id); "${c.name} has the Files key." }
                })
            }
        }
    }

    private fun rotate(info: FilesKeyInfo): String {
        keys.rotate(info)
        val missed = drive.computers.filter { c -> info.devices.any { it.deviceId == c.id } }
            .filter { c -> runCatching { keys.deliver(c.id, c.id) }.isFailure }
            .map { it.name }
        return if (missed.isEmpty()) "New key made." else "New key made. Give it to ${missed.joinToString()} when they're on."
    }

    private fun deviceRow(d: FilesKeyDevice) {
        val me = d.deviceId == drive.auth.deviceId
        list.addView(FilesUi.row(this, if (me) "${d.deviceName} (this phone)" else d.deviceName,
            if (d.hasCurrentKey) "Has the current key" else "Waiting for the new key", null))
        list.addView(button("Remove", false) {
            AlertDialog.Builder(this)
                .setTitle("Remove ${d.deviceName}?")
                .setMessage("${d.deviceName} loses the Files key. Make a new key afterwards so it can't open new copies.")
                .setPositiveButton("Remove") { _, _ -> act { keys.removeDevice(d.deviceId); "${d.deviceName} removed." } }
                .setNegativeButton("Cancel", null)
                .show()
        })
    }

    private fun confirmApprove(a: FilesKeys.Approval) {
        AlertDialog.Builder(this)
            .setTitle("Give ${a.request.deviceName} the Files key?")
            .setMessage("It can then open your replicas. Check that ${a.request.deviceName} shows ${spaced(a.code)}.")
            .setPositiveButton("Approve") { _, _ -> act { keys.approve(a); "${a.request.deviceName} approved." } }
            .setNegativeButton("Cancel", null)
            .show()
    }

    /** Runs a key action off the main thread, then reloads with its message or error. */
    private fun act(block: () -> String?) {
        if (busy) return
        busy = true
        list.addView(small("Working…"), 0)
        thread(name = "files-key-act") {
            val msg = try {
                block()
            } catch (e: Exception) {
                e.message ?: "Something went wrong. Try again."
            }
            main.post {
                busy = false
                if (!isFinishing) load(msg)
            }
        }
    }

    private fun schedulePoll() {
        main.postDelayed({ if (polling && !isFinishing && !busy) load(null) }, 5_000)
    }

    private fun section(title: String, footer: String?) {
        list.addView(FilesUi.gap(this, 20))
        list.addView(TextView(this).apply {
            text = title
            textSize = 15f
            setTypeface(typeface, Typeface.BOLD)
            setTextColor(getColor(R.color.files_text))
        })
        footer?.let { list.addView(small(it)) }
        list.addView(FilesUi.gap(this, 8))
    }

    private fun button(text: String, primary: Boolean, onClick: () -> Unit) =
        FilesUi.matchWidth(FilesUi.button(this, text, primary) { onClick() }).also {
            (it.layoutParams as? LinearLayout.LayoutParams)?.topMargin = FilesUi.dp(this, 8)
        }

    private fun code(code: String, size: Float) = TextView(this).apply {
        text = spaced(code)
        textSize = size
        typeface = Typeface.MONOSPACE
        setTextColor(getColor(R.color.files_text))
        contentDescription = "Code " + code.toList().joinToString(" ")
    }

    private fun small(text: String) = TextView(this).apply {
        this.text = text
        textSize = 13f
        setTextColor(getColor(R.color.files_text_secondary))
    }

    private fun spaced(code: String) = if (code.length == 6) code.take(3) + " " + code.takeLast(3) else code
}
