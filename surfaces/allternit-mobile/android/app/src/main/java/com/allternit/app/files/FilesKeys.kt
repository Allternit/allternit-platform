package com.allternit.app.files

import android.content.Context
import android.os.Build
import android.util.Base64
import com.allternit.files.contract.ErrorCode
import com.allternit.files.contract.FilesKeyInfo
import com.allternit.files.contract.Item
import com.allternit.files.contract.PendingKeyRequest
import com.allternit.phonelink.KeystoreSealer
import com.allternit.phonelink.core.X25519
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/**
 * The Files key on this phone (contracts/files-drive/SPEC.md section 13), for the
 * DocumentsProvider: with it, a file on a sleeping computer opens from a sealed replica
 * on another of the user's computers instead of waking it.
 *
 * - This phone's X25519 secret is sealed with an Android Keystore AES key (never leaves
 *   the phone, no backup); envelopes are stored next to it.
 * - cloud-api routes: `GET /files-keys`, `POST /files-keys/requests`, `DELETE
 *   /files-keys/requests/:device_id` (decline), `POST /files-keys/grants`, `POST
 *   /files-keys/rotations`, `GET /files-keys/envelopes/:device_id`, `DELETE
 *   /files-keys/devices/:device_id`. The screen is [FilesKeyActivity].
 *
 * Every call does network I/O: call off the main thread.
 */
internal class FilesKeys(private val drive: FilesDrive) {
    private val prefs = drive.context.getSharedPreferences("files_keys", Context.MODE_PRIVATE)
    private val sealer by lazy { KeystoreSealer("files_key_device_v1") }

    sealed class State {
        data class Ready(val keyId: String) : State()
        data class Waiting(val code: String) : State()
        object NeedsRequest : State()
        object NoKey : State()
    }

    /** A waiting request from another device, with the code recomputed here. */
    class Approval(val request: PendingKeyRequest) {
        val code: String
        val valid: Boolean

        init {
            val pub = FilesKeyCrypto.unb64url(request.devicePub)?.takeIf { it.size == 32 }
            code = pub?.let { FilesKeyCrypto.confirmCode(it) } ?: request.confirmCode
            valid = pub != null && code == request.confirmCode
        }
    }

    private val deviceId get() = drive.auth.deviceId
    private val deviceName get() = (Build.MODEL ?: "Android phone").ifBlank { "Android phone" }

    // ---- this phone's key ring

    @Synchronized
    private fun deviceSecret(): ByteArray {
        prefs.getString("device_secret", null)?.let { stored ->
            try {
                val (owner, blob) = stored.split('|', limit = 2)
                if (owner == deviceId) return sealer.open(Base64.decode(blob, Base64.NO_WRAP))
            } catch (e: Exception) {
                // Keystore key gone (restore to a new phone, or the user cleared it): make a new one.
            }
        }
        val secret = X25519.generatePrivateKey()
        prefs.edit()
            .putString("device_secret", deviceId + "|" + Base64.encodeToString(sealer.seal(secret), Base64.NO_WRAP))
            .remove("envelopes")
            .apply()
        return secret
    }

    private fun devicePub(): ByteArray = X25519.publicKey(deviceSecret())

    private fun envelopes(): List<FilesKeyCrypto.Envelope> {
        val arr = try { JSONArray(prefs.getString("envelopes", "[]")) } catch (e: Exception) { JSONArray() }
        return List(arr.length()) { FilesKeyCrypto.Envelope.fromJson(arr.getJSONObject(it)) }.filter { it.deviceId == deviceId }
    }

    @Synchronized
    private fun keep(env: FilesKeyCrypto.Envelope) {
        val all = listOf(env) + envelopes().filter { it.keyId != env.keyId }
        prefs.edit().putString("envelopes", JSONArray(all.take(8).map { it.toJson() }).toString()).apply()
    }

    /** This phone holds some Files key (the replica fallback can try). */
    fun hasKey(): Boolean = drive.auth.signedIn() && envelopes().isNotEmpty()

