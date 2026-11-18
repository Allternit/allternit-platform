package com.allternit.phonelink.core

/** Encrypts/decrypts a blob (Android Keystore AES-GCM on device). */
interface BlobSealer {
    fun seal(plain: ByteArray): ByteArray
    /** Throws if the blob was not sealed by this key or was tampered with. */
    fun open(sealed: ByteArray): ByteArray
}
