package com.allternit.phonelink

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import com.allternit.phonelink.core.BlobSealer
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * AES-256-GCM with a non-exportable key in the Android Keystore (no
 * EncryptedSharedPreferences: deprecated). No user-auth requirement on the key,
 * so the server can read its sessions after a reboot without anyone unlocking
 * an extra prompt; the file itself lives in credential-encrypted storage, so
 * nothing is readable before the first unlock after boot.
 *
 * Blob: version(1) | ivLen(1) | iv | ciphertext+tag.
 */
class KeystoreSealer(private val alias: String = "phonelink_sessions_v1") : BlobSealer {
    private val aad = "allternit-phonelink-sessions-v1".toByteArray()

    @Synchronized
    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return gen.generateKey()
    }

    override fun seal(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        val iv = cipher.iv
        val ct = cipher.doFinal(plain)
        return byteArrayOf(1, iv.size.toByte()) + iv + ct
    }

    override fun open(sealed: ByteArray): ByteArray {
        require(sealed.size > 2 && sealed[0].toInt() == 1) { "unknown blob version" }
        val ivLen = sealed[1].toInt() and 0xff
        require(sealed.size > 2 + ivLen) { "truncated blob" }
        val iv = sealed.copyOfRange(2, 2 + ivLen)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed, 2 + ivLen, sealed.size - 2 - ivLen)
    }
}
