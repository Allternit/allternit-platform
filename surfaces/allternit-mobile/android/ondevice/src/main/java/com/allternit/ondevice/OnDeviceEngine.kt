package com.allternit.ondevice

import android.content.ComponentCallbacks2
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.atomic.AtomicReference

/**
 * Process-wide owner of the on-device engine: the installed models, downloads, and at most
 * one loaded model. All engine work runs on one worker thread ([handler]); the bridge only
 * posts to it. [EngineService] shows "running" with Stop while a model is loaded. There is no
 * foreground service: the model runs while the app is open (see EngineService).
 *
 * Unload rules: idle for [IDLE_UNLOAD_SEC] after the last reply, the notification's Stop,
 * the page's `unload`, or the system asking for memory (onTrimMemory at
 * RUNNING_CRITICAL or worse, onLowMemory). A reply in progress is stopped first.
 */
class OnDeviceEngine private constructor(private val app: Context) {
    val store = ModelStore(app)
    val downloads = ModelDownloads(app, store) { emit(Protocol.evt("download", it)) }

    private val thread = HandlerThread("ondevice-engine").apply { start() }
    private val handler = Handler(thread.looper)

    /** Where events and token frames go (the TWA bridge). Returns false when not delivered. */
    @Volatile var sink: (String) -> Boolean = { false }

    private data class Loaded(
        val handle: Long,
        val model: ModelStore.Installed,
        val backend: String,
        val nCtx: Int,
        val memBytes: Long,
        var tokPerSec: Double = 0.0,
        var ctxUsed: Int = 0,
    )

    @Volatile private var loaded: Loaded? = null
    private val current = AtomicReference<TokenStream?>(null)
    @Volatile private var generating = false
    private var devices: JSONArray? = null
    /** stop() can come from any thread; free() only from the engine thread. Never both at once. */
    private val nativeLock = Any()

    private val idleUnload = Runnable {
        if (!generating) unloadNow("idle")
    }

    init {
        app.registerComponentCallbacks(object : ComponentCallbacks2 {
            override fun onTrimMemory(level: Int) {
                @Suppress("DEPRECATION")
                if (level >= ComponentCallbacks2.TRIM_MEMORY_RUNNING_CRITICAL) memoryPressure("trim:$level")
            }
            override fun onConfigurationChanged(newConfig: Configuration) {}
            @Deprecated("Deprecated in Java")
            override fun onLowMemory() = memoryPressure("low_memory")
        })
        downloads.reconcile()
    }

    fun post(block: () -> Unit) {
        handler.post(block)
    }

    // ---- reads (any thread)

    fun hardware(): JSONObject {
        val l = loaded
        val m = MemoryBudget.read(app, heldBytes = l?.memBytes ?: 0)
        val devs = devicesOrInit()
        var gpu: String? = null
        for (i in 0 until devs.length()) {
            val d = devs.getJSONObject(i)
            if (d.optString("type") == "gpu" || d.optString("type") == "igpu") gpu = d.optString("desc")
        }
        return JSONObject()
            .put("budget", JSONObject().put("kind", "unified").put("fast_bytes", m.budgetBytes).put("spill_bytes", 0))
            .put("memory", JSONObject().put("total_bytes", m.totalBytes).put("avail_bytes", m.availBytes)
                .put("threshold_bytes", m.thresholdBytes).put("low", m.lowMemory).put("held_bytes", m.heldBytes))
            .put("device", JSONObject().put("manufacturer", Build.MANUFACTURER).put("model", Build.MODEL)
                .put("soc", if (Build.VERSION.SDK_INT >= 31) "${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}" else Build.HARDWARE)
                .put("android", Build.VERSION.RELEASE).put("gpu", gpu ?: JSONObject.NULL))
            .put("engine", engineInfo())
            .put("disk_free_bytes", store.freeBytes())
            .put("running", runningJson() ?: JSONObject.NULL)
    }

