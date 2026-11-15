package com.allternit.ondevice

import android.content.Context
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.security.MessageDigest

/**
 * Installed models, in app-private storage: files/ondevice/models/<slot>/<file> plus
 * meta.json. Nothing here is readable by other apps, and it is excluded from backups
 * (the app sets allowBackup=false). A model is only listed once its sha256 matched the
 * Freeweights manifest.
 */
class ModelStore(context: Context) {
    private val root = File(context.filesDir, "ondevice/models").apply { mkdirs() }

    data class Installed(
        val key: String,
        val model: String,
        val quant: String,
        val repo: String,
        val file: String,
        val name: String,
        val size: Long,
        val sha256: String,
        val installedAt: Long,
        val path: File,
    ) {
        fun toJson(): JSONObject = JSONObject()
            .put("key", key).put("model", model).put("quant", quant).put("repo", repo)
            .put("file", file).put("name", name).put("size", size).put("sha256", sha256)
            .put("installed_at", installedAt).put("verified", true)
    }

    fun list(): List<Installed> = root.listFiles().orEmpty().mapNotNull { read(it) }.sortedBy { it.name }

    fun get(key: String): Installed? = read(slotDir(key))

    fun delete(key: String): Boolean = slotDir(key).let { it.exists() && it.deleteRecursively() }

    fun freeBytes(): Long = root.usableSpace

    /** Moves a verified download into its slot and records it. */
    fun install(key: String, model: String, quant: String, repo: String, file: String, name: String,
                size: Long, sha256: String, downloaded: File): Installed {
        val dir = slotDir(key)
        dir.deleteRecursively()
        dir.mkdirs()
        val dest = File(dir, File(file).name)
        if (!downloaded.renameTo(dest)) {
            // External app dir and files/ can be different mounts.
            downloaded.inputStream().use { input -> dest.outputStream().use { input.copyTo(it, 1 shl 20) } }
            downloaded.delete()
        }
        val meta = Installed(key, model, quant, repo, file, name, size, sha256, System.currentTimeMillis(), dest)
        File(dir, "meta.json").writeText(meta.toJson().toString())
        return meta
    }

    private fun read(dir: File): Installed? {
        val meta = File(dir, "meta.json")
        if (!meta.isFile) return null
        return try {
            val j = JSONObject(meta.readText())
            val f = File(dir, File(j.getString("file")).name)
            if (!f.isFile || f.length() != j.getLong("size")) return null
            Installed(
                j.getString("key"), j.getString("model"), j.getString("quant"), j.getString("repo"),
                j.getString("file"), j.optString("name", j.getString("model")), j.getLong("size"),
                j.getString("sha256"), j.optLong("installed_at"), f,
            )
        } catch (e: Exception) {
            null
        }
    }

    private fun slotDir(key: String) = File(root, slotName(key))

    companion object {
        fun keyOf(model: String, quant: String) = "$model#$quant"

        /** A directory name for a key: readable prefix + hash, so different keys never collide. */
        fun slotName(key: String): String {
            val safe = key.replace(Regex("[^A-Za-z0-9._-]"), "_").take(80)
            return safe + "-" + sha256Hex(key.byteInputStream()).take(12)
        }

        fun sha256Hex(input: InputStream): String {
            val md = MessageDigest.getInstance("SHA-256")
            val buf = ByteArray(1 shl 20)
            input.use {
                while (true) {
                    val n = it.read(buf)
                    if (n < 0) break
                    md.update(buf, 0, n)
                }
            }
            return md.digest().joinToString("") { "%02x".format(it) }
        }
    }
}
