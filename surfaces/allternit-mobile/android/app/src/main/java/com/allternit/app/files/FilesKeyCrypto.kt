package com.allternit.app.files

import com.allternit.phonelink.core.X25519
import org.json.JSONObject
import java.io.IOException
import java.security.MessageDigest
import java.security.SecureRandom
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * Files key crypto (contracts/files-drive/SPEC.md section 13), the same as the iOS
 * `FilesKeyCrypto` and the web keyring, checked against `vectors/keys-v1.json`
 * (FilesKeyVectorsTest):
 *
 * - device envelopes: X25519 + HKDF-SHA256 + XChaCha20-Poly1305 (`x25519-hkdf-sha256-xchacha20poly1305`);
 * - the 6-digit confirm code: `u32_be(blake3("files-key-confirm/1|" || device_pub)[0..4]) % 1000000`;
 * - replica ids, sealed chunks (`afc1`) and sealed heads (`afh1`).
 *
 * Why in-tree: minSdk is 26; `XDH` arrives in API 33 and `ChaCha20-Poly1305` in API 28,
 * and XChaCha20 is in neither. X25519 reuses the Phone Link implementation (BigInteger,
 * not constant-time; see the risk note in the PR). ChaCha20, HChaCha20 and Poly1305 follow
 * RFC 8439 and draft-irtf-cfrg-xchacha-03.
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

    /** HKDF-SHA256, one 32-byte block. An empty salt is 32 zero bytes (RFC 5869). */
    fun hkdf(ikm: ByteArray, salt: ByteArray, info: ByteArray): ByteArray {
        val prk = hmac(if (salt.isEmpty()) ByteArray(32) else salt, ikm)
        return hmac(prk, info + byteArrayOf(1))
    }

    private fun hmac(key: ByteArray, data: ByteArray): ByteArray =
        Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(key, "HmacSHA256")) }.doFinal(data)

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
        val eph = ephemeralSecret ?: X25519.generatePrivateKey(random)
        val ephPub = X25519.publicKey(eph)
        val shared = X25519.sharedSecret(eph, devicePub)
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
        val devicePub = X25519.publicKey(deviceSecret)
        val shared = X25519.sharedSecret(deviceSecret, ephPub)
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

/** XChaCha20-Poly1305 (draft-irtf-cfrg-xchacha-03) on an in-tree ChaCha20-Poly1305 (RFC 8439). */
internal object XChaCha20Poly1305 {
    fun seal(plain: ByteArray, key: ByteArray, nonce: ByteArray, aad: ByteArray): ByteArray {
        val (subkey, n12) = derive(key, nonce)
        val ct = ChaCha20.xor(subkey, n12, 1, plain)
        return ct + tag(subkey, n12, aad, ct)
    }

    /** The plaintext, or null when the tag does not match. */
    fun open(sealed: ByteArray, key: ByteArray, nonce: ByteArray, aad: ByteArray): ByteArray? {
        if (sealed.size < 16) return null
        val (subkey, n12) = derive(key, nonce)
        val ct = sealed.copyOfRange(0, sealed.size - 16)
        val want = sealed.copyOfRange(sealed.size - 16, sealed.size)
        if (!MessageDigest.isEqual(want, tag(subkey, n12, aad, ct))) return null
        return ChaCha20.xor(subkey, n12, 1, ct)
    }

    private fun derive(key: ByteArray, nonce: ByteArray): Pair<ByteArray, ByteArray> {
        require(key.size == 32 && nonce.size == 24) { "XChaCha20-Poly1305 needs a 32-byte key and a 24-byte nonce" }
        val subkey = ChaCha20.hchacha20(key, nonce.copyOfRange(0, 16))
        val n12 = ByteArray(12)
        System.arraycopy(nonce, 16, n12, 4, 8)
        return subkey to n12
    }

    private fun tag(key: ByteArray, n12: ByteArray, aad: ByteArray, ct: ByteArray): ByteArray {
        val polyKey = ChaCha20.block(key, n12, 0).copyOf(32)
        val mac = Poly1305(polyKey)
        mac.update(aad); mac.update(ByteArray((16 - aad.size % 16) % 16))
        mac.update(ct); mac.update(ByteArray((16 - ct.size % 16) % 16))
        val lens = ByteArray(16)
        for (i in 0 until 8) {
            lens[i] = (aad.size.toLong() ushr (8 * i)).toByte()
            lens[8 + i] = (ct.size.toLong() ushr (8 * i)).toByte()
        }
        mac.update(lens)
        return mac.finish()
    }
}

