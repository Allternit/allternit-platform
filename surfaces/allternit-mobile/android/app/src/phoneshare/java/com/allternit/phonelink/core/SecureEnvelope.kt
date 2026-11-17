package com.allternit.phonelink.core

import org.json.JSONObject
import java.nio.ByteBuffer
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.security.MessageDigest
import java.security.SecureRandom
import java.util.Base64
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/**
 * iCTRL protocol v1 envelope, byte-for-byte compatible with
 * ictrl-fork/iCTRLKit/SecureEnvelope.swift (server) and Scripts/ictrl_rpc.py /
 * Desktop's ictrl-client.ts (clients).
 *
 *     K          = 32-byte key handed out at pairing
 *     tok        = first 4 bytes of SHA-256(K), 8 hex chars (lookup id only)
 *     nonce      = 16 random bytes per request
 *     ts         = unix seconds (server accepts |ts - now| <= 120)
 *     K_enc_req  = HMAC(K, "ictrl-v1|enc-req"  || nonce)
 *     K_enc_resp = HMAC(K, "ictrl-v1|enc-resp" || nonce)
 *     K_mac      = HMAC(K, "ictrl-v1|mac"      || nonce)
 *     keystream  = HMAC(Kx, nonce || be32(0)) || HMAC(Kx, nonce || be32(1)) || ...
 *     ct         = body XOR keystream(K_enc_req)
 *     tag        = HMAC(K_mac, "v1" || tok_4B || be64(ts) || nonce || ct)
 *     reply tag  = HMAC(K_mac, "resp" || nonce || resp_ct)
 */
object SecureEnvelope {
    const val LABEL_MAC = "ictrl-v1|mac"
    const val LABEL_ENC_REQ = "ictrl-v1|enc-req"
    const val LABEL_ENC_RESP = "ictrl-v1|enc-resp"
    const val LABEL_PAIR = "ictrl-pair"
    const val LABEL_WRAP = "ictrl-wrap"

    /** Requests older/newer than this many seconds are rejected. */
    const val TIMESTAMP_WINDOW_S = 120L

    class Failure(val code: String, message: String) : Exception(message)

    private val rng = SecureRandom()

    // ── Primitives ──────────────────────────────────────────────────────────

    fun hmac(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        for (p in parts) mac.update(p)
        return mac.doFinal()
    }

    fun sha256(data: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(data)

    fun constantTimeEquals(a: ByteArray, b: ByteArray): Boolean = MessageDigest.isEqual(a, b)

    /** PRF-CTR stream: keystream_i = HMAC-SHA256(key, nonce || be32(i)). */
    fun keystreamXor(key: ByteArray, nonce: ByteArray, data: ByteArray): ByteArray {
        val out = ByteArray(data.size)
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        var offset = 0
        var counter = 0
        while (offset < data.size) {
            mac.update(nonce)
            mac.update(be32(counter))
            val block = mac.doFinal()
            var j = 0
            while (j < 32 && offset + j < data.size) {
                out[offset + j] = (data[offset + j].toInt() xor block[j].toInt()).toByte()
                j++
            }
            offset += 32
            counter++
        }
        return out
    }

    fun be32(v: Int): ByteArray = ByteBuffer.allocate(4).putInt(v).array()
    fun be64(v: Long): ByteArray = ByteBuffer.allocate(8).putLong(v).array()

    fun tokenId(key: ByteArray): String = sha256(key).copyOfRange(0, 4).toHex()

    fun randomBytes(n: Int): ByteArray = ByteArray(n).also { rng.nextBytes(it) }

    private fun utf8(s: String) = s.toByteArray(Charsets.UTF_8)

    // ── Envelope shape ──────────────────────────────────────────────────────

    /** True if the object looks like a v1 request envelope (v == 1, tok + ct strings). */
    fun looksLikeEnvelope(obj: JSONObject): Boolean {
        val v = obj.opt("v")
        val isOne = v is Number && v.toDouble() == 1.0
        return isOne && obj.opt("tok") is String && obj.opt("ct") is String
    }

    class Parsed(
        val tokId: String,
        val ts: Long,
        val nonceHex: String,
        val nonce: ByteArray,
        val ct: ByteArray,
        val tag: ByteArray,
        val tok4: ByteArray,
    )

    fun parse(obj: JSONObject): Parsed {
        val bad = Failure("bad_envelope", "Malformed envelope fields")
        val tokId = obj.opt("tok") as? String ?: throw bad
        val tsRaw = obj.opt("ts") as? Number ?: throw bad
        val ts = tsRaw.toLong()
        if (ts < 0 || tsRaw.toDouble() != ts.toDouble()) throw bad
        val nonceHex = obj.opt("nonce") as? String ?: throw bad
        val ctB64 = obj.opt("ct") as? String ?: throw bad
        val tagHex = obj.opt("tag") as? String ?: throw bad
        val nonce = Hex.decode(nonceHex)?.takeIf { it.size == 16 } ?: throw bad
        val ct = try { Base64.getDecoder().decode(ctB64) } catch (e: IllegalArgumentException) { throw bad }
        val tag = Hex.decode(tagHex)?.takeIf { it.size == 32 } ?: throw bad
        val tok4 = Hex.decode(tokId)?.takeIf { it.size == 4 } ?: throw bad
        return Parsed(tokId, ts, nonceHex, nonce, ct, tag, tok4)
    }

    // ── Open / seal ─────────────────────────────────────────────────────────

    /** Verifies the tag (before decrypting) and returns the plaintext body. */
    fun open(p: Parsed, key: ByteArray): String {
        val kMac = hmac(key, utf8(LABEL_MAC), p.nonce)
        val expected = hmac(kMac, utf8("v1"), p.tok4, be64(p.ts), p.nonce, p.ct)
        if (!constantTimeEquals(expected, p.tag)) throw Failure("bad_tag", "MAC verification failed")
        val kEncReq = hmac(key, utf8(LABEL_ENC_REQ), p.nonce)
        val plain = keystreamXor(kEncReq, p.nonce, p.ct)
        return try {
            Charsets.UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT)
                .decode(ByteBuffer.wrap(plain)).toString()
        } catch (e: CharacterCodingException) {
            throw Failure("bad_tag", "Decrypted payload is not UTF-8")
        }
    }

