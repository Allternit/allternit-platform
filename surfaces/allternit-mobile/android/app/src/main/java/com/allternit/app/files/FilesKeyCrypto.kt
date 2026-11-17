package com.allternit.app.files

import org.bouncycastle.crypto.InvalidCipherTextException
import org.bouncycastle.crypto.digests.SHA256Digest
import org.bouncycastle.crypto.engines.ChaChaEngine
import org.bouncycastle.crypto.generators.HKDFBytesGenerator
import org.bouncycastle.crypto.modes.ChaCha20Poly1305
import org.bouncycastle.crypto.params.AEADParameters
import org.bouncycastle.crypto.params.HKDFParameters
import org.bouncycastle.crypto.params.KeyParameter
import org.bouncycastle.math.ec.rfc7748.X25519
import org.bouncycastle.util.Pack
import org.json.JSONObject
import java.io.IOException
import java.security.SecureRandom

/**
 * Files key crypto (contracts/files-drive/SPEC.md section 13), the same as the iOS
 * `FilesKeyCrypto` and the web keyring, checked against `vectors/keys-v1.json`
 * (FilesKeyVectorsTest):
 *
 * - device envelopes: X25519 + HKDF-SHA256 + XChaCha20-Poly1305 (`x25519-hkdf-sha256-xchacha20poly1305`);
 * - the 6-digit confirm code: `u32_be(blake3("files-key-confirm/1|" || device_pub)[0..4]) % 1000000`;
 * - replica ids, sealed chunks (`afc1`) and sealed heads (`afh1`).
 *
 * Primitives are BouncyCastle (bcprov-jdk18on 1.78.1, same as :ondevice): X25519
 * (`math.ec.rfc7748`, constant time), HKDF-SHA256 (`HKDFBytesGenerator`) and
 * ChaCha20-Poly1305; only the HChaCha20 subkey step of XChaCha20 is composed here
 * ([XChaCha20Poly1305]). Platform crypto can't be used: minSdk is 26, `XDH` arrives in
 * API 33 and `ChaCha20-Poly1305` in API 28.
 */
internal object FilesKeyCrypto {
    const val WRAP_ALG = "x25519-hkdf-sha256-xchacha20poly1305"
    const val CIPHER = "xchacha20poly1305"
    private const val MAX_HEAD_BYTES = 1 shl 20
    private val random = SecureRandom()

    class Envelope(
        val keyId: String,
        val deviceId: String,
        val ephemeralPub: String,
        val nonce: String,
        val wrappedKey: String,
        val createdAt: String,
        val wrapAlg: String = WRAP_ALG,
    ) {
        fun toJson(): JSONObject = JSONObject()
            .put("key_id", keyId).put("device_id", deviceId).put("wrap_alg", wrapAlg)
            .put("ephemeral_pub", ephemeralPub).put("nonce", nonce).put("wrapped_key", wrappedKey)
            .put("created_at", createdAt)

        companion object {
            fun fromJson(o: JSONObject) = Envelope(
                keyId = o.getString("key_id"), deviceId = o.getString("device_id"),
                ephemeralPub = o.getString("ephemeral_pub"), nonce = o.getString("nonce"),
                wrappedKey = o.getString("wrapped_key"), createdAt = o.optString("created_at"),
                wrapAlg = o.optString("wrap_alg", WRAP_ALG),
            )
        }
    }

    /** A Files root key in memory. */
    class RootKey(val keyId: String, internal val bytes: ByteArray) {
        init { require(bytes.size == 32) { "a Files root key is 32 bytes" } }
        fun subkey(info: String): ByteArray = hkdf(bytes, ByteArray(0), info.toByteArray(Charsets.UTF_8))

        companion object {
            fun generate() = RootKey("fk_" + Blake3.hex(randomBytes(12)), randomBytes(32))
        }
    }

    fun randomBytes(n: Int) = ByteArray(n).also { random.nextBytes(it) }

    // ---- encodings