internal object ChaCha20 {
    private val SIGMA = intArrayOf(0x61707865, 0x3320646e, 0x79622d32, 0x6b206574)

    private fun le32(b: ByteArray, o: Int) =
        (b[o].toInt() and 0xff) or ((b[o + 1].toInt() and 0xff) shl 8) or ((b[o + 2].toInt() and 0xff) shl 16) or ((b[o + 3].toInt() and 0xff) shl 24)

    private fun put32(out: ByteArray, o: Int, v: Int) {
        out[o] = v.toByte(); out[o + 1] = (v ushr 8).toByte(); out[o + 2] = (v ushr 16).toByte(); out[o + 3] = (v ushr 24).toByte()
    }

    private fun qr(s: IntArray, a: Int, b: Int, c: Int, d: Int) {
        s[a] += s[b]; s[d] = Integer.rotateLeft(s[d] xor s[a], 16)
        s[c] += s[d]; s[b] = Integer.rotateLeft(s[b] xor s[c], 12)
        s[a] += s[b]; s[d] = Integer.rotateLeft(s[d] xor s[a], 8)
        s[c] += s[d]; s[b] = Integer.rotateLeft(s[b] xor s[c], 7)
    }

    private fun rounds(s: IntArray) {
        repeat(10) {
            qr(s, 0, 4, 8, 12); qr(s, 1, 5, 9, 13); qr(s, 2, 6, 10, 14); qr(s, 3, 7, 11, 15)
            qr(s, 0, 5, 10, 15); qr(s, 1, 6, 11, 12); qr(s, 2, 7, 8, 13); qr(s, 3, 4, 9, 14)
        }
    }

    private fun state(key: ByteArray, tail: IntArray): IntArray {
        val s = IntArray(16)
        SIGMA.copyInto(s, 0)
        for (i in 0 until 8) s[4 + i] = le32(key, 4 * i)
        tail.copyInto(s, 12)
        return s
    }

    /** One 64-byte keystream block for a 12-byte nonce. */
    fun block(key: ByteArray, nonce12: ByteArray, counter: Int): ByteArray {
        val init = state(key, intArrayOf(counter, le32(nonce12, 0), le32(nonce12, 4), le32(nonce12, 8)))
        val s = init.copyOf()
        rounds(s)
        val out = ByteArray(64)
        for (i in 0 until 16) put32(out, 4 * i, s[i] + init[i])
        return out
    }

    fun xor(key: ByteArray, nonce12: ByteArray, firstCounter: Int, input: ByteArray): ByteArray {
        val out = ByteArray(input.size)
        var counter = firstCounter
        var off = 0
        while (off < input.size) {
            val ks = block(key, nonce12, counter++)
            val n = minOf(64, input.size - off)
            for (i in 0 until n) out[off + i] = (input[off + i].toInt() xor ks[i].toInt()).toByte()
            off += n
        }
        return out
    }

    /** HChaCha20: the XChaCha20 subkey from a 16-byte nonce prefix. */
    fun hchacha20(key: ByteArray, nonce16: ByteArray): ByteArray {
        val s = state(key, intArrayOf(le32(nonce16, 0), le32(nonce16, 4), le32(nonce16, 8), le32(nonce16, 12)))
        rounds(s)
        val out = ByteArray(32)
        for (i in 0 until 4) put32(out, 4 * i, s[i])
        for (i in 0 until 4) put32(out, 16 + 4 * i, s[12 + i])
        return out
    }
}

/** Poly1305 (RFC 8439 section 2.5), 26-bit limbs ("donna"). */
internal class Poly1305(key: ByteArray) {
    private val r0: Long; private val r1: Long; private val r2: Long; private val r3: Long; private val r4: Long
    private val pad = LongArray(4)
    private var h0 = 0L; private var h1 = 0L; private var h2 = 0L; private var h3 = 0L; private var h4 = 0L
    private val buf = ByteArray(16)
    private var bufLen = 0

    init {
        require(key.size == 32)
        r0 = le32(key, 0) and 0x3ffffff
        r1 = (le32(key, 3) ushr 2) and 0x3ffff03
        r2 = (le32(key, 6) ushr 4) and 0x3ffc0ff
        r3 = (le32(key, 9) ushr 6) and 0x3f03fff
        r4 = (le32(key, 12) ushr 8) and 0x00fffff
        for (i in 0 until 4) pad[i] = le32(key, 16 + 4 * i)
    }

