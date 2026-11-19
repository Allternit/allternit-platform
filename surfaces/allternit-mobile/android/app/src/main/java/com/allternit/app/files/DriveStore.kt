package com.allternit.app.files

import com.allternit.core.files.Blake3

import android.net.Uri
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.security.MessageDigest
import java.util.UUID

/**
 * Document ids of the provider. Contract ids are opaque (SPEC section 4: never parse
 * an ItemId), so each part is URI-encoded and joined with `/`, which encoding removes
 * from the parts.
 *
 *  - `c/<computer>`                     one computer (the root's top: its Files roots)
 *  - `i/<computer>/<root>/<item>`       a file or folder (the root id is needed for writes)
 *  - `p/<upload record>`                a file created on this phone, not uploaded yet
 *  - `signin`                           the signed-out placeholder root
 */
internal object DocIds {
    const val SIGNIN = "signin"

    sealed class Parsed {
        data class Computer(val computerId: String) : Parsed()
        data class Item(val computerId: String, val rootId: String, val itemId: String) : Parsed()
        data class Pending(val recordId: String) : Parsed()
        object SignIn : Parsed()
    }

    fun computer(cid: String) = "c/" + Uri.encode(cid)
    fun item(cid: String, rid: String, iid: String) = "i/" + Uri.encode(cid) + "/" + Uri.encode(rid) + "/" + Uri.encode(iid)
    fun pending(recordId: String) = "p/" + Uri.encode(recordId)

    fun parse(id: String): Parsed {
        if (id == SIGNIN) return Parsed.SignIn
        val p = id.split('/')
        return when {
            p.size == 2 && p[0] == "c" -> Parsed.Computer(Uri.decode(p[1]))
            p.size == 4 && p[0] == "i" -> Parsed.Item(Uri.decode(p[1]), Uri.decode(p[2]), Uri.decode(p[3]))
            p.size == 2 && p[0] == "p" -> Parsed.Pending(Uri.decode(p[1]))
            else -> throw FileNotFoundException("Unknown document $id")
        }
    }
}

/**
 * A file written on this phone that is on its way to a computer (a save from another
 * app, a share, or an edit). Persisted so an upload survives the process being killed;
 * `sessionId` lets a chunked upload resume (SPEC section 5).
 *
 * States: `writing` (a client still has it open), `queued`, `waiting_wake` (the
 * computer sleeps and the user has not agreed to wake it), `done`, `failed`.
 */
internal class UploadRecord(
    val id: String,
    val computerId: String,
    val rootId: String,
    val parentId: String,
    var name: String,
    val mime: String,
    var itemId: String?,
    var baseRev: String?,
    val staging: String,
    var state: String,
    var sessionId: String? = null,
    var resultItemId: String? = null,
    var error: String? = null,
    val createdAt: Long = System.currentTimeMillis(),
    var updatedAt: Long = System.currentTimeMillis(),
    var attempts: Int = 0,
) {
    val file get() = File(staging)

    fun toJson(): JSONObject = JSONObject()
        .put("id", id).put("computer_id", computerId).put("root_id", rootId).put("parent_id", parentId)
        .put("name", name).put("mime", mime).put("item_id", itemId ?: JSONObject.NULL)
        .put("base_rev", baseRev ?: JSONObject.NULL).put("staging", staging).put("state", state)
        .put("session_id", sessionId ?: JSONObject.NULL).put("result_item_id", resultItemId ?: JSONObject.NULL)
        .put("error", error ?: JSONObject.NULL).put("created_at", createdAt).put("updated_at", updatedAt)
        .put("attempts", attempts)

    companion object {
        private fun JSONObject.str(k: String): String? = if (isNull(k)) null else optString(k)

        fun fromJson(o: JSONObject) = UploadRecord(
            id = o.getString("id"),
            computerId = o.getString("computer_id"),
            rootId = o.getString("root_id"),
            parentId = o.getString("parent_id"),
            name = o.getString("name"),
            mime = o.optString("mime", "application/octet-stream"),
            itemId = o.str("item_id"),
            baseRev = o.str("base_rev"),
            staging = o.getString("staging"),
            state = o.getString("state"),
            sessionId = o.str("session_id"),
            resultItemId = o.str("result_item_id"),
            error = o.str("error"),
            createdAt = o.optLong("created_at"),
            updatedAt = o.optLong("updated_at"),
            attempts = o.optInt("attempts"),
        )
    }
}

/**
 * Everything the drive keeps on the phone, under no-backup storage (the data
 * extraction rules exclude it anyway):
 *
 *  - `computers.json`, `roots/`, `lists/`: the last answers, so the picker shows the
 *    last list right away, offline, and while a computer sleeps (served_by cache).
 *  - `content/`: the offline cache of file contents, keyed by item, with the rev it
 *    holds. Capped at [CONTENT_CAP_BYTES]; the oldest-used files go first.
 *  - `staging/` + `uploads/`: files on their way up ([UploadRecord]).
 *  - `thumbs/`: thumbnails made on the phone from cached content.
 */
internal class DriveCache(val root: File) {
    private val lists = dir("lists")
    private val roots = dir("roots")
    private val content = dir("content")
    private val staging = dir("staging")
    private val uploads = dir("uploads")
    val thumbs = dir("thumbs")