    // java.util.Base64 (API 26+) so the JVM unit tests run the same code.
    fun b64url(b: ByteArray): String = java.util.Base64.getUrlEncoder().withoutPadding().encodeToString(b)

    fun unb64url(s: String): ByteArray? = try {
        java.util.Base64.getUrlDecoder().decode(s.trimEnd('='))
    } catch (e: IllegalArgumentException) {
        null
    }

    fun unhex(s: String): ByteArray? {
        if (s.length % 2 != 0) return null
        return try {
            ByteArray(s.length / 2) { s.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
        } catch (e: NumberFormatException) {
            null
        }
    }

    // ---- derivations

    /** HKDF-SHA256, 32 bytes. An empty salt is HashLen zero bytes (RFC 5869). */
    fun hkdf(ikm: ByteArray, salt: ByteArray, info: ByteArray): ByteArray {
        val out = ByteArray(32)
        HKDFBytesGenerator(SHA256Digest()).apply { init(HKDFParameters(ikm, salt.takeIf { it.isNotEmpty() }, info)) }.generateBytes(out, 0, 32)
        return out
    }

    // ---- X25519 (BouncyCastle, constant time)

    fun x25519Secret(): ByteArray = ByteArray(X25519.SCALAR_SIZE).also { X25519.generatePrivateKey(random, it) }

    fun x25519Public(secret: ByteArray): ByteArray = ByteArray(X25519.POINT_SIZE).also { X25519.generatePublicKey(secret, 0, it, 0) }

    /** X25519(secret, peer); refuses the all-zero result (a low-order peer point). */
    fun x25519(secret: ByteArray, peerPub: ByteArray): ByteArray {
        require(secret.size == 32 && peerPub.size == 32) { "X25519 inputs are 32 bytes" }
        val out = ByteArray(32)
        if (!X25519.calculateAgreement(secret, 0, peerPub, 0, out, 0)) throw IOException("X25519 produced the all-zero secret")
        return out
    }

    fun confirmCode(devicePub: ByteArray): String {
        val h = Blake3.hash("files-key-confirm/1|".toByteArray(Charsets.UTF_8) + devicePub)
        val n = ((h[0].toLong() and 0xff) shl 24) or ((h[1].toLong() and 0xff) shl 16) or
            ((h[2].toLong() and 0xff) shl 8) or (h[3].toLong() and 0xff)
        return (n % 1_000_000).toString().padStart(6, '0')
    }

    fun replicaId(sourceComputerId: String, rootId: String, hostDeviceId: String): String =
        "rp_" + Blake3.hex(Blake3.hash("files-replica-id/1|$sourceComputerId|$rootId|$hostDeviceId".toByteArray(Charsets.UTF_8))).take(24)

    // ---- envelopes

    fun wrap(frk: RootKey, devicePub: ByteArray, deviceId: String, createdAt: String, ephemeralSecret: ByteArray? = null, nonce: ByteArray? = null): Envelope {
        val eph = ephemeralSecret ?: x25519Secret()
        val ephPub = x25519Public(eph)
        val shared = x25519(eph, devicePub)
        val kek = hkdf(shared, ephPub + devicePub, "files-envelope/1/$deviceId".toByteArray(Charsets.UTF_8))
        val n = nonce ?: randomBytes(24)
        val sealed = XChaCha20Poly1305.seal(frk.bytes, kek, n, "afe1|${frk.keyId}|$deviceId".toByteArray(Charsets.UTF_8))
        return Envelope(frk.keyId, deviceId, b64url(ephPub), b64url(n), b64url(sealed), createdAt)
    }

    fun unwrap(env: Envelope, deviceSecret: ByteArray): RootKey {
        if (env.wrapAlg != WRAP_ALG) throw IOException("Unknown Files key wrapping ${env.wrapAlg}")
        val ephPub = unb64url(env.ephemeralPub)?.takeIf { it.size == 32 } ?: throw IOException("Damaged Files key envelope")
        val nonce = unb64url(env.nonce)?.takeIf { it.size == 24 } ?: throw IOException("Damaged Files key envelope")
        val wrapped = unb64url(env.wrappedKey)?.takeIf { it.size == 48 } ?: throw IOException("Damaged Files key envelope")
        val devicePub = x25519Public(deviceSecret)
        val shared = x25519(deviceSecret, ephPub)
        val kek = hkdf(shared, ephPub + devicePub, "files-envelope/1/${env.deviceId}".toByteArray(Charsets.UTF_8))
        val frk = XChaCha20Poly1305.open(wrapped, kek, nonce, "afe1|${env.keyId}|${env.deviceId}".toByteArray(Charsets.UTF_8))
            ?: throw IOException("This Files key envelope doesn't open with this phone's key")
        return RootKey(env.keyId, frk)
    }

    // ---- replica chunks and heads

    /** `u32 big-endian header length | header JSON | ciphertext`. */
    private fun split(blob: ByteArray): Pair<JSONObject, ByteArray> {
        if (blob.size < 4) throw IntegrityException("sealed blob too short")
        val len = ((blob[0].toInt() and 0xff) shl 24) or ((blob[1].toInt() and 0xff) shl 16) or
            ((blob[2].toInt() and 0xff) shl 8) or (blob[3].toInt() and 0xff)
        if (len < 2 || len > blob.size - 4) throw IntegrityException("sealed blob header damaged")
        val json = try {
            JSONObject(String(blob, 4, len, Charsets.UTF_8))
        } catch (e: Exception) {
            throw IntegrityException("sealed blob header unreadable")
        }
        return json to blob.copyOfRange(4 + len, blob.size)
    }

    class IntegrityException(message: String) : IOException(message)

    class ChunkSealer(frk: RootKey, replicaId: String) {
        val keyId = frk.keyId
        private val key = frk.subkey("files-replica/1/$replicaId")

        fun locator(plaintextHash: String): String {
            val raw = unhex(plaintextHash)?.takeIf { it.size == 32 } ?: throw IntegrityException("$plaintextHash is not a blake3 hash")
            return Blake3.keyedHashHex(key, raw)
        }

        /** Opens one sealed chunk and checks it is the plaintext named by `plaintextHash`. */
        fun open(blob: ByteArray, plaintextHash: String): ByteArray {
            val (h, ct) = split(blob)
            val v = h.optInt("v")
            val cipher = h.optString("cipher")
            val hKey = h.optString("key_id")
            val loc = h.optString("locator")
            val size = h.optLong("plaintext_size", -1)
            if (v != 1 || cipher != CIPHER) throw IntegrityException("unknown sealed format v$v $cipher")
            if (hKey != keyId) throw IOException("This chunk was sealed with Files key $hKey.")
            val nonce = unb64url(h.optString("nonce"))?.takeIf { it.size == 24 } ?: throw IntegrityException("bad chunk nonce")
            if (loc != locator(plaintextHash)) throw IntegrityException("chunk locator does not match")
            val aad = "afc1|$v|$cipher|$hKey|$loc|$size".toByteArray(Charsets.UTF_8)
            val plain = XChaCha20Poly1305.open(ct, key, nonce, aad) ?: throw IntegrityException("sealed chunk failed authentication")
            if (plain.size.toLong() != size) throw IntegrityException("sealed chunk size does not match")
            val actual = Blake3.hashHex(plain)
            if (actual != plaintextHash) throw IntegrityException("chunk hash $actual, expected $plaintextHash")
            return plain
        }
    }

    /** The `key_id` a sealed replica head names (readable without a key). */
    fun headKeyId(bytes: ByteArray): String = split(bytes).first.optString("key_id").ifEmpty { throw IntegrityException("replica head damaged") }

    /** `replica::open_head`: subkey `files-replica/1/<id>/head`, AAD `afh1|1|<id>|<key_id>`. */
    fun openHead(bytes: ByteArray, frk: RootKey, replicaId: String): JSONObject {
        if (bytes.size > MAX_HEAD_BYTES) throw IntegrityException("replica head too big")
        val (env, ct) = split(bytes)
        if (env.optInt("v") != 1) throw IntegrityException("replica head damaged")
        val keyId = env.optString("key_id")
        if (keyId != frk.keyId) throw IOException("This replica was sealed with Files key $keyId.")
        val nonce = unb64url(env.optString("nonce"))?.takeIf { it.size == 24 } ?: throw IntegrityException("replica head damaged")
        val plain = XChaCha20Poly1305.open(ct, frk.subkey("files-replica/1/$replicaId/head"), nonce, "afh1|1|$replicaId|$keyId".toByteArray(Charsets.UTF_8))
            ?: throw IntegrityException("replica head failed authentication")
        val head = JSONObject(String(plain, Charsets.UTF_8))
        if (head.optString("replica_id") != replicaId) throw IntegrityException("replica head names another replica")
        return head
    }
}

/**
 * XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03 section 2.3): HChaCha20(key, nonce[0..16])
 * gives the subkey, then BouncyCastle's ChaCha20-Poly1305 (RFC 8439) runs with the nonce
 * `00 00 00 00 || nonce[16..24]`. BC has no XChaCha AEAD, so only the HChaCha20 step is
 * here, and it uses BC's own ChaCha core for the 20 rounds.
 */
internal object XChaCha20Poly1305 {
    fun seal(plain: ByteArray, key: ByteArray, nonce: ByteArray, aad: ByteArray): ByteArray {
        val aead = cipher(true, key, nonce, aad)
        val out = ByteArray(aead.getOutputSize(plain.size))
        val n = aead.processBytes(plain, 0, plain.size, out, 0)
        aead.doFinal(out, n)
        return out
    }

