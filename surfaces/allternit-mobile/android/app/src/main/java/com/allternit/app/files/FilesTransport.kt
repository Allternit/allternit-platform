package com.allternit.app.files

import com.allternit.files.contract.ChangesResponse
import com.allternit.files.contract.ChunkPutResponse
import com.allternit.files.contract.ChunkRef
import com.allternit.files.contract.DeleteRequest
import com.allternit.files.contract.DeleteResponse
import com.allternit.files.contract.EndpointsResponse
import com.allternit.files.contract.ErrorBody
import com.allternit.files.contract.ErrorCode
import com.allternit.files.contract.ListResponse
import com.allternit.files.contract.MkdirRequest
import com.allternit.files.contract.MoveRequest
import com.allternit.files.contract.MutationResponse
import com.allternit.files.contract.RootsResponse
import com.allternit.files.contract.SmallWriteQuery
import com.allternit.files.contract.StatResponse
import com.allternit.files.contract.UpdateWakeSettingsRequest
import com.allternit.files.contract.UploadChunksAppend
import com.allternit.files.contract.UploadCommitResponse
import com.allternit.files.contract.UploadCreateRequest
import com.allternit.files.contract.UploadStatus
import com.allternit.files.contract.WakeConsentRequest
import com.allternit.files.contract.WakeConsentResponse
import com.allternit.files.contract.WakeSetting
import com.allternit.files.contract.WakeSettings
import org.json.JSONObject
import java.io.File
import java.io.FilterOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.io.RandomAccessFile
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/** A Files error with its contract code (SPEC section 16). `status` 0 = no answer (network). */
internal class FilesException(
    val status: Int,
    val code: ErrorCode?,
    val body: ErrorBody?,
    message: String,
) : IOException(message) {
    val needsWake get() = code == ErrorCode.WAKE_CONSENT_REQUIRED
}

/** One of the user's computers, from cloud-api `GET /api/v1/runtime-devices`. */
internal data class Computer(
    val id: String,
    val name: String,
    val runtimeType: String,
    val status: String,
    val wakeable: Boolean,
)

/** cloud-api calls that are not relayed (computer list, endpoints, wake). */
internal class CloudApi(private val token: String) {

    fun computers(): List<Computer> {
        val text = request("GET", "/api/v1/runtime-devices", null)
        val arr = JSONObject(text).optJSONArray("runtimes") ?: return emptyList()
        val out = ArrayList<Computer>()
        for (i in 0 until arr.length()) {
            val o = arr.getJSONObject(i)
            val type = o.optString("runtimeType")
            // Phones and tablets are clients of the drive, not computers with files.
            if (type in setOf("ios", "android", "ipados")) continue
            val caps = o.optJSONArray("capabilities")
            val hasFiles = caps == null || (0 until caps.length()).any { caps.optString(it) == "runtime:files" }
            if (!hasFiles) continue
            out.add(
                Computer(
                    id = o.getString("id"),
                    name = o.optString("name").ifBlank { "Computer" },
                    runtimeType = type,
                    status = o.optString("status"),
                    wakeable = o.optBoolean("wakeable", false),
                ),
            )
        }
        return out
    }

    fun endpoints(computerId: String): EndpointsResponse =
        request("GET", "/api/v1/computer-files/${seg(computerId)}/endpoints", null).wire()

    fun wakeConsent(computerId: String, body: WakeConsentRequest): WakeConsentResponse =
        request("POST", "/api/v1/computer-files/${seg(computerId)}/wake-consent", WireJson.encode(body).toString()).wire()

    /**
     * SPEC section 12: a signed, 5-minute protected-root unlock token (`fut1`) for this
     * device, as `(token, expires_at)`. Call only right after a biometric success.
     */
    fun unlock(computerId: String, deviceId: String): Pair<String, String> {
        val o = JSONObject(request("POST", "/api/v1/computer-files/${seg(computerId)}/unlock", JSONObject().put("device_id", deviceId).toString()))
        return o.getString("token") to o.getString("expires_at")
    }

    fun setWake(computerId: String, wake: WakeSetting): WakeSettings =
        request("PUT", "/api/v1/computer-files/${seg(computerId)}/wake-settings", WireJson.encode(UpdateWakeSettingsRequest(wake)).toString()).wire()