    fun engineInfo(): JSONObject {
        val devs = devicesOrInit()
        val hasGpu = (0 until devs.length()).any { devs.getJSONObject(it).optString("type").let { t -> t == "gpu" || t == "igpu" } }
        return JSONObject()
            .put("id", "llama.cpp").put("version", LLAMACPP_VERSION)
            .put("backend", loaded?.backend?.let(::backendId) ?: if (hasGpu) "opencl" else "cpu")
            .put("devices", devs)
            .put("state", if (devs.length() > 0) "ready" else "missing")
    }

    fun list(): JSONObject {
        val models = JSONArray()
        store.list().forEach { models.put(it.toJson()) }
        val dl = JSONArray()
        downloads.active().forEach { dl.put(it) }
        return JSONObject().put("models", models).put("downloads", dl).put("running", runningJson() ?: JSONObject.NULL)
    }

    fun runningJson(): JSONObject? {
        val l = loaded ?: return null
        return JSONObject()
            .put("modelId", l.model.model).put("key", l.model.key).put("quant", l.model.quant)
            .put("memBytes", l.memBytes).put("tokPerSec", l.tokPerSec).put("ctxUsed", l.ctxUsed)
            .put("ctxMax", l.nCtx).put("idleUnloadSec", IDLE_UNLOAD_SEC).put("backend", backendId(l.backend))
    }

    fun stream(id: String): TokenStream? = current.get()?.takeIf { it.id == id }

    // ---- engine thread

    /** Loads [key] (unloading anything else). Engine thread only. */
    fun loadNow(key: String, nCtx: Int, backendPref: String): JSONObject {
        loaded?.let { if (it.model.key == key) return runningJson()!! }
        val model = store.get(key) ?: throw Protocol.Failure("not_installed", "That model is not on this phone.")
        unloadNow("switch")
        val budget = MemoryBudget.read(app).budgetBytes
        val need = MemoryBudget.needBytes(model.size)
        if (need > budget) {
            throw Protocol.Failure("no_memory",
                "Not enough free memory right now: needs ${gb(need)} GB, the phone can give ${gb(budget)} GB. Close other apps and try again.")
        }
        val devs = devicesOrInit()
        val wantGpu = backendPref != "cpu" &&
            (0 until devs.length()).any { devs.getJSONObject(it).optString("type").let { t -> t == "gpu" || t == "igpu" } }
        val t0 = SystemClock.elapsedRealtime()
        var handle = if (wantGpu) NativeLlama.load(model.path.absolutePath, nCtx, 0, true) else 0L
        if (handle == 0L) {
            if (wantGpu) Log.w(TAG, "GPU load failed for $key, using the CPU")
            handle = NativeLlama.load(model.path.absolutePath, nCtx, 0, false)
        }
        if (handle == 0L) throw Protocol.Failure("load_failed", "The model could not be loaded.")
        val info = NativeLlama.info(handle)
        val l = Loaded(handle, model, info.optString("backend", "CPU"), info.optInt("n_ctx", nCtx), need)
        loaded = l
        Log.i(TAG, "loaded $key in ${SystemClock.elapsedRealtime() - t0} ms: $info")
        EngineService.showRunning(app, model.name, model.quant, backendId(l.backend))
        scheduleIdle()
        emitState()
        return runningJson()!!
    }

    fun unloadNow(reason: String) {
        val l = loaded ?: return
        handler.removeCallbacks(idleUnload)
        synchronized(nativeLock) {
            loaded = null
            NativeLlama.free(l.handle)
        }
        Log.i(TAG, "unloaded ${l.model.key} ($reason)")
        EngineService.stopRunning(app)
        emit(Protocol.evt("state", JSONObject().put("running", JSONObject.NULL).put("reason", reason)))
    }

