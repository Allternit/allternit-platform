package com.allternit.feature.chat

import android.content.Context
import android.util.AtomicFile
import android.util.Base64
import java.io.File
import java.security.MessageDigest
import org.json.JSONArray
import org.json.JSONObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Account-private durable turns. PNG bytes land before the atomic turn index is replaced. */
internal class LocalImageStore(context: Context, account: String) {
    private val key = MessageDigest.getInstance("SHA-256").digest(account.toByteArray()).joinToString("") { "%02x".format(it) }
    private val root = File(context.filesDir, "aci-images/$key").apply { mkdirs() }
    private val index = AtomicFile(File(root, "turns.json"))
    private var rows = if (index.baseFile.exists()) JSONArray(index.openRead().bufferedReader().use { it.readText() }) else JSONArray()
    init { activeRoot = root.canonicalFile }
    fun deactivate() { if (activeRoot == root.canonicalFile) activeRoot = null }
    companion object {
        private var activeRoot: File? = null
        fun readImage(path: String): ByteArray {
            val current = checkNotNull(activeRoot)
            val file = File(path).canonicalFile
            check(file.parentFile == current && file.name.matches(Regex("[a-f0-9-]{36}\\.png")))
            check(file.length() in 1..8L * 1024 * 1024)
            return file.readBytes().also { check(activeRoot == current && it.size <= 8 * 1024 * 1024) }
        }
    }

    fun entries(): List<JSONObject> = (0 until rows.length()).map { rows.getJSONObject(it) }
    private fun write() {
        val out = index.startWrite()
        try { out.write(rows.toString().toByteArray()); index.finishWrite(out) }
        catch (e: Exception) { index.failWrite(out); throw e }
    }
    fun begin(r: BrowserImageJob.Request, message: String, user: String?, baseline: List<MessageRecord>) {
        if (entries().any { it.getString("id") == r.id }) return
        val before = rows.toString()
        rows.put(JSONObject().put("id", r.id).put("session", r.session).put("call", r.call)
            .put("prompt", r.prompt).put("message", message).put("user", user).put("remote", r.remote)
            .put("baseline", JSONArray(baseline.filter { m -> m.id != message && m.id != user && entries().none { it.optString("message") == m.id || it.optString("user") == m.id } }.map {
                JSONObject().put("id", it.id).put("role", it.role).put("content", it.content)
            })))
        try { write() } catch (e: Exception) { rows = JSONArray(before); throw e }
    }
    fun complete(id: String, result: JSONObject): JSONObject {
        val row = entries().first { it.getString("id") == id }
        val saved = JSONObject(result.toString())
        if (result.optBoolean("ok")) {
            val file = File(root, "${java.util.UUID.randomUUID()}.png")
            val png = AtomicFile(file)
            val out = png.startWrite()
            try {
                out.write(Base64.decode(result.getString("image").substringAfter(','), Base64.DEFAULT)); png.finishWrite(out)
            } catch (e: Exception) { png.failWrite(out); throw e }
            saved.remove("image"); saved.put("localImage", file.absolutePath)
        }
        val previous = row.optJSONObject("result")
        row.put("result", saved)
        try { write() } catch (e: Exception) { if (previous == null) row.remove("result") else row.put("result", previous); throw e }
        return saved
    }
    fun messages(session: String, liveCall: String? = null): List<MessageRecord> {
        val turns = entries().filter { it.getString("session") == session }
        val base = turns.firstOrNull()?.optJSONArray("baseline")
        val result = mutableListOf<MessageRecord>()
        if (base != null) for (i in 0 until base.length()) base.getJSONObject(i).let { result += MessageRecord(it.getString("id"), it.getString("role"), it.getString("content")) }
        turns.forEach { row ->
            row.optString("user").takeIf { it.isNotBlank() }?.let { result += MessageRecord(it, "user", row.getString("prompt")) }
            result += message(row, liveCall)
        }
        return result.distinctBy { it.id }
    }
    fun message(row: JSONObject, liveCall: String? = null): MessageRecord {
        val payload = row.optJSONObject("result") ?: BrowserImageJob.error("ambiguous_submission", "This request was interrupted. Check ChatGPT before making another image; it will not be sent again.")
        val running = !row.has("result") && row.getString("call") == liveCall
        val call = ToolCallRecord(row.getString("call"), "media_generate", if (running) ToolState.Running else if (payload.optBoolean("ok")) ToolState.Done else ToolState.Failed,
            output = payload.toString(), browserImageResult = payload.toString(), error = payload.optString("error").takeIf { !running && it.isNotBlank() },
            input = buildJsonObject { put("kind", "image"); put("engine", "chatgpt-aci"); put("prompt", row.getString("prompt")) })
        return MessageRecord(row.getString("message"), "assistant", "", isStreaming = running, toolCalls = listOf(call))
    }
    fun merge(session: String, remote: List<MessageRecord>, liveCall: String? = null): List<MessageRecord> {
        val local = messages(session, liveCall)
        val replacements = local.filter { it.toolCalls.isNotEmpty() }.associateBy { it.id }
        return remote.map { replacements[it.id]?.let { image -> it.copy(isStreaming = it.isStreaming || image.isStreaming, toolCalls = it.toolCalls.filterNot { c -> image.toolCalls.any { x -> x.id == c.id } } + image.toolCalls) } ?: it } + local.filter { l -> remote.none { it.id == l.id } }
    }
    fun delete(session: String) {
        val kept = entries().filterNot { it.getString("session") == session }
        entries().filter { it.getString("session") == session }.forEach { it.optJSONObject("result")?.optString("localImage")?.takeIf { p -> File(p).parentFile == root }?.let { p -> File(p).delete() } }
        rows = JSONArray(kept); write()
    }
}
