package com.allternit.core.files

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * AES-256-GCM under a non-exportable, app-private Android Keystore key (iOS: the Keychain item, this device only).
 * The key is made in StrongBox when the phone has one (API 28+), else in the TEE. A key made before this class
 * (Phone Link's KeystoreSealer, the Files key ring's `files_key_device_v1`) keeps working: the alias and the blob
 * format are the same.
 *
 * Blob: version(1) | ivLen(1) | iv | ciphertext+tag. The AAD binds a blob to its purpose.
 * Never log what goes in or comes out.
 */
class KeystoreBox(private val alias: String, aad: String) {
    private val aad = aad.toByteArray(Charsets.UTF_8)

    @Synchronized
    private fun key(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(alias, null) as? SecretKey)?.let { return it }
        fun spec(strongBox: Boolean): KeyGenParameterSpec {
            val b = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
            if (strongBox && Build.VERSION.SDK_INT >= 28) b.setIsStrongBoxBacked(true)
            return b.build()
        }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        if (Build.VERSION.SDK_INT >= 28) {
            try {
                gen.init(spec(strongBox = true))
                return gen.generateKey()
            } catch (e: StrongBoxUnavailableException) {
                // No StrongBox on this phone: the TEE key below.
            }
        }
        gen.init(spec(strongBox = false))
        return gen.generateKey()
    }

    fun seal(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        val iv = cipher.iv
        return byteArrayOf(1, iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    fun open(sealed: ByteArray): ByteArray {
        require(sealed.size > 2 && sealed[0].toInt() == 1) { "unknown blob version" }
        val ivLen = sealed[1].toInt() and 0xff
        require(sealed.size > 2 + ivLen) { "truncated blob" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, sealed.copyOfRange(2, 2 + ivLen)))
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed, 2 + ivLen, sealed.size - 2 - ivLen)
    }

    /** Forgets the key (sign-out): every blob it sealed stops opening. */
    fun destroy() {
        runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(alias) }
    }
}

/**
 * A small secret kept as a Keystore-sealed file in no-backup storage (iOS: one Keychain item). Used for the device
 * read token (private notification bodies) and the user's own voice keys. The plaintext is never in prefs or logs.
 */
class SealedSecret(context: android.content.Context, name: String, private val box: KeystoreBox) {
    private val file = java.io.File(java.io.File(context.noBackupFilesDir, "sealed").apply { mkdirs() }, "$name.bin")

    fun read(): String? = try {
        if (!file.exists()) null else String(box.open(file.readBytes()), Charsets.UTF_8).takeIf { it.isNotEmpty() }
    } catch (e: Exception) {
        // Keystore key gone (restore to a new phone): the secret is lost, as on iOS with a this-device-only item.
        file.delete()
        null
    }

    fun write(value: String?) {
        if (value.isNullOrEmpty()) {
            file.delete()
            return
        }
        val tmp = java.io.File(file.parentFile, file.name + ".tmp")
        tmp.writeBytes(box.seal(value.toByteArray(Charsets.UTF_8)))
        if (!tmp.renameTo(file)) {
            tmp.copyTo(file, overwrite = true)
            tmp.delete()
        }
    }
}