    /** The FRK for `keyId` (null: the newest held), opened in memory. */
    fun rootKey(keyId: String? = null): FilesKeyCrypto.RootKey {
        val env = envelopes().firstOrNull { keyId == null || it.keyId == keyId }
            ?: throw FilesException(503, ErrorCode.KEY_UNAVAILABLE, null, "This phone doesn't have your Files key yet. Approve it in Files settings > Files key.")
        return FilesKeyCrypto.unwrap(env, deviceSecret())
    }

    // ---- cloud-api

    private fun call(method: String, path: String, body: JSONObject? = null): String =
        drive.api().filesKeys(method, path, body?.toString())

    fun info(): FilesKeyInfo = WireJson.decode(call("GET", "/api/v1/files-keys"), FilesKeyInfo::class.java)

    private fun envelopeFor(device: String, keyId: String? = null): FilesKeyCrypto.Envelope? = try {
        val q = keyId?.let { "?key_id=" + CloudApi.seg(it) } ?: ""
        FilesKeyCrypto.Envelope.fromJson(JSONObject(call("GET", "/api/v1/files-keys/envelopes/${CloudApi.seg(device)}$q")))
    } catch (e: FilesException) {
        if (e.status == 404) null else throw e
    }

    private fun ownRequest(): JSONObject {
        val pub = devicePub()
        return JSONObject().put("device_id", deviceId).put("device_pub", FilesKeyCrypto.b64url(pub))
            .put("device_name", deviceName).put("confirm_code", FilesKeyCrypto.confirmCode(pub))
    }

    private fun post(request: JSONObject): PendingKeyRequest =
        WireJson.decode(call("POST", "/api/v1/files-keys/requests", request), PendingKeyRequest::class.java)

    private fun grant(env: FilesKeyCrypto.Envelope, code: String) {
        call("POST", "/api/v1/files-keys/grants", JSONObject().put("envelope", env.toJson()).put("approved_by_device_id", deviceId).put("confirm_code", code))
    }

    /** Where this phone stands; picks up an envelope another device granted. */
    fun state(info: FilesKeyInfo = info()): State {
        val keyId = info.keyId ?: return State.NoKey
        if (envelopes().any { it.keyId == keyId } && runCatching { rootKey(keyId) }.isSuccess) return State.Ready(keyId)
        envelopeFor(deviceId)?.takeIf { it.keyId == keyId }?.let { env ->
            if (runCatching { FilesKeyCrypto.unwrap(env, deviceSecret()) }.isSuccess) {
                keep(env)
                return State.Ready(keyId)
            }
        }
        val pub = FilesKeyCrypto.b64url(devicePub())
        info.pendingRequests.firstOrNull { it.deviceId == deviceId && it.devicePub == pub }?.let { return State.Waiting(it.confirmCode) }
        return State.NeedsRequest
    }

    /** The account's first device makes the Files key and grants it to itself. */
    fun createFirstKey(): String {
        info().keyId?.let { throw FilesException(409, ErrorCode.ALREADY_EXISTS, null, "Your account already has a Files key. Approve this phone from another device.") }
        val pending = post(ownRequest())
        val frk = FilesKeyCrypto.RootKey.generate()
        val env = FilesKeyCrypto.wrap(frk, devicePub(), deviceId, now())
        grant(env, pending.confirmCode)
        keep(env)
        return frk.keyId
    }

    /** Asks for the key; returns the 6-digit code to show. */
    fun requestAccess(): String = post(ownRequest()).confirmCode

    fun approvals(info: FilesKeyInfo): List<Approval> = info.pendingRequests.filter { it.deviceId != deviceId }.map { Approval(it) }

    /** Call only after the user saw the same code on both devices and tapped Approve. */
    fun approve(a: Approval) {
        val pub = FilesKeyCrypto.unb64url(a.request.devicePub)
        if (!a.valid || pub == null) throw FilesException(403, ErrorCode.FORBIDDEN, null, "That request's code doesn't match its key. Don't approve it.")
        val keyId = info().keyId ?: throw FilesException(503, ErrorCode.KEY_UNAVAILABLE, null, "There is no Files key yet.")
        grant(FilesKeyCrypto.wrap(rootKey(keyId), pub, a.request.deviceId, now()), a.request.confirmCode)
    }

