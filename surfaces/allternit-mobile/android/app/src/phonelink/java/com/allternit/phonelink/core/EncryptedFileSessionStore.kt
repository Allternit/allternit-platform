package com.allternit.phonelink.core

import java.io.File

/** Encrypts/decrypts a blob (Android Keystore AES-GCM on device). */
interface BlobSealer {
    fun seal(plain: ByteArray): ByteArray
    /** Throws if the blob was not sealed by this key or was tampered with. */
    fun open(sealed: ByteArray): ByteArray
}

/**
 * Sessions (the pairing keys) as one sealed JSON blob in app-private storage.
 * Written to a temp file and renamed, so a crash mid-write never leaves half a file.
 * An unreadable blob (key lost after a restore, corruption) loads as empty:
 * clients then get auth_failed and re-pair, which is the safe outcome.
 */
class EncryptedFileSessionStore(
    private val file: File,
    private val sealer: BlobSealer,
    private val log: (String) -> Unit = {},
) : SessionStore {
    @Synchronized
    override fun load(): List<AuthSession> {
        if (!file.exists()) return emptyList()
        return try {
            AuthSession.listFromJson(String(sealer.open(file.readBytes()), Charsets.UTF_8))
        } catch (e: Exception) {
            log("session store unreadable (${e.javaClass.simpleName}); starting empty")
            emptyList()
        }
    }

    @Synchronized
    override fun save(sessions: List<AuthSession>) {
        val blob = sealer.seal(AuthSession.listToJson(sessions).toByteArray(Charsets.UTF_8))
        val tmp = File(file.parentFile, file.name + ".tmp")
        tmp.writeBytes(blob)
        if (!tmp.renameTo(file)) {
            file.delete()
            if (!tmp.renameTo(file)) throw java.io.IOException("could not replace ${file.name}")
        }
    }
}
