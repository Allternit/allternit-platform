package com.allternit.ondevice

import android.content.Context
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The native app's view of the on-phone engine (gap run GS-D, iOS OnDeviceRuntime + OnDeviceDownloads):
 * installed models, resumable downloads with pause, resume and cancel, storage use, and one chat reply.
 *
 * The Play TWA keeps [ModelDownloads] (DownloadManager, no pause). The native Models screen needs pause and
 * resume like iOS's URLSession resume data, so it downloads here with HTTP Range requests into app-private
 * storage. The same manifest check applies: size + sha256 from the signed Freeweights manifest, then
 * [ModelStore.install]. Chat ids are `ondevice/<freeweights id>#<quant>` (iOS OnDeviceChat.prefix).
 */
object PhoneModels {
    const val CHAT_PREFIX = "ondevice/"
    private const val TAG = "PhoneModels"
    private const val PREFS = "allternit_ondevice_phone_downloads"

    @Volatile private var app: Context? = null

    /** Remembers the application context. Cheap: loads no native code. */
    fun init(context: Context) {
        if (app == null) app = context.applicationContext
    }

    private fun ctx(): Context = app ?: error("PhoneModels.init was not called")
    private fun engine(): OnDeviceEngine = OnDeviceEngine.get(ctx())

    fun chatId(key: String) = CHAT_PREFIX + key
    fun keyOfChatId(id: String?): String? = id?.takeIf { it.startsWith(CHAT_PREFIX) }?.removePrefix(CHAT_PREFIX)?.takeIf { it.isNotEmpty() }

    // ---- engine and storage

    /**
     * True when this build carries the llama.cpp libraries for this phone's ABI and they load. Loads native
     * code on first call: call it off the main thread.
     */
    fun engineReady(): Boolean = runCatching { engine().engineInfo().optString("state") == "ready" }.getOrDefault(false)

    /** "opencl" or "cpu" for the engine row. */
    fun backend(): String = runCatching { engine().engineInfo().optString("backend", "cpu") }.getOrDefault("cpu")

    fun installed(): List<ModelStore.Installed> = runCatching { engine().store.list() }.getOrDefault(emptyList())

    /** Bytes the downloaded models take on disk. */
    fun storageUsed(): Long = installed().sumOf { it.path.length() }

    fun freeBytes(): Long = runCatching { engine().store.freeBytes() }.getOrDefault(0L)

    /** What the app may give one model in memory right now (MemoryBudget). */
    fun memoryBudget(): Long = MemoryBudget.read(ctx()).budgetBytes

    fun delete(key: String): Boolean {
        val e = engine()
        if (e.loadedKey() == key) e.requestUnload("deleted")
        return e.store.delete(key)
    }

    /** iOS memorySection: what is loaded, its speed. Null when nothing is. */
    fun running(): JSONObject? = runCatching { engine().runningJson() }.getOrNull()

    fun unload() = engine().requestUnload("stopped by user")

    // ---- chat

    /**
     * One reply on this phone. [onText] runs on the engine thread for each piece; return false to stop.
     * [onEnd] gets null on success, else the message to show.
     */
    fun chat(key: String, roles: List<String>, contents: List<String>, onText: (String) -> Boolean, onEnd: (String?) -> Unit) {
        val e = engine()
        if (e.store.get(key) == null) {
            onEnd("This model is not on this phone any more. Download it again in Models.")
            return
        }
        if (!engineReady()) {
            onEnd("Models on this phone are not available on this build.")
            return
        }
        e.chatDirect(key, roles.toTypedArray(), contents.toTypedArray(), 512, 0.6f, onText) { err, _ -> onEnd(err) }
    }

    fun stopReply() = runCatching { engine().stopAny() }

    // ---- downloads

    enum class Status { Queued, Downloading, Paused, Verifying, Failed }

    data class Download(
        val key: String, val model: String, val quant: String, val repo: String, val file: String, val name: String,
        val status: Status, val done: Long, val total: Long, val message: String? = null,
    ) {
        val fraction: Float get() = if (total > 0) (done.toDouble() / total).toFloat().coerceIn(0f, 1f) else 0f
    }

    private data class Job(
        val key: String, val model: String, val quant: String, val repo: String, val file: String, val name: String,
        val url: String, val size: Long, val sha256: String,
    )

    private val pool = Executors.newFixedThreadPool(2) { r -> Thread(r, "phone-model-download") }
    private val live = ConcurrentHashMap<String, Download>()
    private val stopFlags = ConcurrentHashMap<String, AtomicBoolean>()

    /** Every download that is not installed yet, newest state. Jobs left from a killed app show as Paused. */
    fun downloads(): List<Download> {
        val saved = jobs()
        return saved.map { j ->
            live[j.key] ?: Download(j.key, j.model, j.quant, j.repo, j.file, j.name, Status.Paused, partFile(j.key).length(), j.size)
        } + live.values.filter { d -> saved.none { it.key == d.key } }
    }

