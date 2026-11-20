package com.allternit.core.files

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import org.json.JSONObject
import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.MessageDigest
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/**
 * The phone side of the voice key hand-off, the exact port of iOS `Core/Voice/VoiceKeyHandoff.swift`
 * (`VoiceHandoffCrypto`), wire format `docs/voice-handoff-vectors.json`:
 *
 *   QR1  = allternit-voice:1:<sid>:<pubA>
 *   QR2  = allternit-voice:2:<sid>:<nonce>:<ciphertext||tag>
 *   code = SHA-256(utf8(sid) || pubA_raw || pubB_raw) → first 4 bytes, big-endian uint32, mod 1_000_000, 6 digits
 *   key  = HKDF-SHA256(ikm = P-256 ECDH x-coordinate, salt = utf8(sid), info = "allternit-voice-key-handoff-v1", 32)
 *   box  = AES-256-GCM, 12-byte nonce, no AAD, 16-byte tag appended
 *
 * Public keys are uncompressed SEC1 points (65 bytes); base64url is unpadded. The phone's one-time P-256 key is made
 * in the Android Keystore on API 31+ (ECDH `PURPOSE_AGREE_KEY`, StrongBox when present, never exportable) and
 * deleted when the hand-off ends; before API 31 the Keystore can't do ECDH, so it is a software key held only in
 * memory for the hand-off (as iOS holds its CryptoKit key).
 */
object VoiceHandoffCrypto {
    const val INFO = "allternit-voice-key-handoff-v1"
    const val PREFIX = "allternit-voice"
    val VENDORS = setOf("openai", "elevenlabs")

    sealed class QR {
        abstract val sid: String
        /** QR 1: the session id and the computer's one-time public key. */
        class Offer(override val sid: String, val senderKey: ByteArray) : QR()
        /** QR 2: the sealed key for that session. */
        class Sealed(override val sid: String, val nonce: ByteArray, val ciphertext: ByteArray) : QR()

        companion object {
            fun parse(text: String): QR? {
                val parts = text.trim().split(":")
                if (parts.size < 4 || parts[0] != PREFIX || parts[2].isEmpty()) return null
                return when {
                    parts[1] == "1" && parts.size == 4 -> {
                        val key = unb64url(parts[3])?.takeIf { it.size == 65 && it[0] == 0x04.toByte() } ?: return null
                        Offer(parts[2], key)
                    }
                    parts[1] == "2" && parts.size == 5 -> {
                        val nonce = unb64url(parts[3])?.takeIf { it.size == 12 } ?: return null
                        val sealed = unb64url(parts[4])?.takeIf { it.size > 16 } ?: return null
                        Sealed(parts[2], nonce, sealed)
                    }
                    else -> null
                }
            }
        }
    }

    /** What QR 2 / the relay carries. */
    class Payload(val provider: String, val apiKey: String, val voiceId: String?)

    class Failure(val kind: String) : Exception(kind)

    fun confirmCode(sid: String, senderKey: ByteArray, receiverKey: ByteArray): String {
        val md = MessageDigest.getInstance("SHA-256")
        md.update(sid.toByteArray(Charsets.UTF_8))
        md.update(senderKey)
        md.update(receiverKey)
        val d = md.digest()
        val v = ((d[0].toLong() and 0xff) shl 24) or ((d[1].toLong() and 0xff) shl 16) or ((d[2].toLong() and 0xff) shl 8) or (d[3].toLong() and 0xff)
        return (v % 1_000_000).toString().padStart(6, '0')
    }

    /** One hand-off's P-256 key pair. [close] forgets it (deletes the Keystore entry). */
    class Ephemeral private constructor(private val pair: KeyPair, private val alias: String?) : AutoCloseable {
        /** Uncompressed SEC1 (65 bytes). */
        val publicKey: ByteArray = encodePoint(pair.public as ECPublicKey)

