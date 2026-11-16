package com.allternit.app.files

import android.app.Activity
import android.graphics.drawable.GradientDrawable
import android.os.Bundle
import android.view.Gravity
import android.view.View
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import com.allternit.app.R
import org.json.JSONObject
import kotlin.concurrent.thread

/**
 * Inbox tasks (Files drive step 17). A file dropped in a computer's Inbox is untrusted data and
 * waits here as a task. Nothing reads it until the owner taps Approve; Reject leaves it unread.
 * Buttons only (no long-press menus). A finished task shows its result's provenance label:
 * AI-generated, which agent, who approved it. Routes and shapes: docs/api/files-drive-inbox.mdx.
 */
class InboxActivity : Activity() {
    private val drive by lazy { FilesDrive.get(this) }
    private lateinit var list: LinearLayout

    /** One task from `GET /inbox/tasks`, read from the runtime's JSON. */
    private data class Task(
        val id: String,
        val name: String,
        val state: String,
        val droppedBy: String,
        val agentId: String?,
        val approvedBy: String?,
        val results: Int,
        val error: String?,
    )

    /** One computer's section: its tasks, or why they could not be read. */
    private class Section(val cid: String, val tasks: List<Task>?, val error: String?)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val col = FilesUi.column(this, padH = 0, padV = 16)
        val head = FilesUi.column(this, padH = 24, padV = 8)
        head.addView(FilesUi.title(this, "Inbox tasks"))
        head.addView(FilesUi.body(this, "Files dropped in your computers' Inbox wait here. Your agent reads a file only after you approve it."))
        col.addView(head)
        list = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(ScrollView(this).apply { addView(list) }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
        FilesUi.setContent(this, col)
    }

    override fun onResume() {
        super.onResume()
        if (!drive.auth.signedIn()) {
            list.removeAllViews()
            list.addView(FilesUi.row(this, "Sign in to Allternit to see your Inbox tasks.", null, null))
            return
        }
        load()
    }

    private fun load() {
        list.removeAllViews()
        list.addView(FilesUi.row(this, "Loading…", null, null))
        thread(name = "files-inbox-load") {
            val sections = ArrayList<Section>()
            val computers = try {
                drive.refreshComputers()
            } catch (e: Exception) {
                drive.computers
            }
            val shown = computers.filter { it.id !in drive.hiddenComputers() }
            for (c in shown) {
                try {
                    sections.add(Section(c.id, parse(drive.client(c.id).inboxTasksText()), null))
                } catch (e: Exception) {
                    sections.add(Section(c.id, null, e.message ?: "Couldn't reach ${c.name}"))
                }
            }
            runOnUiThread { show(sections) }
        }
    }

    private fun parse(text: String): List<Task> {
        val arr = JSONObject(text).optJSONArray("tasks") ?: return emptyList()
        val out = ArrayList<Task>()
        for (i in 0 until arr.length()) {
            val o = arr.getJSONObject(i)
            val by = o.optJSONObject("dropped_by")
            val grant = o.optJSONObject("grant")
            val approver = grant?.optJSONObject("approved_by")
            out.add(
                Task(
                    id = o.getString("task_id"),
                    name = str(o, "name") ?: "File",
                    state = str(o, "state") ?: "needs_approval",
                    droppedBy = by?.let { str(it, "display_name") ?: str(it, "kind") } ?: "someone",
                    agentId = grant?.let { str(it, "agent_id") },
                    approvedBy = approver?.let { str(it, "display_name") ?: str(it, "kind") },
                    results = o.optJSONArray("result_item_ids")?.length() ?: 0,
                    error = str(o, "error"),
                ),
            )
        }
        return out
    }

    private fun str(o: JSONObject, k: String): String? = if (o.isNull(k)) null else o.optString(k).takeIf { it.isNotBlank() }

    private fun show(sections: List<Section>) {
        list.removeAllViews()
        if (sections.isEmpty()) {
            list.addView(FilesUi.row(this, "No computers to show", "Choose which computers show in Files settings", null))
            return
        }
        for (sec in sections) {
            list.addView(label(drive.computerName(sec.cid)))
            val tasks = sec.tasks
            if (tasks == null) {
                list.addView(FilesUi.row(this, sec.error ?: "Couldn't reach the computer", null, null))
            } else if (tasks.isEmpty()) {
                list.addView(FilesUi.row(this, "No tasks", null, null))
            } else {
                for (t in tasks) list.addView(card(sec.cid, t))
            }
        }
    }