    fun decline(a: Approval) {
        call("DELETE", "/api/v1/files-keys/requests/${CloudApi.seg(a.request.deviceId)}?declined_by_device_id=${CloudApi.seg(deviceId)}")
    }

    fun removeDevice(device: String) {
        call("DELETE", "/api/v1/files-keys/devices/${CloudApi.seg(device)}")
        if (device == deviceId) prefs.edit().remove("envelopes").apply()
    }

    /** A new key sealed to every listed device (this one included), after a removal. */
    fun rotate(info: FilesKeyInfo): String {
        val current = info.keyId ?: throw FilesException(503, ErrorCode.KEY_UNAVAILABLE, null, "There is no Files key yet.")
        rootKey(current) // this phone must hold it
        val frk = FilesKeyCrypto.RootKey.generate()
        val envs = info.devices.map { d ->
            val pub = FilesKeyCrypto.unb64url(d.devicePub)?.takeIf { it.size == 32 } ?: throw IOException("${d.deviceName} has a bad public key")
            FilesKeyCrypto.wrap(frk, pub, d.deviceId, now())
        }
        call("POST", "/api/v1/files-keys/rotations", JSONObject().put("key_id", frk.keyId).put("rotated_from", current)
            .put("rotated_by_device_id", deviceId).put("envelopes", JSONArray(envs.map { it.toJson() })))
        envs.firstOrNull { it.deviceId == deviceId }?.let { keep(it) }
        return frk.keyId
    }

    /**
     * Gives a computer the key: its request from `GET /replicas/key` over the owner's
     * relay, approved here (no second screen: it came over the owner's own relay), then
     * the envelope delivered with `PUT /replicas/key`.
     */
    fun giveComputer(cid: String) {
        val client = drive.client(cid)
        val c = client.raw("GET", "/replicas/key")
        val req = try { JSONObject(c.inputStream.use { it.readBytes().toString(Charsets.UTF_8) }) } finally { c.disconnect() }
        val pending = post(req)
        approve(Approval(pending))
        deliver(cid, pending.deviceId)
    }

    /** `PUT /replicas/key`; the runtime wants `x-allternit-unlock` to replace a different key. */
    fun deliver(cid: String, device: String) {
        val env = envelopeFor(device) ?: throw FilesException(404, ErrorCode.NOT_FOUND, null, "That computer doesn't have your Files key yet.")
        drive.client(cid).raw("PUT", "/replicas/key", CloudApi.RelayBody.Text(env.toJson().toString())).disconnect()
    }

    // ---- replica reads (SPEC section 10 step 4)

    /** Other computers that may hold a replica of `rootId`, awake first. */
    private fun replicaHosts(cid: String, rootId: String): List<String> {
        val hosts = ArrayList<String>()
        try {
            val o = JSONObject(call("GET", "/api/v1/computer-files/${CloudApi.seg(cid)}/placement?root_id=${CloudApi.seg(rootId)}"))
            val arr = o.optJSONArray("options") ?: JSONArray()
            val opts = List(arr.length()) { arr.getJSONObject(it) }
            val rank = { s: String -> when (s) { "awake" -> 0; "offline" -> 2; else -> 1 } }
            opts.sortedBy { rank(it.optString("computer_state")) }.mapTo(hosts) { it.getString("device_id") }
        } catch (e: Exception) {
            // Placement unavailable: try every computer this phone knows.
        }
        if (hosts.isEmpty()) drive.computers.mapTo(hosts) { it.id }
        return hosts.filter { it != cid }.distinct()
    }

    private fun hostGet(client: FilesClient, path: String): ByteArray? = try {
        val c = client.raw("GET", path, extra = mapOf("accept" to "application/octet-stream"), readTimeoutMs = 60_000)
        try { c.inputStream.use { it.readBytes() } } finally { c.disconnect() }
    } catch (e: FilesException) {
        if (e.status == 404) null else throw e
    }