    private fun dir(name: String) = File(root, name).apply { mkdirs() }

    fun key(vararg parts: String): String {
        val md = MessageDigest.getInstance("SHA-256").digest(parts.joinToString("\u0000").toByteArray(Charsets.UTF_8))
        return Blake3.hex(md).substring(0, 40)
    }

    @Synchronized
    fun writeText(f: File, text: String) {
        val tmp = File(f.parentFile, f.name + ".tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(f)) {
            f.delete()
            tmp.renameTo(f)
        }
    }

    private fun readText(f: File): String? = try {
        if (f.exists()) f.readText() else null
    } catch (e: Exception) {
        null
    }

    // Computers
    private val computersFile = File(root, "computers.json")

    fun readComputers(): List<Computer> = try {
        val arr = JSONArray(readText(computersFile) ?: "[]")
        List(arr.length()) {
            val o = arr.getJSONObject(it)
            Computer(o.getString("id"), o.getString("name"), o.optString("runtime_type"), o.optString("status"), o.optBoolean("wakeable"))
        }
    } catch (e: Exception) {
        emptyList()
    }

    fun writeComputers(list: List<Computer>) {
        val arr = JSONArray()
        list.forEach {
            arr.put(JSONObject().put("id", it.id).put("name", it.name).put("runtime_type", it.runtimeType).put("status", it.status).put("wakeable", it.wakeable))
        }
        writeText(computersFile, arr.toString())
    }

    // Roots and listings (raw contract JSON, decoded on read)
    fun rootsFile(cid: String) = File(roots, key(cid) + ".json")
    fun readRoots(cid: String): String? = readText(rootsFile(cid))

    fun listFile(cid: String, rid: String, parent: String) = File(lists, key(cid, rid, parent) + ".json")
    fun readList(cid: String, rid: String, parent: String): JSONObject? = readText(listFile(cid, rid, parent))?.let {
        try {
            JSONObject(it)
        } catch (e: Exception) {
            null
        }
    }

    // Content
    fun contentFile(cid: String, iid: String) = File(content, key(cid, iid))
    private fun revFile(cid: String, iid: String) = File(content, key(cid, iid) + ".rev")

    fun cachedRev(cid: String, iid: String): String? =
        if (contentFile(cid, iid).exists()) readText(revFile(cid, iid))?.trim() else null

    /** Moves `src` into the cache as `iid` at `rev`. */
    fun putContent(cid: String, iid: String, rev: String, src: File) {
        val dst = contentFile(cid, iid)
        dst.delete()
        if (!src.renameTo(dst)) {
            src.copyTo(dst, overwrite = true)
            src.delete()
        }
        writeText(revFile(cid, iid), rev)
        trim()
    }

    fun touch(f: File) {
        f.setLastModified(System.currentTimeMillis())
    }

    fun partFile(cid: String, iid: String, rev: String) = File(content, key(cid, iid, rev) + ".part")

    @Synchronized
    fun trim(cap: Long = CONTENT_CAP_BYTES) {
        val files = content.listFiles { f -> !f.name.endsWith(".rev") && !f.name.endsWith(".tmp") }?.toMutableList() ?: return
        var total = files.sumOf { it.length() }
        if (total <= cap) return
        files.sortBy { it.lastModified() }
        for (f in files) {
            if (total <= cap) break
            total -= f.length()
            f.delete()
            File(f.parentFile, f.name + ".rev").delete()
        }
    }

    fun newStaging(): File = File(staging, UUID.randomUUID().toString())

    // Upload records
    private fun recordFile(id: String) = File(uploads, "$id.json")

    @Synchronized
    fun save(rec: UploadRecord) {
        rec.updatedAt = System.currentTimeMillis()
        writeText(recordFile(rec.id), rec.toJson().toString())
    }

    @Synchronized
    fun record(id: String): UploadRecord? = readText(recordFile(id))?.let {
        try {
            UploadRecord.fromJson(JSONObject(it))
        } catch (e: Exception) {
            null
        }
    }

    @Synchronized
    fun records(): List<UploadRecord> = (uploads.listFiles { f -> f.name.endsWith(".json") } ?: emptyArray())
        .mapNotNull { f ->
            try {
                UploadRecord.fromJson(JSONObject(f.readText()))
            } catch (e: Exception) {
                null
            }
        }
        .sortedBy { it.createdAt }

    @Synchronized
    fun deleteRecord(rec: UploadRecord) {
        recordFile(rec.id).delete()
        rec.file.delete()
    }

    /** Done records stay a week so an app holding a `p/` URI still reaches the uploaded file. */
    fun pruneRecords() {
        val cutoff = System.currentTimeMillis() - 7 * 24 * 3600_000L
        records().filter { it.state == "done" && it.updatedAt < cutoff }.forEach { deleteRecord(it) }
    }

    fun clearAll() {
        root.listFiles()?.forEach { it.deleteRecursively() }
        listOf(lists, roots, content, staging, uploads, thumbs).forEach { it.mkdirs() }
    }

    companion object {
        const val CONTENT_CAP_BYTES = 1L shl 30 // 1 GiB
    }
}
