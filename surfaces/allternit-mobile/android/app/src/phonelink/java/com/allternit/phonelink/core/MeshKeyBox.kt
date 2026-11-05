package com.allternit.phonelink.core

import java.util.Base64

/**
 * Sealed-at-rest storage for the mesh pre-auth key. The key is sealed with the
 * app's [BlobSealer] (Android Keystore AES-256-GCM in production) and base64'd
 * into the settings store, so it is never held in the clear and never logged.
 *
 * Lifetime: written at PROVISION time, read once when the mesh node first
 * joins, and cleared by [MeshController] only after that join succeeds — tsnet
 * keeps its own node key in the mesh state dir afterwards, so the pre-auth key
 * is single-use by construction.
 */
class MeshKeyBox(private val kv: KeyValueStore, private val sealer: BlobSealer) {
    companion object {
        private const val K_SEALED_KEY = "mesh_auth_key_sealed"
    }

    @Synchronized
    fun store(authKey: String) {
        val sealed = sealer.seal(authKey.toByteArray(Charsets.UTF_8))
        kv.edit { putString(K_SEALED_KEY, Base64.getEncoder().encodeToString(sealed)) }
    }

    /** The sealed key, opened, or null if absent or unopenable. Does NOT delete. */
    @Synchronized
    fun read(): String? {
        val b64 = kv.getString(K_SEALED_KEY) ?: return null
        return try {
            String(sealer.open(Base64.getDecoder().decode(b64)), Charsets.UTF_8)
        } catch (e: Exception) {
            null
        }
    }

    @Synchronized
    fun hasKey(): Boolean = kv.getString(K_SEALED_KEY) != null

    @Synchronized
    fun clear() {
        kv.edit { putString(K_SEALED_KEY, null) }
    }
}