    private fun request(method: String, path: String, json: String?): String {
        val c = open(URL(FilesConfig.CLOUD_API + path), method, token)
        try {
            if (json != null) {
                c.doOutput = true
                c.setRequestProperty("Content-Type", "application/json")
                c.outputStream.use { it.write(json.toByteArray(Charsets.UTF_8)) }
            }
            return readOrThrow(c)
        } finally {
            c.disconnect()
        }
    }

    /**
     * A relayed request (SPEC section 2 "Relay"): the cloud-api `RelayRequest`
     * `{method, path, headers, body, bodyEncoding}` posted to the relay candidate's
     * `base_url` (`/api/v1/runtime-devices/:id/proxy`).
     *
     * The encoding key is `bodyEncoding` and only that: cloud-api's `BrowserProxyRequest`
     * is camelCase, so `body_encoding` alone is dropped today and a base64 upload would
     * land as base64 text. The contract fix (SPEC gap 7) adds `body_encoding` as a serde
     * alias, and serde refuses a request that carries both spellings, so sending one key
     * is the only form that works before and after it. The body is streamed: binary
     * bodies are base64-encoded straight from the file into the request, never held whole.
     * The response is the runtime's raw response (real status, raw bytes), so the
     * caller reads [HttpURLConnection.getInputStream] and must disconnect.
     */
    fun relay(
        baseUrl: String,
        method: String,
        path: String,
        headers: Map<String, String>,
        body: RelayBody?,
        readTimeoutMs: Int = 35_000,
    ): HttpURLConnection {
        val c = open(URL(baseUrl), "POST", token)
        c.readTimeout = readTimeoutMs
        c.doOutput = true
        c.setRequestProperty("Content-Type", "application/json")
        c.setRequestProperty("Accept", "*/*")
        c.setChunkedStreamingMode(64 * 1024)
        try {
            c.outputStream.buffered(64 * 1024).use { out ->
                val head = StringBuilder()
                head.append("{\"method\":").append(JSONObject.quote(method))
                head.append(",\"path\":").append(JSONObject.quote(path))
                head.append(",\"headers\":").append(JSONObject(headers as Map<*, *>).toString())
                when (body) {
                    null -> head.append(",\"body\":\"\",\"bodyEncoding\":\"utf8\"}")
                    is RelayBody.Text -> head.append(",\"body\":").append(JSONObject.quote(body.text)).append(",\"bodyEncoding\":\"utf8\"}")
                    is RelayBody.FileRange -> head.append(",\"bodyEncoding\":\"base64\",\"body\":\"")
                }
                out.write(head.toString().toByteArray(Charsets.UTF_8))
                if (body is RelayBody.FileRange) {
                    val b64 = java.util.Base64.getEncoder().wrap(NonClosing(out))
                    RandomAccessFile(body.file, "r").use { raf ->
                        raf.seek(body.offset)
                        val buf = ByteArray(64 * 1024)
                        var left = body.length
                        while (left > 0) {
                            val r = raf.read(buf, 0, minOf(buf.size.toLong(), left).toInt())
                            if (r < 0) throw IOException("File shrank while uploading")
                            b64.write(buf, 0, r)
                            left -= r
                        }
                    }
                    b64.close() // writes the base64 padding; NonClosing keeps `out` open
                    out.write("\"}".toByteArray(Charsets.UTF_8))
                }
            }
            c.responseCode // send + read the status line
            return c
        } catch (e: IOException) {
            c.disconnect()
            throw if (e is FilesException) e else FilesException(0, null, null, e.message ?: "Network error")
        }
    }

    sealed class RelayBody {
        class Text(val text: String) : RelayBody()
        class FileRange(val file: File, val offset: Long, val length: Long) : RelayBody()
    }

    private class NonClosing(out: OutputStream) : FilterOutputStream(out) {
        override fun write(b: ByteArray, off: Int, len: Int) = out.write(b, off, len)
        override fun close() = out.flush()
    }