        /** The ECDH x-coordinate with the computer's key (`badKey` for a point that isn't on P-256). */
        fun sharedSecret(peer: ByteArray): ByteArray {
            val peerKey = try { decodePoint(peer, pair.public as ECPublicKey) } catch (e: Exception) { throw Failure("badKey") }
            val ka = if (alias != null) KeyAgreement.getInstance("ECDH", "AndroidKeyStore") else KeyAgreement.getInstance("ECDH")
            return try {
                ka.init(pair.private)
                ka.doPhase(peerKey, true)
                ka.generateSecret()
            } catch (e: Exception) {
                throw Failure("badKey")
            }
        }

        override fun close() {
            alias?.let { a -> runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(a) } }
        }

        companion object {
            private const val ALIAS = "voice_handoff_ephemeral_v1"

            fun create(): Ephemeral {
                if (Build.VERSION.SDK_INT >= 31) {
                    runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(ALIAS) }
                    fun gen(strongBox: Boolean): KeyPair {
                        val g = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, "AndroidKeyStore")
                        val b = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_AGREE_KEY)
                            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
                        if (strongBox) b.setIsStrongBoxBacked(true)
                        g.initialize(b.build())
                        return g.generateKeyPair()
                    }
                    val pair = try {
                        gen(strongBox = true)
                    } catch (e: java.security.ProviderException) {
                        // No StrongBox (StrongBoxUnavailableException), or a chip that refuses AGREE_KEY: the TEE key.
                        null
                    } ?: runCatching { gen(strongBox = false) }.getOrNull()
                    if (pair != null) return Ephemeral(pair, ALIAS)
                }
                val g = KeyPairGenerator.getInstance("EC")
                g.initialize(ECGenParameterSpec("secp256r1"))
                return Ephemeral(g.generateKeyPair(), null)
            }
        }
    }

    fun sessionKey(ephemeral: Ephemeral, senderKey: ByteArray, sid: String): ByteArray {
        val shared = ephemeral.sharedSecret(senderKey)
        return try {
            FilesKeyCrypto.hkdf(shared, sid.toByteArray(Charsets.UTF_8), INFO.toByteArray(Charsets.UTF_8))
        } finally {
            shared.fill(0)
        }
    }

    /** Opens QR 2's sealed box (AES-256-GCM, no AAD, 16-byte tag appended). */
    fun open(nonce: ByteArray, ciphertext: ByteArray, key: ByteArray): Payload {
        val plain = try {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce))
            c.doFinal(ciphertext)
        } catch (e: Exception) {
            throw Failure("cannotOpen")
        }
        try {
            val o = JSONObject(String(plain, Charsets.UTF_8))
            val provider = o.optString("provider")
            val apiKey = o.optString("api_key")
            if (provider !in VENDORS || apiKey.isBlank()) throw Failure("badPayload")
            return Payload(provider, apiKey, o.optString("voice_id").takeIf { it.isNotEmpty() && !o.isNull("voice_id") })
        } catch (e: Failure) {
            throw e
        } catch (e: Exception) {
            throw Failure("badPayload")
        } finally {
            plain.fill(0)
        }
    }

    fun b64url(b: ByteArray): String = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(b)

    fun unb64url(s: String): ByteArray? = try {
        java.util.Base64.getUrlDecoder().decode(s.trimEnd('='))
    } catch (e: IllegalArgumentException) {
        null
    }

    private fun fixed(n: BigInteger): ByteArray {
        val raw = n.toByteArray()
        return when {
            raw.size == 32 -> raw
            raw.size > 32 -> raw.copyOfRange(raw.size - 32, raw.size)
            else -> ByteArray(32 - raw.size) + raw
        }
    }

    private fun encodePoint(k: ECPublicKey): ByteArray = byteArrayOf(0x04) + fixed(k.w.affineX) + fixed(k.w.affineY)

    private fun decodePoint(raw: ByteArray, like: ECPublicKey): java.security.PublicKey {
        require(raw.size == 65 && raw[0] == 0x04.toByte())
        val point = ECPoint(BigInteger(1, raw.copyOfRange(1, 33)), BigInteger(1, raw.copyOfRange(33, 65)))
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point, like.params))
    }
}
