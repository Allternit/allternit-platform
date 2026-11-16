package com.allternit.app.files

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.view.View
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import com.allternit.app.R
import com.allternit.files.contract.ItemKind
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.concurrent.thread

/**
 * "Save to computer" in Android's share sheet (design 7C: choose the computer, then
 * the folder). Shared files are copied into staging on the phone (streamed, so the
 * grant on the sender's URI can end) and upload in the background through the same
 * queue as the DocumentsProvider: resumable, `keep_both`, and a sleeping computer asks
 * before it wakes.
 */
class ShareToComputerActivity : Activity() {
    private val drive by lazy { FilesDrive.get(this) }
    private val prefs by lazy { getSharedPreferences("files_drive", MODE_PRIVATE) }

    /** One level of the chooser. `rid`/`iid` null = the computer list or a computer's roots. */
    private data class Level(val cid: String?, val rid: String?, val iid: String?, val title: String)

    private val stack = ArrayList<Level>()
    private lateinit var heading: TextView
    private lateinit var where: TextView
    private lateinit var list: LinearLayout
    private lateinit var save: TextView
    private var shared: List<Uri> = emptyList()
    private var sharedText: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        readShare(intent)
        if (shared.isEmpty() && sharedText == null) {
            Toast.makeText(this, "Nothing to save", Toast.LENGTH_SHORT).show()
            return finish()
        }
        val col = FilesUi.column(this, padH = 0, padV = 16)
        val head = FilesUi.column(this, padH = 24, padV = 8)
        heading = FilesUi.title(this, "Save to computer")
        val count = if (shared.isNotEmpty()) shared.size else 1
        where = FilesUi.body(this, if (count == 1) "1 item" else "$count items")
        head.addView(heading)
        head.addView(where)
        col.addView(head)
        list = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        col.addView(ScrollView(this).apply { addView(list) }, LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f))
        val bar = FilesUi.column(this, padH = 24, padV = 12)
        save = FilesUi.button(this, "Save here", true) { saveHere() }
        bar.addView(FilesUi.matchWidth(save))
        col.addView(bar)
        setContentView(col)

        if (!drive.auth.signedIn()) {
            showSignIn()
            return
        }
        push(Level(null, null, null, "Choose a computer"))
    }

    override fun onResume() {
        super.onResume()
        // Back from sign-in.
        if (stack.isEmpty() && ::list.isInitialized && drive.auth.signedIn()) push(Level(null, null, null, "Choose a computer"))
    }

    private fun readShare(i: Intent) {
        when (i.action) {
            Intent.ACTION_SEND -> {
                @Suppress("DEPRECATION")
                val u = i.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                if (u != null) shared = listOf(u) else sharedText = i.getStringExtra(Intent.EXTRA_TEXT)
            }
            Intent.ACTION_SEND_MULTIPLE -> {
                @Suppress("DEPRECATION")
                shared = i.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM) ?: emptyList()
            }
        }
    }

    private fun showSignIn() {
        list.removeAllViews()
        save.visibility = View.GONE
        val box = FilesUi.column(this)
        box.addView(FilesUi.body(this, "Sign in to Allternit to save to your computers."))
        box.addView(FilesUi.matchWidth(FilesUi.button(this, "Sign in", true) {
            startActivity(Intent(this, FilesSignInActivity::class.java))
        }))
        list.addView(box)
    }

    private fun push(l: Level) {
        stack.add(l)
        render()
    }

    @Deprecated("Framework Activity back handling; the app has no androidx activity dependency")
    override fun onBackPressed() {
        if (stack.size > 1) {
            stack.removeAt(stack.size - 1)
            render()
        } else {
            @Suppress("DEPRECATION")
            super.onBackPressed()
        }
    }

    private fun render() {
        val l = stack.last()
        heading.text = l.title
        save.visibility = if (l.iid != null) View.VISIBLE else View.GONE
        list.removeAllViews()
        list.addView(FilesUi.row(this, "Loading…", null, null))
        thread(name = "files-share-list") {
            val rows = try {
                load(l)
            } catch (e: Exception) {
                listOf(Triple(e.message ?: "Couldn't reach the computer", null as String?, null as Level?))
            }
            runOnUiThread {
                if (stack.lastOrNull() != l) return@runOnUiThread
                list.removeAllViews()
                if (rows.isEmpty()) list.addView(FilesUi.row(this, if (l.iid != null) "No folders here. Save here, or go back." else "Nothing here yet", null, null))
                for ((title, sub, next) in rows) {
                    list.addView(FilesUi.row(this, title, sub, next?.let { n -> { push(n) } }))
                    list.addView(View(this).apply {
                        setBackgroundColor(getColor(R.color.files_divider))
                        layoutParams = LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 1)
                    })
                }
            }
        }
    }

    /** Rows for a level: (title, subtitle, where a tap goes). */
    private fun load(l: Level): List<Triple<String, String?, Level?>> {
        if (l.cid == null) {
            val list = try {
                drive.refreshComputers()
            } catch (e: Exception) {
                drive.computers.ifEmpty { throw e }
            }
            val out = ArrayList<Triple<String, String?, Level?>>()
            lastTarget()?.let { out.add(Triple("Last used: ${it.title}", drive.computerName(it.cid!!), it)) }
            list.forEach { out.add(Triple(it.name, if (it.status == "online") "Awake" else "Saves when it wakes", Level(it.id, null, null, it.name))) }
            return out
        }
        val cid = l.cid
        if (l.rid == null) {
            val roots = try {
                drive.refreshRoots(cid)
            } catch (e: Exception) {
                drive.rootsCached(cid) ?: throw e
            }
            val ids = roots.roots.map { it.id }.toSet()
            return roots.roots
                .filter { (it.parentRootId == null || it.parentRootId !in ids) && it.policy.write && !it.policy.protected }
                .map { Triple(it.name, null, Level(cid, it.id, it.rootItemId, it.name)) }
        }
        val listing = try {
            drive.refreshListing(cid, l.rid, l.iid!!)
        } catch (e: Exception) {
            drive.listingCached(cid, l.rid, l.iid!!) ?: throw e
        }
        return listing.items.filter { it.kind == ItemKind.FOLDER && it.permissions.write }.map { item ->
            val link = item.rootLink
            val rid = link ?: l.rid
            val iid = if (link != null) drive.root(cid, link)?.rootItemId ?: item.id else item.id
            Triple(item.name, null, Level(cid, rid, iid, item.name))
        }
    }

    private fun lastTarget(): Level? {
        val cid = prefs.getString("share_cid", null) ?: return null
        if (drive.computer(cid) == null) return null
        return Level(cid, prefs.getString("share_rid", null) ?: return null, prefs.getString("share_iid", null) ?: return null, prefs.getString("share_title", null) ?: "Folder")
    }

    private fun saveHere() {
        val l = stack.last()
        val cid = l.cid ?: return
        val rid = l.rid ?: return
        val iid = l.iid ?: return
        prefs.edit().putString("share_cid", cid).putString("share_rid", rid).putString("share_iid", iid).putString("share_title", l.title).apply()
        save.isEnabled = false
        save.text = "Copying…"
        thread(name = "files-share-copy") {
            var n = 0
            var failed: String? = null
            try {
                if (shared.isEmpty()) {
                    val stamp = SimpleDateFormat("yyyy-MM-dd HHmm", Locale.US).format(Date())
                    val rec = drive.newPending(cid, rid, iid, "Shared text $stamp.txt", "text/plain", "writing")
                    rec.file.writeText(sharedText ?: "")
                    drive.queue(rec)
                    n = 1
                } else {
                    for (u in shared) {
                        val name = displayName(u)
                        val mime = contentResolver.getType(u) ?: intent.type ?: AllternitDocumentsProvider.mimeOf(name)
                        val rec = drive.newPending(cid, rid, iid, name, mime, "writing")
                        val input = contentResolver.openInputStream(u) ?: continue
                        input.use { inp -> rec.file.outputStream().use { FilesClient.copy(inp, it) } }
                        drive.queue(rec)
                        n += 1
                    }
                }
            } catch (e: Exception) {
                failed = e.message
            }
            runOnUiThread {
                val dest = "${drive.computerName(cid)} · ${l.title}"
                val msg = if (failed != null) "Couldn't copy everything: $failed" else if (n == 1) "Saving to $dest" else "Saving $n files to $dest"
                Toast.makeText(this, msg, Toast.LENGTH_LONG).show()
                finish()
            }
        }
    }

    private fun displayName(u: Uri): String {
        try {
            contentResolver.query(u, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0)?.takeIf { it.isNotBlank() }?.let { return it }
            }
        } catch (_: Exception) {
        }
        return u.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "Shared file"
    }
}