    companion object {
        fun seg(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

        fun open(url: URL, method: String, token: String): HttpURLConnection =
            (url.openConnection() as HttpURLConnection).apply {
                requestMethod = method
                connectTimeout = 10_000
                readTimeout = 30_000
                setRequestProperty("Authorization", "Bearer $token")
                setRequestProperty("Accept", "application/json")
            }

        /** Body text for 2xx, else a [FilesException] carrying the contract error. */
        fun readOrThrow(c: HttpURLConnection): String {
            val status = try {
                c.responseCode
            } catch (e: IOException) {
                throw FilesException(0, null, null, e.message ?: "Network error")
            }
            if (status in 200..299) return c.inputStream.use { it.readBytes().toString(Charsets.UTF_8) }
            throw errorFrom(c, status)
        }

        fun errorFrom(c: HttpURLConnection, status: Int): FilesException {
            val text = try {
                c.errorStream?.use { it.readBytes().toString(Charsets.UTF_8) } ?: ""
            } catch (e: IOException) {
                ""
            }
            val body = try {
                WireJson.decode(text, ErrorBody::class.java)
            } catch (e: Exception) {
                null
            }
            // cloud-api's own errors are the same flat {error, message, code}; anything else is HTTP.
            val loose: JSONObject? = try {
                JSONObject(text)
            } catch (e: Exception) {
                null
            }
            var code: ErrorCode? = body?.code
            if (code == null && loose != null) code = ErrorCode.fromWire(loose.optString("code").ifBlank { loose.optString("error") })
            if (code == null) {
                code = when (status) {
                    401 -> ErrorCode.UNAUTHORIZED
                    403 -> ErrorCode.FORBIDDEN
                    404 -> ErrorCode.NOT_FOUND
                    else -> null
                }
            }
            val msg = body?.message ?: loose?.optString("message")?.takeIf { it.isNotBlank() } ?: "HTTP $status"
            return FilesException(status, code, body, msg)
        }
    }
}

/**
 * The v2 runtime routes of one computer (SPEC section 2), over the relay. Every
 * request carries this phone's device id and name, the computer's wake ticket when
 * the user consented (section 9) and the unlock proof after a biometric unlock
 * (section 12). A `waking` answer is retried for up to a minute.
 */
internal class FilesClient(
    private val api: CloudApi,
    val computerId: String,
    private val relayUrl: String,
    private val headers: () -> Map<String, String>,
) {
    private fun q(vararg pairs: Pair<String, Any?>): String {
        val parts = pairs.filter { it.second != null }.map { (k, v) -> "$k=" + URLEncoder.encode(v.toString(), "UTF-8").replace("+", "%20") }
        return if (parts.isEmpty()) "" else "?" + parts.joinToString("&")
    }

    private fun seg(s: String) = CloudApi.seg(s)

    /** Runs one relayed request, retrying `waking` (503 + retry_after_s) for up to 60 s. */
    fun raw(
        method: String,
        path: String,
        body: CloudApi.RelayBody? = null,
        extra: Map<String, String> = emptyMap(),
        readTimeoutMs: Int = 35_000,
    ): HttpURLConnection {
        val deadline = System.currentTimeMillis() + 60_000
        while (true) {
            val h = HashMap(headers())
            h.putAll(extra)
            if (body is CloudApi.RelayBody.FileRange) h["content-type"] = "application/octet-stream"
            if (body is CloudApi.RelayBody.Text) h["content-type"] = "application/json"
            val c = api.relay(relayUrl, method, FilesConfig.V2 + path, h, body, readTimeoutMs)
            val status = c.responseCode
            if (status in 200..399) return c
            val err = CloudApi.errorFrom(c, status)
            c.disconnect()
            if (err.code == ErrorCode.WAKING && System.currentTimeMillis() < deadline) {
                Thread.sleep(((err.body?.retryAfterS ?: 3L).coerceIn(1L, 10L)) * 1000)
                continue
            }
            throw err
        }
    }

    private fun text(method: String, path: String, body: CloudApi.RelayBody? = null, readTimeoutMs: Int = 35_000): String {
        val c = raw(method, path, body, readTimeoutMs = readTimeoutMs)
        try {
            return c.inputStream.use { it.readBytes().toString(Charsets.UTF_8) }
        } finally {
            c.disconnect()
        }
    }

    private fun json(o: Any) = CloudApi.RelayBody.Text(WireJson.encode(o).toString())

    fun rootsText(): String = text("GET", "/roots")
    fun roots(): RootsResponse = rootsText().wire()

    fun listText(rootId: String, parentId: String, pageToken: String?, limit: Int = 500): String =
        text("GET", "/roots/${seg(rootId)}/list" + q("parent_id" to parentId, "page_token" to pageToken, "limit" to limit))

    fun list(rootId: String, parentId: String, pageToken: String?): ListResponse = listText(rootId, parentId, pageToken).wire()

    fun stat(itemId: String): StatResponse = text("GET", "/items/${seg(itemId)}").wire()

    fun changes(rootId: String, cursor: String, waitS: Int): ChangesResponse =
        text("GET", "/roots/${seg(rootId)}/changes" + q("cursor" to cursor, "limit" to 500, "wait_s" to waitS), readTimeoutMs = (waitS + 15) * 1000).wire()

    /** Content bytes (SPEC: `ETag: "<rev>"`, 206 for ranges). The caller disconnects. */
    fun content(itemId: String, from: Long? = null, to: Long? = null): HttpURLConnection {
        val extra = HashMap<String, String>()
        if (from != null) extra["range"] = "bytes=$from-" + (to?.toString() ?: "")
        return raw("GET", "/items/${seg(itemId)}/content", extra = extra, readTimeoutMs = 60_000)
    }

    fun smallWrite(rootId: String, query: SmallWriteQuery, file: File): MutationResponse =
        text(
            "PUT",
            "/roots/${seg(rootId)}/content" + q(
                "parent_id" to query.parentId,
                "name" to query.name,
                "base_rev" to query.baseRev,
                "on_conflict" to query.onConflict?.wire,
                "device_name" to query.deviceName,
                "utc_offset_minutes" to query.utcOffsetMinutes,
            ),
            CloudApi.RelayBody.FileRange(file, 0, file.length()),
            readTimeoutMs = 120_000,
        ).wire()

    fun uploadCreate(req: UploadCreateRequest): UploadStatus = text("POST", "/uploads", json(req)).wire()
    fun uploadAppend(sessionId: String, refs: List<ChunkRef>): UploadStatus =
        text("POST", "/uploads/${seg(sessionId)}/chunks", json(UploadChunksAppend(refs))).wire()
    fun uploadStatus(sessionId: String): UploadStatus = text("GET", "/uploads/${seg(sessionId)}").wire()
    fun putChunk(sessionId: String, ref: ChunkRef, file: File): ChunkPutResponse =
        text("PUT", "/uploads/${seg(sessionId)}/chunks/${seg(ref.hash)}", CloudApi.RelayBody.FileRange(file, ref.offset, ref.size), readTimeoutMs = 120_000).wire()
    fun uploadCommit(sessionId: String): UploadCommitResponse = text("POST", "/uploads/${seg(sessionId)}/commit", readTimeoutMs = 120_000).wire()

    // Inbox approval tasks (docs/api/files-drive-inbox.mdx). The Task shape is runtime-only, so the
    // caller reads the JSON itself.
    fun inboxTasksText(): String = text("GET", "/inbox/tasks" + q("limit" to 100))
    fun inboxApproveText(taskId: String): String = text("POST", "/inbox/tasks/${seg(taskId)}/approve", CloudApi.RelayBody.Text("{}"))
    fun inboxRejectText(taskId: String): String = text("POST", "/inbox/tasks/${seg(taskId)}/reject")

    fun mkdir(req: MkdirRequest): MutationResponse = text("POST", "/mkdir", json(req)).wire()
    fun move(req: MoveRequest): MutationResponse = text("POST", "/move", json(req)).wire()
    fun delete(req: DeleteRequest): DeleteResponse = text("POST", "/delete", json(req)).wire()

    companion object {
        /** Copies a response body to a file, streaming. */
        fun copy(input: InputStream, out: OutputStream, onBytes: (Int) -> Unit = {}): Long {
            val buf = ByteArray(64 * 1024)
            var total = 0L
            while (true) {
                val r = input.read(buf)
                if (r < 0) break
                out.write(buf, 0, r)
                total += r
                onBytes(r)
            }
            return total
        }
    }
}