    private fun label(text: String): View = TextView(this).apply {
        this.text = text
        textSize = 13f
        setTextColor(getColor(R.color.files_text_secondary))
        setPadding(FilesUi.dp(this@InboxActivity, 24), FilesUi.dp(this@InboxActivity, 16), FilesUi.dp(this@InboxActivity, 24), FilesUi.dp(this@InboxActivity, 4))
    }

    private fun badge(t: Task): View {
        val (text, color) = when (t.state) {
            "needs_approval" -> Pair("Needs approval", R.color.files_status_warning)
            "approved", "approved_by_rule", "running" -> Pair("Running", R.color.files_status_info)
            "succeeded" -> Pair("Done", R.color.files_status_agent)
            "failed" -> Pair("Failed", R.color.files_status_error)
            "rejected" -> Pair("Rejected", R.color.files_status_muted)
            else -> Pair(t.state, R.color.files_status_muted)
        }
        return TextView(this).apply {
            this.text = text
            textSize = 12f
            setTextColor(getColor(R.color.files_badge_glyph))
            setPadding(FilesUi.dp(this@InboxActivity, 10), FilesUi.dp(this@InboxActivity, 3), FilesUi.dp(this@InboxActivity, 10), FilesUi.dp(this@InboxActivity, 3))
            background = GradientDrawable().apply {
                cornerRadius = FilesUi.dp(this@InboxActivity, 12).toFloat()
                setColor(getColor(color))
            }
        }
    }

    private fun card(cid: String, t: Task): View {
        val box = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(FilesUi.dp(this@InboxActivity, 16), FilesUi.dp(this@InboxActivity, 12), FilesUi.dp(this@InboxActivity, 16), FilesUi.dp(this@InboxActivity, 12))
            background = GradientDrawable().apply {
                cornerRadius = FilesUi.dp(this@InboxActivity, 16).toFloat()
                setColor(getColor(R.color.files_fill))
            }
            layoutParams = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT).apply {
                setMargins(FilesUi.dp(this@InboxActivity, 16), FilesUi.dp(this@InboxActivity, 4), FilesUi.dp(this@InboxActivity, 16), FilesUi.dp(this@InboxActivity, 4))
            }
        }
        val top = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
        top.addView(
            TextView(this).apply {
                text = t.name
                textSize = 16f
                setTextColor(getColor(R.color.files_text))
            },
            LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f),
        )
        top.addView(badge(t))
        box.addView(top)
        box.addView(small("Dropped by ${t.droppedBy}. Untrusted until you approve it."))
        when (t.state) {
            "needs_approval" -> {
                val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
                val reject = FilesUi.button(this, "Reject", false) { act(cid, t, false) }
                val approve = FilesUi.button(this, "Approve", true) { act(cid, t, true) }
                val lp = { LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f) }
                row.addView(reject, lp().apply { marginEnd = FilesUi.dp(this@InboxActivity, 8) })
                row.addView(approve, lp())
                box.addView(FilesUi.gap(this, 8))
                box.addView(row)
            }
            "succeeded" -> {
                // Provenance label, from the task's grant (the sidecar next to the result carries the same).
                val who = t.approvedBy?.let { " Approved by $it." } ?: ""
                val agent = t.agentId?.let { " Agent $it." } ?: ""
                box.addView(small("AI-generated.$agent$who"))
                box.addView(small(if (t.results == 1) "1 result file next to the original, with its provenance file." else "${t.results} result files next to the original, with provenance files."))
            }
            "failed" -> box.addView(small(t.error ?: "A step was refused or failed."))
        }
        return box
    }

    private fun small(text: String): TextView = TextView(this).apply {
        this.text = text
        textSize = 13f
        setTextColor(getColor(R.color.files_text_secondary))
        setPadding(0, FilesUi.dp(this@InboxActivity, 4), 0, 0)
    }

    private fun act(cid: String, t: Task, approve: Boolean) {
        Toast.makeText(this, if (approve) "Approving ${t.name}" else "Rejecting ${t.name}", Toast.LENGTH_SHORT).show()
        thread(name = "files-inbox-act") {
            val err = try {
                val c = drive.client(cid)
                if (approve) c.inboxApproveText(t.id) else c.inboxRejectText(t.id)
                null
            } catch (e: Exception) {
                e.message ?: "Couldn't reach the computer"
            }
            runOnUiThread {
                if (err != null) Toast.makeText(this, err, Toast.LENGTH_LONG).show()
                load()
            }
        }
    }
}