    private fun le32(b: ByteArray, o: Int): Long =
        (b[o].toLong() and 0xff) or ((b[o + 1].toLong() and 0xff) shl 8) or ((b[o + 2].toLong() and 0xff) shl 16) or ((b[o + 3].toLong() and 0xff) shl 24)

    private fun block(m: ByteArray, o: Int, hibit: Long) {
        val mask = 0x3ffffffL
        h0 += le32(m, o) and mask
        h1 += (le32(m, o + 3) ushr 2) and mask
        h2 += (le32(m, o + 6) ushr 4) and mask
        h3 += (le32(m, o + 9) ushr 6) and mask
        h4 += (le32(m, o + 12) ushr 8) or hibit
        val s1 = r1 * 5; val s2 = r2 * 5; val s3 = r3 * 5; val s4 = r4 * 5
        val d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1
        var d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2
        var d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3
        var d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4
        var d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0
        var c = d0 ushr 26; h0 = d0 and mask
        d1 += c; c = d1 ushr 26; h1 = d1 and mask
        d2 += c; c = d2 ushr 26; h2 = d2 and mask
        d3 += c; c = d3 ushr 26; h3 = d3 and mask
        d4 += c; c = d4 ushr 26; h4 = d4 and mask
        h0 += c * 5; c = h0 ushr 26; h0 = h0 and mask
        h1 += c
    }

    fun update(data: ByteArray) {
        var off = 0
        if (bufLen > 0) {
            val n = minOf(16 - bufLen, data.size)
            System.arraycopy(data, 0, buf, bufLen, n)
            bufLen += n; off = n
            if (bufLen < 16) return
            block(buf, 0, 1L shl 24); bufLen = 0
        }
        while (data.size - off >= 16) { block(data, off, 1L shl 24); off += 16 }
        if (off < data.size) { System.arraycopy(data, off, buf, 0, data.size - off); bufLen = data.size - off }
    }

    fun finish(): ByteArray {
        val mask = 0x3ffffffL
        if (bufLen > 0) {
            buf[bufLen] = 1
            for (i in bufLen + 1 until 16) buf[i] = 0
            block(buf, 0, 0)
        }
        var c = h1 ushr 26; h1 = h1 and mask
        h2 += c; c = h2 ushr 26; h2 = h2 and mask
        h3 += c; c = h3 ushr 26; h3 = h3 and mask
        h4 += c; c = h4 ushr 26; h4 = h4 and mask
        h0 += c * 5; c = h0 ushr 26; h0 = h0 and mask
        h1 += c
        var g0 = h0 + 5; c = g0 ushr 26; g0 = g0 and mask
        var g1 = h1 + c; c = g1 ushr 26; g1 = g1 and mask
        var g2 = h2 + c; c = g2 ushr 26; g2 = g2 and mask
        var g3 = h3 + c; c = g3 ushr 26; g3 = g3 and mask
        val g4 = h4 + c - (1L shl 26)
        // g4 >= 0 means h >= p: take g (h - p).
        val takeG = (g4 ushr 63) - 1 // all ones when g4 >= 0
        h0 = (h0 and takeG.inv()) or (g0 and takeG)
        h1 = (h1 and takeG.inv()) or (g1 and takeG)
        h2 = (h2 and takeG.inv()) or (g2 and takeG)
        h3 = (h3 and takeG.inv()) or (g3 and takeG)
        h4 = (h4 and takeG.inv()) or ((g4 and mask) and takeG)
        val w0 = (h0 or (h1 shl 26)) and 0xffffffffL
        val w1 = ((h1 ushr 6) or (h2 shl 20)) and 0xffffffffL
        val w2 = ((h2 ushr 12) or (h3 shl 14)) and 0xffffffffL
        val w3 = ((h3 ushr 18) or (h4 shl 8)) and 0xffffffffL
        val out = ByteArray(16)
        var f = w0 + pad[0]
        put(out, 0, f)
        f = w1 + pad[1] + (f ushr 32); put(out, 4, f)
        f = w2 + pad[2] + (f ushr 32); put(out, 8, f)
        f = w3 + pad[3] + (f ushr 32); put(out, 12, f)
        return out
    }

    private fun put(out: ByteArray, o: Int, v: Long) {
        out[o] = v.toByte(); out[o + 1] = (v ushr 8).toByte(); out[o + 2] = (v ushr 16).toByte(); out[o + 3] = (v ushr 24).toByte()
    }
}
