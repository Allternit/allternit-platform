package com.allternit.ondevice

import android.app.DownloadManager
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * Model downloads through the system DownloadManager:
 *  - the system shows the download notification with progress, and the download keeps
 *    going when the app is closed or killed;
 *  - any network: Wi-Fi, mobile data, metered and roaming (Eoj's call, plan decision);
 *  - DownloadManager can only write to the app's external files dir, so the file lands
 *    there, is checked (size + sha256 from the Freeweights manifest) and then moved into
 *    app-private storage ([ModelStore]). A mismatch deletes it and reports an error.
 *
 * Active downloads are kept in SharedPreferences, so a download that finished while the
 * app was not running is verified on the next start ([reconcile]) or by
 * [DownloadCompleteReceiver].
 */
class ModelDownloads(private val context: Context, private val store: ModelStore, private val emit: (JSONObject) -> Unit) {
    private val dm = context.getSystemService(DownloadManager::class.java)
    private val prefs = context.getSharedPreferences("allternit_ondevice_downloads", Context.MODE_PRIVATE)
    private val worker = Executors.newSingleThreadScheduledExecutor { r -> Thread(r, "ondevice-downloads") }
    private var poller: ScheduledFuture<*>? = null
    private val verifying = HashSet<String>()

    data class Job(val key: String, val id: Long, val model: String, val quant: String, val repo: String,
                   val file: String, val name: String, val size: Long, val sha256: String) {
        fun toJson(): JSONObject = JSONObject().put("key", key).put("id", id).put("model", model).put("quant", quant)
            .put("repo", repo).put("file", file).put("name", name).put("size", size).put("sha256", sha256)

        companion object {
            fun from(j: JSONObject) = Job(j.getString("key"), j.getLong("id"), j.getString("model"), j.getString("quant"),
                j.getString("repo"), j.getString("file"), j.getString("name"), j.getLong("size"), j.getString("sha256"))
        }
    }

    /** Fetches the manifest and queues the download. Runs on the caller's (bridge) worker. */
    fun start(model: String, quant: String, repo: String, file: String, name: String): Job {
        val key = ModelStore.keyOf(model, quant)
        jobs().firstOrNull { it.key == key }?.let { return it }
        store.get(key)?.let { throw Protocol.Failure("installed", "$name $quant is already on this phone.") }
        val ref = FreeweightsManifest.fetch(repo, file)
        val headroom = 200L * 1024 * 1024
        if (store.freeBytes() < ref.size * 2 + headroom) {
            // x2: the download sits in the external dir until it is verified and moved.
            throw Protocol.Failure("disk_full", "Not enough free space: ${gb(ref.size * 2 + headroom)} GB needed.")
        }
        val dir = File(context.getExternalFilesDir(null), DOWNLOAD_DIR).apply { mkdirs() }
        val target = File(dir, ModelStore.slotName(key) + ".part")
        target.delete()
        val req = DownloadManager.Request(Uri.parse(ref.url))
            .setTitle("$name $quant")
            .setDescription("Allternit: model for chat on this phone")
            .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE)
            .setAllowedNetworkTypes(DownloadManager.Request.NETWORK_WIFI or DownloadManager.Request.NETWORK_MOBILE)
            .setAllowedOverMetered(true)
            .setAllowedOverRoaming(true)
            .setDestinationUri(Uri.fromFile(target))
        val id = dm.enqueue(req)
        val job = Job(key, id, model, quant, repo, file, name, ref.size, ref.sha256)
        save(jobs().filter { it.key != key } + job)
        Log.i(TAG, "download $key id=$id ${ref.url}")
        ensurePolling()
        return job
    }

    fun cancel(key: String): Boolean {
        val job = jobs().firstOrNull { it.key == key } ?: return false
        dm.remove(job.id)
        save(jobs().filter { it.key != key })
        emit(progress(job, "cancelled", 0, job.size))
        return true
    }

    /** Snapshot for `list`: one entry per active download. */
    fun active(): List<JSONObject> = jobs().map { job -> query(job) ?: progress(job, "queued", 0, job.size) }

    /** Verifies anything that finished while nobody was watching, then polls the rest. */
    fun reconcile(done: (() -> Unit)? = null) {
        worker.execute {
            try { tick() } finally { done?.invoke() }
        }
        ensurePolling()
    }

    @Synchronized
    private fun ensurePolling() {
        if (poller?.isDone == false) return
        poller = worker.scheduleWithFixedDelay({ tick() }, 0, 700, TimeUnit.MILLISECONDS)
    }

    private fun tick() {
        val all = jobs()
        if (all.isEmpty()) {
            poller?.cancel(false)
            return
        }
        for (job in all) {
            val status = query(job)
            if (status == null) {
                // Removed from DownloadManager (user cancelled it from the notification).
                save(jobs().filter { it.key != job.key })
                emit(progress(job, "cancelled", 0, job.size))
                continue
            }
            when (status.getString("status")) {
                "downloaded" -> verify(job)
                "error" -> {
                    dm.remove(job.id)
                    save(jobs().filter { it.key != job.key })
                    emit(status)
                }
                else -> emit(status)
            }
        }
    }

    private fun verify(job: Job) {
        if (!verifying.add(job.key)) return
        try {
            val file = File(File(context.getExternalFilesDir(null), DOWNLOAD_DIR), ModelStore.slotName(job.key) + ".part")
            emit(progress(job, "verifying", job.size, job.size))
            val ok = file.isFile && file.length() == job.size &&
                ModelStore.sha256Hex(file.inputStream()) == job.sha256
            if (!ok) {
                Log.w(TAG, "verify failed for ${job.key}: size=${file.length()} want=${job.size}")
                file.delete()
                dm.remove(job.id)
                save(jobs().filter { it.key != job.key })
                emit(progress(job, "error", 0, job.size).put("message", "The download did not match the Freeweights manifest (sha256). It was deleted."))
                return
            }
            val installed = store.install(job.key, job.model, job.quant, job.repo, job.file, job.name, job.size, job.sha256, file)
            // The system keeps a row for a completed download; drop it now the file moved.
            dm.remove(job.id)
            save(jobs().filter { it.key != job.key })
            emit(progress(job, "ready", job.size, job.size).put("model_info", installed.toJson()))
            Log.i(TAG, "installed ${job.key} (${job.size} bytes, sha256 ok)")
        } finally {
            verifying.remove(job.key)
        }
    }

    private fun query(job: Job): JSONObject? {
        dm.query(DownloadManager.Query().setFilterById(job.id)).use { c ->
            if (!c.moveToFirst()) return null
            val st = c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_STATUS))
            val done = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR))
            val total = c.getLong(c.getColumnIndexOrThrow(DownloadManager.COLUMN_TOTAL_SIZE_BYTES)).takeIf { it > 0 } ?: job.size
            val reason = c.getInt(c.getColumnIndexOrThrow(DownloadManager.COLUMN_REASON))
            return when (st) {
                DownloadManager.STATUS_SUCCESSFUL -> progress(job, "downloaded", total, total)
                DownloadManager.STATUS_FAILED -> progress(job, "error", done, total).put("message", "Download failed (reason $reason).")
                DownloadManager.STATUS_PAUSED -> progress(job, "paused", done, total).put("reason", pausedReason(reason))
                DownloadManager.STATUS_PENDING -> progress(job, "queued", done, total)
                else -> progress(job, "downloading", done, total)
            }
        }
    }

    private fun pausedReason(r: Int) = when (r) {
        DownloadManager.PAUSED_WAITING_FOR_NETWORK -> "waiting_for_network"
        DownloadManager.PAUSED_WAITING_TO_RETRY -> "retrying"
        DownloadManager.PAUSED_QUEUED_FOR_WIFI -> "queued_for_wifi"
        else -> "paused"
    }

    private fun progress(job: Job, status: String, done: Long, total: Long): JSONObject = JSONObject()
        .put("key", job.key).put("model", job.model).put("quant", job.quant).put("name", job.name)
        .put("status", status).put("done", done).put("total", total)

    @Synchronized
    private fun jobs(): List<Job> {
        val raw = prefs.getString(PREF_JOBS, null) ?: return emptyList()
        return try {
            val arr = org.json.JSONArray(raw)
            (0 until arr.length()).map { Job.from(arr.getJSONObject(it)) }
        } catch (e: Exception) {
            emptyList()
        }
    }

    @Synchronized
    private fun save(list: List<Job>) {
        val arr = org.json.JSONArray()
        list.forEach { arr.put(it.toJson()) }
        prefs.edit().putString(PREF_JOBS, arr.toString()).commit()
    }

    private fun gb(b: Long) = "%.1f".format(b / 1e9)

    companion object {
        const val TAG = "OnDeviceDownloads"
        private const val PREF_JOBS = "jobs"
        private const val DOWNLOAD_DIR = "ondevice-downloads"
    }
}

/** DownloadManager tells the app when a download ends, even if the app was not running. */
class DownloadCompleteReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != DownloadManager.ACTION_DOWNLOAD_COMPLETE) return
        // goAsync keeps the process up while a finished file is hashed and moved.
        val pending = goAsync()
        OnDeviceEngine.get(context).downloads.reconcile { pending.finish() }
    }
}
