package com.allternit.core.files

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.net.URLEncoder
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import java.util.UUID

/**
 * This phone's Files key ring (contracts/files-drive/SPEC.md section 13), the port of iOS AllternitFilesKit
 * `Keys/FilesKeyService.swift`. One ring per phone: :app's FilesKeys (the DocumentsProvider's replica reads) and
 * Settings > Files > Files key (:feature:settings) both read and write it.
 *
 * - The phone's long-lived X25519 secret exists only as ciphertext: AES-256-GCM under the non-exportable Keystore
 *   key `files_key_device_v1` ([KeystoreBox], StrongBox when present), in the app-private prefs `files_keys`
 *   (`device_secret` = `<device id>|<base64 blob>`). The envelopes next to it are sealed to that secret. Backup is
 *   off. A blob that no longer opens (Keystore key gone) is replaced by a new key pair, and the phone asks again.
 * - The FRK (Files root key) is opened in memory only when needed and never stored in clear.
 * - The 6-digit confirm code is recomputed here from the request's public key; a request whose code does not
 *   match its key can only be declined (iOS `codeMatches`).
 *
 * [call] runs one cloud-api request (`method`, `/api/v1/…` path, JSON body) and returns the body, throwing on any
 * non-2xx ([HttpException] with the status, so a 404 envelope reads as "none"). Every call does network I/O: call off
 * the main thread.
 */
