package com.allternit.feature.chat.voice

import android.content.Context
import android.net.ConnectivityManager
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import okhttp3.OkHttpClient
import okhttp3.Request
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream
import java.io.BufferedInputStream
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.IOException
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/*
 * iOS Core/Voice/VoicePackManager on Android: the on-phone voice packs, downloaded on first use into the app's files
 * (never in the APK), sha256-checked, resumable.
 *
 *  - Listening: Silero VAD + Moonshine tiny EN (~31 MB). Same files and sha256 pins as iOS and the native engine
 *    (services/voice/src/models.rs `small`, minus Smart Turn, which phones don't run).
 *  - Replies: Kokoro-82M v1.0 (~350 MB). iOS runs Kokoro on MLX from its own fp16 safetensors pack
 *    (voice-models/ios/v1); MLX is Apple-only, so Android runs the ONNX pack the native engine runs
 *    (services/voice `tts`, kokoro-multi-lang-v1_0, same sha256) on sherpa-onnx.
 *
 * Files come from our mirror (runtime.allternit.com/voice-packs/v1/<file>, flat, scripts/mirror-voice-packs.sh), then
 * the upstream k2-fsa release, the same order as the native engine.
 */

/** iOS VoicePackFile. [asset] is the upstream release path; the mirror serves it by file name. */
class VoicePackFile(val asset: String, val sha256: String, val size: Long) {
    val fileName: String get() = asset.substringAfterLast('/')
    val isArchive: Boolean get() = asset.endsWith(".tar.bz2")
}

/** iOS VoicePack: its folder under files/voice, its files and the paths that must exist once installed. */
class VoicePack(val dir: String, val files: List<VoicePackFile>, val required: List<String>) {
    val totalBytes: Long get() = files.sumOf { it.size }

    companion object {
        const val MOONSHINE_DIR = "sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27"
        const val KOKORO_DIR = "kokoro-multi-lang-v1_0"
        const val MIRROR_BASE = "https://runtime.allternit.com/voice-packs/v1"
        const val UPSTREAM_BASE = "https://github.com/k2-fsa/sherpa-onnx/releases/download"

        val vad = VoicePackFile("asr-models/silero_vad.onnx", "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6", 643_854)
        val moonshine = VoicePackFile(
            "asr-models/$MOONSHINE_DIR.tar.bz2", "9ec31b342d8fa3240c3b81b8f82e1cf7e3ac467c93ca5a999b741d5887164f8d", 29_858_559,
        )
        val kokoroArchive = VoicePackFile(
            "tts-models/$KOKORO_DIR.tar.bz2", "c5f7e2d2caf082bc1d20fb70334a61d99d20b484500aad32e7cf84c128ea3298", 349_906_910,
        )

        /** Listening: Silero VAD + Moonshine. */
        val listening = VoicePack(
            "small", listOf(vad, moonshine),
            listOf("silero_vad.onnx") + listOf("encoder_model.ort", "decoder_model_merged.ort", "tokens.txt").map { "$MOONSHINE_DIR/$it" },
        )

        /** Replies: Kokoro-82M ONNX + voices + espeak-ng data + the English lexicon. */
        val kokoro = VoicePack(
            "kokoro-onnx-v1", listOf(kokoroArchive),
            listOf("model.onnx", "voices.bin", "tokens.txt", "espeak-ng-data").map { "$KOKORO_DIR/$it" },
        )
    }
}

/** iOS VoicePackState, plus [Paused] (a download stopped with its partial file kept, to resume later). */
sealed interface VoicePackState {
    data object Missing : VoicePackState
    data class Downloading(val progress: Double) : VoicePackState
    data class Paused(val progress: Double) : VoicePackState
    data object Ready : VoicePackState
    data class Failed(val message: String) : VoicePackState
}

/** Model paths for the listening engine (iOS VoicePackPaths). */
data class VoicePackPaths(val vad: String, val encoder: String, val decoder: String, val tokens: String)

/** Model paths for Kokoro. */
data class KokoroPaths(val model: String, val voices: String, val tokens: String, val dataDir: String, val lexicon: String?)

