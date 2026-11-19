package com.allternit.core.rfb

import java.math.BigInteger
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.spec.SecretKeySpec

/*
 * RFB sign-in for a paired computer's own screen sharing (iOS RFBAuth.swift). The bot sandbox runs
 * `x11vnc -nopw` and never gets here. Same two types noVNC answers on the web:
 * - VNC Authentication (2): DES-encrypt the 16-byte challenge with the password (RFC 6143 §7.2.2).
 * - Apple Remote Desktop (30): Diffie-Hellman over the server's prime, then the user name and
 *   password (64 bytes each, NUL-terminated, random-filled) AES-128-ECB encrypted with MD5(shared key).
 */

/** What the person typed for the computer's screen sharing. Held in memory for the viewer's life only. */
data class RfbCredentials(val username: String, val password: String)

object RfbAuth {
    /** Security types the client can answer, best first: none, Apple's, plain VNC password. */
    val supportedTypes = listOf(1, 30, 2)

    private val random = SecureRandom()

    /** The 16-byte answer to a VNC challenge: key = the password's first 8 bytes, each bit-reversed. */
    fun vncResponse(challenge: ByteArray, password: String): ByteArray? {
        if (challenge.size != 16) return null
        val key = ByteArray(8)
        password.toByteArray(Charsets.UTF_8).take(8).forEachIndexed { i, b -> key[i] = reverseBits(b) }
        return ecb("DES", key, challenge)
    }

    private fun reverseBits(byte: Byte): Byte {
        var b = byte.toInt() and 0xFF
        var r = 0
        repeat(8) {
            r = (r shl 1) or (b and 1)
            b = b shr 1
        }
        return r.toByte()
    }

    /** The ARD answer: 128 bytes of encrypted credentials, then the client's public key (prime.size bytes). */
    fun ardResponse(generator: ByteArray, prime: ByteArray, serverPublicKey: ByteArray, credentials: RfbCredentials): ByteArray? {
        val keyLength = prime.size
        if (keyLength == 0 || serverPublicKey.size != keyLength) return null
        val p = BigInteger(1, prime)
        if (!p.testBit(0)) return null
        val privateBytes = ByteArray(keyLength).also { random.nextBytes(it) }
        val x = BigInteger(1, privateBytes)
        val clientPublic = BigInteger(1, generator).modPow(x, p)
        val shared = BigInteger(1, serverPublicKey).modPow(x, p)

        val block = ByteArray(128).also { random.nextBytes(it) }
        val user = credentials.username.toByteArray(Charsets.UTF_8).take(63).toByteArray()
        val pass = credentials.password.toByteArray(Charsets.UTF_8).take(63).toByteArray()
        System.arraycopy(user, 0, block, 0, user.size)
        block[user.size] = 0
        System.arraycopy(pass, 0, block, 64, pass.size)
        block[64 + pass.size] = 0

        val aesKey = MessageDigest.getInstance("MD5").digest(fixed(shared, keyLength))
        val encrypted = ecb("AES", aesKey, block) ?: return null
        return encrypted + fixed(clientPublic, keyLength)
    }

    /** A big integer as exactly [length] big-endian bytes. */
    private fun fixed(value: BigInteger, length: Int): ByteArray {
        val raw = value.toByteArray()
        val out = ByteArray(length)
        val copy = minOf(raw.size, length)
        System.arraycopy(raw, raw.size - copy, out, length - copy, copy)
        return out
    }

    private fun ecb(algorithm: String, key: ByteArray, input: ByteArray): ByteArray? = runCatching {
        val cipher = Cipher.getInstance("$algorithm/ECB/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, SecretKeySpec(key, algorithm))
        cipher.doFinal(input)
    }.getOrNull()?.takeIf { it.size == input.size }
}