class FilesKeyRing(
    context: Context,
    private val deviceName: String,
    private val call: (method: String, path: String, body: String?) -> String,
) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("files_keys", Context.MODE_PRIVATE)
    private val box = KeystoreBox(DEVICE_ALIAS, DEVICE_AAD)

    val deviceId: String get() = deviceId(app)

    class HttpException(val status: Int, message: String) : IOException(message)
    class KeyException(message: String) : IOException(message)

    data class Device(val deviceId: String, val deviceName: String, val devicePub: String, val hasCurrentKey: Boolean, val addedAt: String?)
    data class Request(val requestId: String, val deviceId: String, val devicePub: String, val deviceName: String, val confirmCode: String)
    data class Info(val keyId: String?, val deviceIds: List<String>, val devices: List<Device>, val pendingRequests: List<Request>, val rotationNeeded: Boolean) {
        companion object {
            fun parse(text: String): Info {
                val o = JSONObject(text)
                val devices = o.optJSONArray("devices") ?: JSONArray()
                val pending = o.optJSONArray("pending_requests") ?: JSONArray()
                val ids = o.optJSONArray("device_ids") ?: JSONArray()
                return Info(
                    keyId = o.optString("key_id").takeIf { it.isNotEmpty() && !o.isNull("key_id") },
                    deviceIds = List(ids.length()) { ids.optString(it) },
                    devices = List(devices.length()) { devices.getJSONObject(it) }.map {
                        Device(it.optString("device_id"), it.optString("device_name"), it.optString("device_pub"), it.optBoolean("has_current_key"), it.optString("added_at").ifEmpty { null })
                    },
                    pendingRequests = List(pending.length()) { pending.getJSONObject(it) }.map(::request),
                    rotationNeeded = o.optBoolean("rotation_needed"),
                )
            }

            fun request(it: JSONObject) = Request(it.optString("request_id"), it.optString("device_id"), it.optString("device_pub"), it.optString("device_name"), it.optString("confirm_code"))
        }
    }

    sealed class State {
        data class Ready(val keyId: String) : State()
        data class Waiting(val code: String) : State()
        data object NeedsRequest : State()
        data object NoKey : State()
    }

    /** A waiting request from another device, with the code recomputed from its key. */
    class Approval(val request: Request) {
        val code: String
        val codeMatches: Boolean

        init {
            val pub = FilesKeyCrypto.unb64url(request.devicePub)?.takeIf { it.size == 32 }
            code = pub?.let { FilesKeyCrypto.confirmCode(it) } ?: request.confirmCode
            codeMatches = pub != null && code == request.confirmCode
        }
    }

    // ---- this phone's key ring

    @Synchronized
    fun deviceSecret(): ByteArray {
        val id = deviceId
        prefs.getString("device_secret", null)?.let { stored ->
            try {
                val (owner, blob) = stored.split('|', limit = 2)
                if (owner == id) return box.open(java.util.Base64.getDecoder().decode(blob))
            } catch (e: Exception) {
                // Keystore key gone (restore to a new phone, or the user cleared it): make a new one below.
            }
        }
        val secret = FilesKeyCrypto.x25519Secret()
        prefs.edit()
            .putString("device_secret", id + "|" + java.util.Base64.getEncoder().encodeToString(box.seal(secret)))
            .remove("envelopes")
            .apply()
        return secret
    }

    fun devicePub(): ByteArray = FilesKeyCrypto.x25519Public(deviceSecret())

    fun envelopes(): List<FilesKeyCrypto.Envelope> {
        val arr = try { JSONArray(prefs.getString("envelopes", "[]")) } catch (e: Exception) { JSONArray() }
        val id = deviceId
        return List(arr.length()) { FilesKeyCrypto.Envelope.fromJson(arr.getJSONObject(it)) }.filter { it.deviceId == id }
    }

    @Synchronized
    fun keep(env: FilesKeyCrypto.Envelope) {
        val all = listOf(env) + envelopes().filter { it.keyId != env.keyId }
        prefs.edit().putString("envelopes", JSONArray(all.take(8).map { it.toJson() }).toString()).apply()
    }

    fun forgetEnvelopes() {
        prefs.edit().remove("envelopes").apply()
    }

    /** The FRK for [keyId] (null: the newest held), opened in memory. */
    fun rootKey(keyId: String? = null): FilesKeyCrypto.RootKey {
        val env = envelopes().firstOrNull { keyId == null || it.keyId == keyId }
            ?: throw KeyException("This phone doesn't have your Files key yet. Approve it in Settings > Files > Files key.")
        return FilesKeyCrypto.unwrap(env, deviceSecret())
    }

    // ---- cloud-api

    fun info(): Info = Info.parse(call("GET", "/api/v1/files-keys", null))

    fun envelopeFor(device: String, keyId: String? = null): FilesKeyCrypto.Envelope? = try {
        val q = keyId?.let { "?key_id=" + seg(it) } ?: ""
        FilesKeyCrypto.Envelope.fromJson(JSONObject(call("GET", "/api/v1/files-keys/envelopes/${seg(device)}$q", null)))
    } catch (e: HttpException) {
        if (e.status == 404) null else throw e
    }

    private fun ownRequest(): JSONObject {
        val pub = devicePub()
        return JSONObject().put("device_id", deviceId).put("device_pub", FilesKeyCrypto.b64url(pub))
            .put("device_name", deviceName).put("confirm_code", FilesKeyCrypto.confirmCode(pub))
    }

    fun post(request: JSONObject): Request = Info.request(JSONObject(call("POST", "/api/v1/files-keys/requests", request.toString())))

    private fun grant(env: FilesKeyCrypto.Envelope, code: String) {
        call("POST", "/api/v1/files-keys/grants", JSONObject().put("envelope", env.toJson()).put("approved_by_device_id", deviceId).put("confirm_code", code).toString())
    }

    /** Where this phone stands; picks up an envelope another device granted. */
    fun state(info: Info = info()): State {
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
        if (info().keyId != null) throw KeyException("Your account already has a Files key. Approve this phone from another device.")
        val pending = post(ownRequest())
        val frk = FilesKeyCrypto.RootKey.generate()
        val env = FilesKeyCrypto.wrap(frk, devicePub(), deviceId, now())
        grant(env, pending.confirmCode)
        keep(env)
        return frk.keyId
    }

    /** Asks for the key; returns the 6-digit code to show. */
    fun requestAccess(): String = post(ownRequest()).confirmCode

    fun approvals(info: Info): List<Approval> = info.pendingRequests.filter { it.deviceId != deviceId }.map { Approval(it) }

    /** Call only after the person saw the same code on both devices and confirmed. */
    fun approve(a: Approval) {
        val pub = FilesKeyCrypto.unb64url(a.request.devicePub)?.takeIf { it.size == 32 }
        if (!a.codeMatches || pub == null) throw KeyException("That request's code doesn't match its key. Don't approve it.")
        val keyId = info().keyId ?: throw KeyException("There is no Files key yet.")
        grant(FilesKeyCrypto.wrap(rootKey(keyId), pub, a.request.deviceId, now()), a.request.confirmCode)
    }

    fun decline(a: Approval) {
        call("DELETE", "/api/v1/files-keys/requests/${seg(a.request.deviceId)}?declined_by_device_id=${seg(deviceId)}", null)
    }

    fun removeDevice(device: String) {
        call("DELETE", "/api/v1/files-keys/devices/${seg(device)}", null)
        if (device == deviceId) forgetEnvelopes()
    }

    /** A new key sealed to every listed device (this one included), after a removal. */
    fun rotate(info: Info): String {
        val current = info.keyId ?: throw KeyException("There is no Files key yet.")
        rootKey(current) // this phone must hold it
        val frk = FilesKeyCrypto.RootKey.generate()
        val envs = info.devices.map { d ->
            val pub = FilesKeyCrypto.unb64url(d.devicePub)?.takeIf { it.size == 32 } ?: throw KeyException("${d.deviceName} has a bad public key")
            FilesKeyCrypto.wrap(frk, pub, d.deviceId, now())
        }
        call("POST", "/api/v1/files-keys/rotations", JSONObject().put("key_id", frk.keyId).put("rotated_from", current)
            .put("rotated_by_device_id", deviceId).put("envelopes", JSONArray(envs.map { it.toJson() })).toString())
        envs.firstOrNull { it.deviceId == deviceId }?.let { keep(it) }
        return frk.keyId
    }

    /**
     * Gives a computer the key: its request from `GET /replicas/key` over the owner's relay ([relay], Files v2
     * paths), approved here (it came over the owner's own relay), then the envelope delivered with `PUT /replicas/key`.
     */
    fun giveComputer(relay: (method: String, path: String, body: String?) -> String) {
        val pending = post(JSONObject(relay("GET", "/replicas/key", null)))
        approve(Approval(pending))
        deliver(relay, pending.deviceId)
    }

    /** `PUT /replicas/key` with the computer's current envelope. */
    fun deliver(relay: (method: String, path: String, body: String?) -> String, device: String) {
        val env = envelopeFor(device) ?: throw KeyException("That computer doesn't have your Files key yet.")
        relay("PUT", "/replicas/key", env.toJson().toString())
    }

    private fun now(): String = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }.format(Date())

    companion object {
        const val DEVICE_ALIAS = "files_key_device_v1"
        /** The AAD Phone Link's KeystoreSealer used for every alias; kept so existing device secrets still open. */
        const val DEVICE_AAD = "allternit-phonelink-sessions-v1"

        /** Stable id for this phone as a Files device (`X-Allternit-Device-Id`, the key ring owner). Not a secret. */
        fun deviceId(context: Context): String {
            val prefs = context.applicationContext.getSharedPreferences("files_drive", Context.MODE_PRIVATE)
            return prefs.getString("device_id", null) ?: synchronized(this) {
                prefs.getString("device_id", null) ?: ("and_" + UUID.randomUUID().toString().replace("-", "")).also {
                    prefs.edit().putString("device_id", it).commit()
                }
            }
        }

        fun seg(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

        /** "123 456" */
        fun spaced(code: String): String = if (code.length == 6) code.take(3) + " " + code.takeLast(3) else code
    }
}