private class PackCancelled(val keepPartial: Boolean) : Exception()

private class PackError(message: String) : Exception(message)

/** iOS VoicePackManager. [state] is Compose state, so Settings and voice mode follow it. */
class VoicePackManager private constructor(val pack: VoicePack) {
    var state by mutableStateOf<VoicePackState>(VoicePackState.Missing)
        private set

    private val main = Handler(Looper.getMainLooper())
    private val lock = Any()
    private var running: CountDownLatch? = null
    @Volatile private var stopRequest: Boolean? = null
    private val progressListeners = mutableListOf<(Double) -> Unit>()

    fun root(context: Context): File = File(context.applicationContext.filesDir, "voice/${pack.dir}")

    fun isInstalled(context: Context): Boolean {
        val root = root(context)
        return File(root, ".ready").exists() && pack.required.all { File(root, it).exists() }
    }

    /** Re-reads the disk (Settings open): Ready, Paused with a partial file, or Missing. Leaves a live download alone. */
    fun refresh(context: Context) {
        synchronized(lock) { if (running != null) return }
        val next = when {
            isInstalled(context) -> VoicePackState.Ready
            partialBytes(context) > 0 -> VoicePackState.Paused(partialBytes(context).toDouble() / pack.totalBytes)
            else -> (state as? VoicePackState.Failed) ?: VoicePackState.Missing
        }
        postState(next)
    }

    /** Bytes the pack holds on this phone (installed files plus any partial download). */
    fun bytesOnDisk(context: Context): Long = root(context).walkBottomUp().filter { it.isFile }.sumOf { it.length() }

    private fun partialBytes(context: Context): Long =
        root(context).listFiles()?.filter { it.name.endsWith(".part") }?.sumOf { it.length() } ?: 0L

    /** Starts (or resumes) the download in the background. `cellular = false` waits for an unmetered network (iOS). */
    fun install(context: Context, cellular: Boolean = true) {
        val app = context.applicationContext
        synchronized(lock) { if (running != null || isInstalled(app)) return }
        Thread({ installBlocking(app, cellular) }, "voice-pack-${pack.dir}").apply { isDaemon = true }.start()
    }

    /**
     * Downloads and installs the pack on the calling thread; true once installed. A second caller while one runs waits
     * for the same download (iOS install() is idempotent). [onProgress] gets 0…1 from any thread.
     */
    fun installBlocking(context: Context, cellular: Boolean = true, onProgress: ((Double) -> Unit)? = null): Boolean {
        val app = context.applicationContext
        if (isInstalled(app)) { postState(VoicePackState.Ready); return true }
        val mine = CountDownLatch(1)
        val wait: CountDownLatch? = synchronized(lock) {
            onProgress?.let { progressListeners.add(it) }
            val other = running
            if (other == null) { running = mine; stopRequest = null }
            other
        }
        if (wait != null) {
            wait.await()
            synchronized(lock) { onProgress?.let { progressListeners.remove(it) } }
            return isInstalled(app)
        }
        postState(VoicePackState.Downloading(0.0))
        var last = 0.0
        val result = try {
            download(app, cellular) { p ->
                last = p
                val listeners = synchronized(lock) { progressListeners.toList() }
                listeners.forEach { it(p) }
                postState(VoicePackState.Downloading(p))
            }
            postState(VoicePackState.Ready)
            true
        } catch (e: PackCancelled) {
            if (e.keepPartial) postState(VoicePackState.Paused(last))
            else { root(app).listFiles()?.filter { it.name.endsWith(".part") }?.forEach { it.delete() }; postState(VoicePackState.Missing) }
            false
        } catch (e: Exception) {
            postState(VoicePackState.Failed(e.message ?: "The voice model download failed."))
            false
        } finally {
            synchronized(lock) {
                running = null
                stopRequest = null
                onProgress?.let { progressListeners.remove(it) }
            }
            mine.countDown()
        }
        return result
    }

    /** Stops the download and keeps what arrived (Resume picks it up). */
    fun pause() { synchronized(lock) { if (running != null) stopRequest = true } }