    /** Generates one reply into a [TokenStream]. Engine thread only. */
    fun chatNow(id: String, key: String, args: JSONObject) {
        val stream = TokenStream(id, { sink(it) })
        current.set(stream)
        val l = try {
            loadNow(key, args.optInt("ctx", DEFAULT_CTX), args.optString("backend", "auto"))
            loaded!!
        } catch (f: Protocol.Failure) {
            stream.finish("error", JSONObject().put("error", JSONObject().put("code", f.code).put("message", f.message)))
            return
        }
        val msgs = args.optJSONArray("messages") ?: JSONArray()
        val roles = Array(msgs.length()) { msgs.getJSONObject(it).optString("role", "user") }
        val contents = Array(msgs.length()) { msgs.getJSONObject(it).optString("content", "") }
        handler.removeCallbacks(idleUnload)
        generating = true
        val stats = try {
            NativeLlama.chat(
                l.handle, roles, contents,
                maxTokens = args.optInt("max_tokens", 512).coerceIn(1, 4096),
                temperature = args.optDouble("temperature", 0.7).toFloat(),
                seed = if (args.has("seed")) args.optInt("seed") else -1,
                thinking = args.optBoolean("thinking", false),
            ) { bytes -> stream.append(String(bytes, Charsets.UTF_8)); true }
        } finally {
            generating = false
        }
        l.tokPerSec = stats.optDouble("tok_per_sec", 0.0)
        l.ctxUsed = stats.optInt("ctx_used", 0)
        stats.put("backend", backendId(l.backend))
        val finish = stats.optString("finish", "error")
        if (finish == "error") {
            stats.put("error", JSONObject().put("code", "generate_failed").put("message", stats.optString("error", "Generation failed.")))
        }
        stream.finish(finish, stats)
        scheduleIdle()
        emitState()
    }

    fun stop(id: String): Boolean {
        val s = current.get() ?: return false
        if (s.id != id || s.done) return false
        stopNative()
        return true
    }

    /** Stops any reply at once (any thread), then unloads on the engine thread. */
    fun requestUnload(reason: String) {
        stopNative()
        handler.post { unloadNow(reason) }
    }

    private fun stopNative() {
        synchronized(nativeLock) { loaded?.let { NativeLlama.stop(it.handle) } }
    }

    private fun memoryPressure(why: String) {
        Log.w(TAG, "memory pressure ($why)")
        requestUnload("memory")
    }

    private fun scheduleIdle() {
        handler.removeCallbacks(idleUnload)
        handler.postDelayed(idleUnload, IDLE_UNLOAD_SEC * 1000L)
    }

    private fun emitState() = emit(Protocol.evt("state", JSONObject().put("running", runningJson() ?: JSONObject.NULL)))

    private fun emit(m: String) {
        sink(m)
    }

    @Synchronized
    private fun devicesOrInit(): JSONArray {
        devices?.let { return it }
        val d = try {
            NativeLlama.init(app.applicationInfo.nativeLibraryDir)
        } catch (e: Throwable) {
            Log.e(TAG, "native engine failed to load", e)
            JSONArray()
        }
        val allowed = Build.VERSION.SDK_INT >= 31 && GpuPolicy.openclAllowed(Build.SOC_MANUFACTURER, Build.SOC_MODEL)
        val usable = GpuPolicy.filter(d, allowed)
        if (usable.length() != d.length()) Log.i(TAG, "GPU not used on this SoC (${Build.SOC_MODEL}): CPU backend")
        devices = usable
        return usable
    }

    private fun gb(b: Long) = "%.1f".format(b / 1e9)

    companion object {
        const val TAG = "OnDeviceEngine"
        const val LLAMACPP_VERSION = "b12414"
        const val IDLE_UNLOAD_SEC = 300
        const val DEFAULT_CTX = 2048

        /** "GPUOpenCL" -> "opencl", "CPU" -> "cpu" (DeviceRuntime.backend). */
        fun backendId(native: String): String = when {
            native.contains("opencl", ignoreCase = true) -> "opencl"
            native.equals("CPU", ignoreCase = true) -> "cpu"
            else -> native.lowercase()
        }

        @Volatile private var instance: OnDeviceEngine? = null

        fun get(context: Context): OnDeviceEngine =
            instance ?: synchronized(this) {
                instance ?: OnDeviceEngine(context.applicationContext).also { instance = it }
            }
    }
}
