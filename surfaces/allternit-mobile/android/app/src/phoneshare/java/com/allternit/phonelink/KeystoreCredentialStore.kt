package com.allternit.phonelink

import com.allternit.phonelink.core.AppPhoneCredentials
import com.allternit.phonelink.core.BlobSealer
import com.allternit.phonelink.core.CredentialStore
import org.json.JSONObject
import java.io.File

/**
 * The app phone key and session key, sealed with a non-exportable Android
 * Keystore key ([KeystoreSealer], its own alias) in app-private storage.
 * Unreadable (key lost after a restore, tampering) = not paired.
 */
class KeystoreCredentialStore(private val file: File, private val sealer: BlobSealer) : CredentialStore {
    @Synchronized
    override fun load(): AppPhoneCredentials? {
        if (!file.exists()) return null
        return try {
            val o = JSONObject(String(sealer.open(file.readBytes()), Charsets.UTF_8))
            AppPhoneCredentials(
                phoneId = o.getString("phoneId"),
                name = o.getString("name"),
                runtimeDeviceId = o.getString("runtimeDeviceId"),
                appPhoneKey = o.getString("appPhoneKey"),
                sessionKeyHex = o.getString("sessionKeyHex"),
                cloudBaseUrl = o.getString("cloudBaseUrl"),
            )
        } catch (e: Exception) {
            null
        }
    }

    @Synchronized
    override fun save(credentials: AppPhoneCredentials) {
        val o = JSONObject()
            .put("phoneId", credentials.phoneId).put("name", credentials.name)
            .put("runtimeDeviceId", credentials.runtimeDeviceId).put("appPhoneKey", credentials.appPhoneKey)
            .put("sessionKeyHex", credentials.sessionKeyHex).put("cloudBaseUrl", credentials.cloudBaseUrl)
        val blob = sealer.seal(o.toString().toByteArray(Charsets.UTF_8))
        val tmp = File(file.parentFile, file.name + ".tmp")
        tmp.writeBytes(blob)
        if (!tmp.renameTo(file)) {
            file.delete()
            if (!tmp.renameTo(file)) throw java.io.IOException("could not replace ${file.name}")
        }
    }

    @Synchronized
    override fun clear() { file.delete() }
}