    /** Encrypts a response under the request's nonce with the response-direction key. */
    fun seal(responseJson: String, key: ByteArray, nonce: ByteArray, nonceHex: String): String {
        val kMac = hmac(key, utf8(LABEL_MAC), nonce)
        val kEncResp = hmac(key, utf8(LABEL_ENC_RESP), nonce)
        val ct = keystreamXor(kEncResp, nonce, utf8(responseJson))
        val tag = hmac(kMac, utf8("resp"), nonce, ct)
        // Hand-built: hex and standard base64 need no JSON escaping, and this
        // keeps "/" unescaped exactly like the Swift server's output.
        return "{\"v\":1,\"nonce\":\"$nonceHex\",\"ct\":\"${Base64.getEncoder().encodeToString(ct)}\",\"tag\":\"${tag.toHex()}\"}"
    }

    // ── Client side (tests, conformance) ───────────────────────────────────

    /** Builds a request envelope exactly as ictrl_rpc.py does. */
    fun sealRequest(key: ByteArray, body: String, ts: Long, nonce: ByteArray): JSONObject {
        val tok4 = sha256(key).copyOfRange(0, 4)
        val kEnc = hmac(key, utf8(LABEL_ENC_REQ), nonce)
        val kMac = hmac(key, utf8(LABEL_MAC), nonce)
        val ct = keystreamXor(kEnc, nonce, utf8(body))
        val tag = hmac(kMac, utf8("v1"), tok4, be64(ts), nonce, ct)
        return JSONObject()
            .put("v", 1)
            .put("tok", tok4.toHex())
            .put("ts", ts)
            .put("nonce", nonce.toHex())
            .put("ct", Base64.getEncoder().encodeToString(ct))
            .put("tag", tag.toHex())
    }

    /** Verifies and decrypts a response envelope (client side). */
    fun openResponse(key: ByteArray, nonce: ByteArray, envelope: JSONObject): String {
        val ct = Base64.getDecoder().decode(envelope.getString("ct"))
        val tag = Hex.decode(envelope.getString("tag")) ?: throw Failure("bad_response_mac", "bad tag hex")
        val kMac = hmac(key, utf8(LABEL_MAC), nonce)
        if (!constantTimeEquals(hmac(kMac, utf8("resp"), nonce, ct), tag)) {
            throw Failure("bad_response_mac", "response MAC mismatch")
        }
        val kEncResp = hmac(key, utf8(LABEL_ENC_RESP), nonce)
        return String(keystreamXor(kEncResp, nonce, ct), Charsets.UTF_8)
    }

    /** Pairing pad: HMAC(HMAC(shared, "ictrl-pair" || c_pub || d_pub), "ictrl-wrap"). */
    fun pairingPad(shared: ByteArray, clientPub: ByteArray, devicePub: ByteArray): ByteArray {
        val kWrap = hmac(shared, utf8(LABEL_PAIR), clientPub, devicePub)
        return hmac(kWrap, utf8(LABEL_WRAP))
    }

    fun xor(a: ByteArray, b: ByteArray): ByteArray =
        ByteArray(minOf(a.size, b.size)) { (a[it].toInt() xor b[it].toInt()).toByte() }
}