    /** Stops the download and drops the partial file (iOS cancel). */
    fun cancel(context: Context) {
        val live = synchronized(lock) { if (running != null) { stopRequest = false; true } else false }
        if (!live) {
            root(context).listFiles()?.filter { it.name.endsWith(".part") }?.forEach { it.delete() }
            postState(VoicePackState.Missing)
        }
    }

    /** Removes the pack (Settings › free the space; iOS remove). */
    fun remove(context: Context) {
        val app = context.applicationContext
        val live = synchronized(lock) { if (running != null) { stopRequest = false; running } else null }
        Thread({
            live?.await(30, TimeUnit.SECONDS)
            root(app).deleteRecursively()
            postState(VoicePackState.Missing)
        }, "voice-pack-remove").apply { isDaemon = true }.start()
    }

    /** iOS VoicePackManager.paths: nil until the listening pack is installed. */
    fun listeningPaths(context: Context): VoicePackPaths? {
        if (pack !== VoicePack.listening || !isInstalled(context)) return null
        val root = root(context)
        val model = File(root, VoicePack.MOONSHINE_DIR)
        return VoicePackPaths(
            vad = File(root, "silero_vad.onnx").path,
            encoder = File(model, "encoder_model.ort").path,
            decoder = File(model, "decoder_model_merged.ort").path,
            tokens = File(model, "tokens.txt").path,
        )
    }

    fun kokoroPaths(context: Context): KokoroPaths? {
        if (pack !== VoicePack.kokoro || !isInstalled(context)) return null
        val dir = File(root(context), VoicePack.KOKORO_DIR)
        return KokoroPaths(
            model = File(dir, "model.onnx").path,
            voices = File(dir, "voices.bin").path,
            tokens = File(dir, "tokens.txt").path,
            dataDir = File(dir, "espeak-ng-data").path,
            lexicon = File(dir, "lexicon-us-en.txt").takeIf { it.exists() }?.path,
        )
    }

    private fun postState(next: VoicePackState) {
        if (Looper.myLooper() == Looper.getMainLooper()) state = next else main.post { state = next }
    }

    private fun checkStop() {
        stopRequest?.let { throw PackCancelled(keepPartial = it) }
    }

    // MARK: - Download

    private fun download(context: Context, cellular: Boolean, progress: (Double) -> Unit) {
        val root = root(context)
        if (!root.exists() && !root.mkdirs()) throw PackError("There isn't room on this phone for the voice model.")
        File(root, ".ready").delete()
        if (!cellular) awaitUnmetered(context)
        val total = pack.totalBytes.toDouble()
        var finished = 0L
        var lastTick = 0L
        for (file in pack.files) {
            checkStop()
            val target = File(root, file.fileName)
            // Resume a pack that was cut off: keep plain files already verified (iOS).
            if (!file.isArchive && target.length() == file.size && sha256(target) == file.sha256) {
                finished += file.size
                progress(finished / total)
                continue
            }
            val part = File(root, "${file.fileName}.part")
            val base = finished
            fetch(file, part) { written ->
                val now = System.currentTimeMillis()
                if (now - lastTick > 250) { lastTick = now; progress(((base + written) / total).coerceAtMost(1.0)) }
            }
            if (part.length() != file.size || sha256(part) != file.sha256) {
                part.delete()
                throw PackError("The voice model ${file.fileName} failed its integrity check.")
            }
            if (file.isArchive) {
                try { extract(part, root) } finally { part.delete() }
            } else {
                target.delete()
                if (!part.renameTo(target)) throw PackError("The voice model could not be saved.")
            }
            finished += file.size
            progress(finished / total)
        }
        if (!pack.required.all { File(root, it).exists() }) throw PackError("The voice model download was incomplete.")
        File(root, ".ready").createNewFile()
    }

