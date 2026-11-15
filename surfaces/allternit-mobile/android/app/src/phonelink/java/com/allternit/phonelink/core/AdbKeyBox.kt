package com.allternit.phonelink.core

import java.util.Base64

/**
 * Sealed-at-rest storage for the per-phone adb key (PKCS#8 DER) that lets the
 * app start its shell server over its own wireless debugging. Same scheme as
 * [MeshKeyBox]: sealed with the app's [BlobSealer], never held in the clear
 * at rest, never logged. Unlike the mesh key it is long-lived: every boot uses it.
 */
class AdbKeyBox(private val kv: KeyValueStore, private val sealer: BlobSealer) {
    companion object {
        private const val K_SEALED_KEY = "adb_key_sealed"
        private const val K_TOKEN = "shell_server_token"
    }

    @Synchronized
    fun store(pkcs8: ByteArray) {
        kv.edit { putString(K_SEALED_KEY, Base64.getEncoder().encodeToString(sealer.seal(pkcs8))) }
    }

    @Synchronized
    fun read(): ByteArray? {
        val b64 = kv.getString(K_SEALED_KEY) ?: return null
        return try { sealer.open(Base64.getDecoder().decode(b64)) } catch (e: Exception) { null }
    }

    @Synchronized
    fun hasKey(): Boolean = kv.getString(K_SEALED_KEY) != null

    @Synchronized
    fun clear() = kv.edit { putString(K_SEALED_KEY, null); putString(K_TOKEN, null) }

    /**
     * The shell server's token: random, created once, reused across launches so
     * a server still running from before an app restart keeps accepting us.
     */
    @Synchronized
    fun shellToken(newToken: () -> String): String =
        kv.getString(K_TOKEN) ?: newToken().also { t -> kv.edit { putString(K_TOKEN, t) } }
}