    /** The plaintext, or null when the tag does not match. */
    fun open(sealed: ByteArray, key: ByteArray, nonce: ByteArray, aad: ByteArray): ByteArray? {
        if (sealed.size < 16) return null
        val aead = cipher(false, key, nonce, aad)
        val out = ByteArray(aead.getOutputSize(sealed.size))
        return try {
            val n = aead.processBytes(sealed, 0, sealed.size, out, 0)
            val m = aead.doFinal(out, n)
            out.copyOf(n + m)
        } catch (e: InvalidCipherTextException) {
            null
        }
    }

    private fun cipher(encrypt: Boolean, key: ByteArray, nonce: ByteArray, aad: ByteArray): ChaCha20Poly1305 {
        require(key.size == 32 && nonce.size == 24) { "XChaCha20-Poly1305 needs a 32-byte key and a 24-byte nonce" }
        val n12 = ByteArray(12)
        System.arraycopy(nonce, 16, n12, 4, 8)
        return ChaCha20Poly1305().apply {
            init(encrypt, AEADParameters(KeyParameter(hchacha20(key, nonce.copyOfRange(0, 16))), 128, n12, aad))
        }
    }

    /**
     * HChaCha20 (draft section 2.2): the ChaCha20 state from the key and a 16-byte nonce,
     * 20 rounds, words 0..3 and 12..15 without the final addition. BC's `chachaCore` adds
     * the input back, so it is subtracted again.
     */
    fun hchacha20(key: ByteArray, nonce16: ByteArray): ByteArray {
        require(key.size == 32 && nonce16.size == 16)
        val input = IntArray(16)
        input[0] = 0x61707865; input[1] = 0x3320646e; input[2] = 0x79622d32; input[3] = 0x6b206574
        for (i in 0 until 8) input[4 + i] = Pack.littleEndianToInt(key, 4 * i)
        for (i in 0 until 4) input[12 + i] = Pack.littleEndianToInt(nonce16, 4 * i)
        val x = IntArray(16)
        ChaChaEngine.chachaCore(20, input, x)
        val out = ByteArray(32)
        for (i in 0 until 4) Pack.intToLittleEndian(x[i] - input[i], out, 4 * i)
        for (i in 0 until 4) Pack.intToLittleEndian(x[12 + i] - input[12 + i], out, 16 + 4 * i)
        return out
    }
}