    /** Mirror first, then upstream. Resumes [part] with a Range request when it already holds bytes. */
    private fun fetch(file: VoicePackFile, part: File, onWritten: (Long) -> Unit) {
        var lastError: Exception? = null
        for (url in listOf("${VoicePack.MIRROR_BASE}/${file.fileName}", "${VoicePack.UPSTREAM_BASE}/${file.asset}")) {
            checkStop()
            try {
                fetchOne(url, file, part, onWritten)
                if (part.length() == file.size && sha256(part) == file.sha256) return
                // A mirror or upstream copy that doesn't match the pin: start over from the next source.
                part.delete()
                lastError = PackError("The voice model ${file.fileName} failed its integrity check.")
            } catch (e: PackCancelled) {
                throw e
            } catch (e: Exception) {
                lastError = e
            }
        }
        throw lastError ?: PackError("The voice model download failed.")
    }

    private fun fetchOne(url: String, file: VoicePackFile, part: File, onWritten: (Long) -> Unit) {
        val have = part.length().takeIf { it in 1 until file.size } ?: 0L
        if (have == 0L) part.delete()
        val request = Request.Builder().url(url).apply { if (have > 0) header("Range", "bytes=$have-") }.build()
        http.newCall(request).execute().use { r ->
            val append = r.code == 206 && have > 0
            if (r.code != 200 && !append) {
                throw PackError(if (r.code == 416) "The voice model download failed (HTTP 416)." else "The voice model download failed (HTTP ${r.code}).")
            }
            val body = r.body ?: throw PackError("The voice model download failed.")
            var written = if (append) have else 0L
            FileOutputStream(part, append).use { out ->
                body.byteStream().use { input ->
                    val buf = ByteArray(1 shl 16)
                    while (true) {
                        checkStop()
                        val n = input.read(buf)
                        if (n < 0) break
                        out.write(buf, 0, n)
                        written += n
                        onWritten(written)
                    }
                }
            }
        }
    }

    /** Unpacks a .tar.bz2 into a staging folder, then moves its top-level folders into [root] (iOS Bz2Tar). */
    private fun extract(archive: File, root: File) {
        val staging = File(root, ".staging")
        staging.deleteRecursively()
        staging.mkdirs()
        val stagingPath = staging.canonicalPath + File.separator
        TarArchiveInputStream(BZip2CompressorInputStream(BufferedInputStream(FileInputStream(archive), 1 shl 16))).use { tar ->
            while (true) {
                val entry = tar.nextEntry ?: break
                checkStop()
                if (entry.isSymbolicLink || entry.isLink) continue
                val out = File(staging, entry.name).canonicalFile
                if (!out.path.startsWith(stagingPath)) throw PackError("The voice model archive is malformed.")
                if (entry.isDirectory) { out.mkdirs(); continue }
                out.parentFile?.mkdirs()
                FileOutputStream(out).use { tar.copyTo(it, 1 shl 16) }
            }
        }
        staging.listFiles()?.forEach { child ->
            val dest = File(root, child.name)
            dest.deleteRecursively()
            if (!child.renameTo(dest)) throw IOException("Could not move ${child.name}")
        }
        staging.deleteRecursively()
    }

    /** iOS `allowsExpensiveNetworkAccess = false` + `waitsForConnectivity`: hold until the network isn't metered. */
    private fun awaitUnmetered(context: Context) {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return
        while (cm.activeNetwork == null || cm.isActiveNetworkMetered) {
            checkStop()
            Thread.sleep(5_000)
        }
    }

    companion object {
        /** Listening: Silero VAD + Moonshine (iOS VoicePackManager.shared). */
        val listening = VoicePackManager(VoicePack.listening)
        /** Kokoro reply voice (iOS VoicePackManager.kokoro). */
        val kokoro = VoicePackManager(VoicePack.kokoro)

        private val http: OkHttpClient by lazy {
            OkHttpClient.Builder().connectTimeout(20, TimeUnit.SECONDS).readTimeout(60, TimeUnit.SECONDS).build()
        }

        fun sha256(file: File): String {
            val digest = MessageDigest.getInstance("SHA-256")
            FileInputStream(file).use { input ->
                val buf = ByteArray(1 shl 20)
                while (true) { val n = input.read(buf); if (n < 0) break; digest.update(buf, 0, n) }
            }
            return digest.digest().joinToString("") { "%02x".format(it) }
        }

        /** "350 MB" for a settings row. */
        fun megabytes(bytes: Long): String = "${(bytes + 999_999) / 1_000_000} MB"
    }
}