    /** Starts a download (fetches and checks the signed manifest first). Never on the main thread's critical path. */
    fun download(model: String, quant: String, repo: String, file: String, name: String) {
        val key = ModelStore.keyOf(model, quant)
        if (live[key]?.status.let { it == Status.Downloading || it == Status.Queued || it == Status.Verifying }) return
        live[key] = Download(key, model, quant, repo, file, name, Status.Queued, 0, 0)
        pool.execute {
            try {
                if (engine().store.get(key) != null) throw Protocol.Failure("installed", "$name $quant is already on this phone.")
                val ref = FreeweightsManifest.fetch(repo, file)
                val headroom = 200L * 1024 * 1024
                if (freeBytes() < ref.size + headroom) {
                    throw Protocol.Failure("disk_full", "Not enough free space: ${"%.1f".format((ref.size + headroom) / 1e9)} GB needed.")
                }
                val job = Job(key, model, quant, repo, file, name, ref.url, ref.size, ref.sha256)
                save(jobs().filter { it.key != key } + job)
                run(job)
            } catch (f: Protocol.Failure) {
                live[key] = Download(key, model, quant, repo, file, name, Status.Failed, 0, 0, f.message)
            } catch (e: Exception) {
                live[key] = Download(key, model, quant, repo, file, name, Status.Failed, 0, 0, "Freeweights could not be reached. Check your connection.")
            }
        }
    }

    fun pause(key: String) {
        stopFlags[key]?.set(true)
    }

    fun resume(key: String) {
        val job = jobs().firstOrNull { it.key == key } ?: return
        if (live[key]?.status == Status.Downloading) return
        live[key] = Download(job.key, job.model, job.quant, job.repo, job.file, job.name, Status.Queued, partFile(key).length(), job.size)
        pool.execute { run(job) }
    }

    fun cancel(key: String) {
        stopFlags[key]?.set(true)
        save(jobs().filter { it.key != key })
        live.remove(key)
        partFile(key).delete()
    }

    /** Clears a failed row (iOS downloads.errors dismiss). */
    fun dismiss(key: String) {
        if (live[key]?.status == Status.Failed) live.remove(key)
    }

    private fun partFile(key: String) = File(File(ctx().filesDir, "ondevice/partial").apply { mkdirs() }, ModelStore.slotName(key) + ".part")

    private fun run(job: Job) {
        val stop = AtomicBoolean(false)
        stopFlags[job.key] = stop
        val part = partFile(job.key)
        fun state(s: Status, done: Long, msg: String? = null) {
            live[job.key] = Download(job.key, job.model, job.quant, job.repo, job.file, job.name, s, done, job.size, msg)
        }
        try {
            var have = part.length().coerceAtMost(job.size)
            if (have < job.size) {
                val conn = URL(job.url).openConnection() as HttpURLConnection
                conn.connectTimeout = 15_000
                conn.readTimeout = 30_000
                if (have > 0) conn.setRequestProperty("Range", "bytes=$have-")
                try {
                    val code = conn.responseCode
                    if (code == 200) have = 0 else if (code != 206) throw Protocol.Failure("http", "Download failed (error $code).")
                    RandomAccessFile(part, "rw").use { out ->
                        out.setLength(have)
                        out.seek(have)
                        val buf = ByteArray(1 shl 16)
                        var lastEmit = 0L
                        conn.inputStream.use { input ->
                            while (true) {
                                if (stop.get()) {
                                    state(Status.Paused, have)
                                    return
                                }
                                val n = input.read(buf)
                                if (n < 0) break
                                out.write(buf, 0, n)
                                have += n
                                val now = System.currentTimeMillis()
                                if (now - lastEmit > 250) { state(Status.Downloading, have); lastEmit = now }
                            }
                        }
                    }
                } finally {
                    conn.disconnect()
                }
            }
            state(Status.Verifying, have)
            val ok = part.length() == job.size && ModelStore.sha256Hex(part.inputStream()) == job.sha256
            if (!ok) {
                part.delete()
                save(jobs().filter { it.key != job.key })
                state(Status.Failed, 0, "The download did not match the Freeweights manifest (sha256). It was deleted.")
                return
            }
            engine().store.install(job.key, job.model, job.quant, job.repo, job.file, job.name, job.size, job.sha256, part)
            save(jobs().filter { it.key != job.key })
            live.remove(job.key)
            Log.i(TAG, "installed ${job.key}")
        } catch (f: Protocol.Failure) {
            state(Status.Failed, part.length(), f.message)
        } catch (e: Exception) {
            // Network drop: keep the partial file so Resume continues from it.
            state(Status.Paused, part.length(), "The connection dropped. Tap Resume to continue.")
        } finally {
            stopFlags.remove(job.key, stop)
        }
    }

    @Synchronized
    private fun jobs(): List<Job> {
        val raw = ctx().getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("jobs", null) ?: return emptyList()
        return runCatching {
            val arr = JSONArray(raw)
            (0 until arr.length()).map { i ->
                val j = arr.getJSONObject(i)
                Job(j.getString("key"), j.getString("model"), j.getString("quant"), j.getString("repo"), j.getString("file"),
                    j.getString("name"), j.getString("url"), j.getLong("size"), j.getString("sha256"))
            }
        }.getOrDefault(emptyList())
    }

    @Synchronized
    private fun save(list: List<Job>) {
        val arr = JSONArray()
        list.forEach { j ->
            arr.put(JSONObject().put("key", j.key).put("model", j.model).put("quant", j.quant).put("repo", j.repo).put("file", j.file)
                .put("name", j.name).put("url", j.url).put("size", j.size).put("sha256", j.sha256))
        }
        ctx().getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString("jobs", arr.toString()).commit()
    }
}