    /**
     * Reads `item` from a replica into `dest`, checking every chunk and the whole file.
     * Never wakes anything (the relay does not wake for Files v2). Throws when no host has
     * a current copy; the caller keeps its original error.
     */
    fun readFromReplica(cid: String, item: Item, dest: File) {
        var last: Exception = FilesException(503, ErrorCode.REPLICA_UNAVAILABLE, null, "No replica of this folder is reachable right now.")
        for (host in replicaHosts(cid, item.rootId)) {
            val rid = FilesKeyCrypto.replicaId(cid, item.rootId, host)
            try {
                val client = drive.client(host)
                val headBytes = hostGet(client, "/replicas/${CloudApi.seg(rid)}/head") ?: continue
                val frk = rootKey(FilesKeyCrypto.headKeyId(headBytes))
                val head = FilesKeyCrypto.openHead(headBytes, frk, rid)
                val sealer = FilesKeyCrypto.ChunkSealer(frk, rid)
                if (head.optLong("catalog_size") > 8L * 1024 * 1024) throw IOException("This replica's file list is too big to open on a phone.")
                val catalog = java.io.ByteArrayOutputStream()
                val parts = head.optJSONArray("parts") ?: JSONArray()
                for (i in 0 until parts.length()) {
                    val h = parts.getJSONObject(i).getString("hash")
                    val blob = hostGet(client, "/replicas/${CloudApi.seg(rid)}/blobs/${sealer.locator(h)}")
                        ?: throw IOException("A part of this replica's file list is missing.")
                    catalog.write(sealer.open(blob, h))
                }
                val json = catalog.toByteArray()
                if (Blake3.hashHex(json) != head.optString("catalog_hash")) throw FilesKeyCrypto.IntegrityException("replica catalog hash")
                val items = JSONObject(String(json, Charsets.UTF_8)).optJSONArray("items") ?: JSONArray()
                val entry = (0 until items.length()).map { items.getJSONObject(it) }.firstOrNull { it.optString("item_id") == item.id } ?: continue
                val same = entry.optString("rev") == item.rev || (item.contentHash != null && entry.optString("content_hash") == item.contentHash)
                if (entry.optString("kind") != "file" || !same || entry.optLong("size") != item.size) {
                    last = FilesException(503, ErrorCode.REPLICA_UNAVAILABLE, null, "The replica is behind. It catches up the next time both computers are on.")
                    continue
                }
                writeChunks(client, rid, sealer, entry, dest)
                return
            } catch (e: FilesException) {
                last = e
            } catch (e: IOException) {
                last = e
            }
        }
        throw last
    }

    private fun writeChunks(client: FilesClient, rid: String, sealer: FilesKeyCrypto.ChunkSealer, entry: JSONObject, dest: File) {
        val part = File(dest.parentFile, dest.name + ".replica")
        val whole = Blake3()
        var done = 0L
        try {
            FileOutputStream(part).use { out ->
                val chunks = entry.optJSONArray("chunks") ?: JSONArray()
                for (i in 0 until chunks.length()) {
                    val h = chunks.getJSONObject(i).getString("hash")
                    val blob = hostGet(client, "/replicas/${CloudApi.seg(rid)}/blobs/${sealer.locator(h)}")
                        ?: throw FilesException(503, ErrorCode.REPLICA_UNAVAILABLE, null, "A part of this file is missing on the replica.")
                    val plain = sealer.open(blob, h)
                    whole.update(plain)
                    out.write(plain)
                    done += plain.size
                }
            }
            val want = entry.optString("content_hash").ifEmpty { Blake3.hashHex(ByteArray(0)) }
            if (done != entry.optLong("size") || whole.finalizeHex() != want) throw FilesKeyCrypto.IntegrityException("replica file hash")
            if (!part.renameTo(dest)) {
                part.copyTo(dest, overwrite = true)
            }
        } finally {
            part.delete()
        }
    }

    private fun now(): String = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }.format(Date())

    companion object {
        @Volatile private var instance: FilesKeys? = null
        fun get(drive: FilesDrive): FilesKeys = instance ?: synchronized(this) { instance ?: FilesKeys(drive).also { instance = it } }
    }
}
