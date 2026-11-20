package com.allternit.app.files

import android.app.Activity
import android.os.Bundle
import android.widget.CheckBox
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import com.allternit.app.R
import com.allternit.files.contract.WakeReason
import com.allternit.files.contract.WakeSetting
import kotlin.concurrent.thread

/**
 * The wake prompt (SPEC section 9, design copy from the Files designs page): opening or
 * saving a file on a sleeping computer asks first. "Wake" calls wake-consent, the only
 * call that wakes a computer, and keeps the ticket for the requests that follow.
 * "Wake automatically from now on" sets the computer's wake setting to `auto`.
 * Started from the wake notification the provider posts, and for a result from a computer's in-app Files screens
 * (:feature:computer, iOS FilesWakeSheet): there it answers RESULT_OK after a successful wake, and the screen runs the
 * file action again (iOS FilesComputerModel.wake → prompt.retry).
 */
class WakePromptActivity : Activity() {
    private val drive by lazy { FilesDrive.get(this) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val cid = intent.getStringExtra(EXTRA_COMPUTER) ?: return finish()
        val reason = WakeReason.fromWire(intent.getStringExtra(EXTRA_REASON) ?: "") ?: WakeReason.OPEN_FILE
        val itemId = intent.getStringExtra(EXTRA_ITEM)
        val file = intent.getStringExtra(EXTRA_FILE)
        val name = drive.computerName(cid)
        val retries = intent.getBooleanExtra(EXTRA_RETRY, false)

        val col = FilesUi.column(this)
        // A rounded sheet on the dialog's transparent window (Material 3 dialog radius).
        col.background = android.graphics.drawable.GradientDrawable().apply {
            cornerRadius = FilesUi.dp(this@WakePromptActivity, 28).toFloat()
            setColor(this@WakePromptActivity.getColor(R.color.files_bg))
        }
        col.addView(FilesUi.title(this, "Wake $name?"))
        val what = when (reason) {
            WakeReason.UPLOAD -> if (file != null) "Saving $file" else "Saving files"
            else -> if (file != null) "Opening $file" else "Opening this file"
        }
        val body = FilesUi.body(this, "$what needs $name awake. While awake it uses your plan's awake rate. Browsing the last synced list stays free.")
        col.addView(body)
        val auto = CheckBox(this).apply {
            text = "Wake this computer automatically from now on"
            setTextColor(getColor(R.color.files_text))
        }
        col.addView(auto)
        col.addView(FilesUi.gap(this, 16))
        val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        lateinit var wake: TextView
        val notNow = FilesUi.button(this, "Not now", false) { finish() }
        wake = FilesUi.button(this, "Wake", true) {
            wake.isEnabled = false
            notNow.isEnabled = false
            body.text = "Waking $name…"
            thread(name = "files-wake") {
                var ok = false
                val msg = try {
                    if (auto.isChecked) drive.setWakeSetting(cid, WakeSetting.AUTO)
                    val res = drive.consentWake(cid, reason, itemId)
                    drive.kickUploads()
                    drive.notifyRoots()
                    val s = res.estimatedWakeS
                    ok = true
                    if (retries) {
                        "Waking $name" + (if (s != null && s > 0) " (about $s s)" else "") + "."
                    } else if (reason == WakeReason.UPLOAD) {
                        "Waking $name. Your files upload as soon as it is up."
                    } else {
                        "Waking $name" + (if (s != null && s > 0) " (about $s s)" else "") + ". Open the file again in a moment."
                    }
                } catch (e: Exception) {
                    "Couldn't wake $name: ${e.message}"
                }
                runOnUiThread {
                    Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
                    if (ok) setResult(RESULT_OK)
                    finish()
                }
            }
        }
        val lp = { LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f) }
        row.addView(notNow, lp().apply { marginEnd = FilesUi.dp(this@WakePromptActivity, 8) })
        row.addView(wake, lp())
        col.addView(row)
        setContentView(col)
        window.setLayout((resources.displayMetrics.widthPixels * 0.9).toInt(), android.view.ViewGroup.LayoutParams.WRAP_CONTENT)
    }

    companion object {
        const val EXTRA_COMPUTER = "computer_id"
        const val EXTRA_REASON = "reason"
        const val EXTRA_ITEM = "item_id"
        const val EXTRA_FILE = "file_name"
        /** Set by an in-app caller that runs the action again on RESULT_OK. */
        const val EXTRA_RETRY = "retry"
    }
}
