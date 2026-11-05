package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.util.Base64

/**
 * Golden values were produced by the reference client
 * (ictrl-fork/Scripts/ictrl_rpc.py: _hmac/_keystream_xor/_x25519), so a pass
 * here means byte-for-byte agreement with the iCTRL wire protocol.
 */
class SecureEnvelopeTest {
    private val key = ByteArray(32) { it.toByte() }
    private val nonce = ByteArray(16) { (100 + it).toByte() }
    private val ts = 1760600000L
    private val body = """{"jsonrpc":"2.0","method":"device_info","params":{},"id":1}"""

    @Test fun tokenIdMatchesReference() {
        assertEquals("630dcd29", SecureEnvelope.tokenId(key))
    }

    @Test fun requestEnvelopeMatchesReferenceClient() {
        val env = SecureEnvelope.sealRequest(key, body, ts, nonce)
        assertEquals("630dcd29", env.getString("tok"))
        assertEquals("DBVHJ9EnUYwJQNVtBBdsReGCHcpwPKPQFl9tkVI6Bq0cjQ6AszE+cFptwA3nl9rOc0xpwOl6obQMvfA=", env.getString("ct"))
        assertEquals("8145aeaefa6d7103d5d92b12744757cde13b9b97121da54f6ca0b19c72a50c20", env.getString("tag"))
        assertEquals(nonce.toHex(), env.getString("nonce"))
        assertEquals(1, env.getInt("v"))
    }

    @Test fun serverOpensReferenceEnvelope() {
        val env = JSONObject()
            .put("v", 1).put("tok", "630dcd29").put("ts", ts).put("nonce", nonce.toHex())
            .put("ct", "DBVHJ9EnUYwJQNVtBBdsReGCHcpwPKPQFl9tkVI6Bq0cjQ6AszE+cFptwA3nl9rOc0xpwOl6obQMvfA=")
            .put("tag", "8145aeaefa6d7103d5d92b12744757cde13b9b97121da54f6ca0b19c72a50c20")
        assertTrue(SecureEnvelope.looksLikeEnvelope(env))
        assertEquals(body, SecureEnvelope.open(SecureEnvelope.parse(env), key))
    }

    @Test fun responseSealMatchesReference() {
        val resp = """{"jsonrpc":"2.0","result":{"ok":true},"id":1}"""
        val sealed = JSONObject(SecureEnvelope.seal(resp, key, nonce, nonce.toHex()))
        assertEquals("jpavZtTVkuUVICa4Oil1aR0dIvpxKOX7bJWd65GOWTDo/e71kMErobG18WLf", sealed.getString("ct"))
        assertEquals("ea8f78268d5a93d5dc5bb1fbb22564b6e26f926b1d9bdbf87495624948381582", sealed.getString("tag"))
        assertEquals(resp, SecureEnvelope.openResponse(key, nonce, sealed))
    }

    @Test fun sealedJsonKeepsSlashesUnescaped() {
        val s = SecureEnvelope.seal("x".repeat(200), key, nonce, nonce.toHex())
        assertFalse(s.contains("\\/"))
    }

    @Test fun tamperedCiphertextIsRejectedBeforeDecrypting() {
        val env = SecureEnvelope.sealRequest(key, body, ts, nonce)
        val ct = Base64.getDecoder().decode(env.getString("ct"))
        ct[0] = (ct[0].toInt() xor 1).toByte()
        env.put("ct", Base64.getEncoder().encodeToString(ct))
        expectFailure("bad_tag") { SecureEnvelope.open(SecureEnvelope.parse(env), key) }
    }

    @Test fun wrongTimestampBreaksTheTag() {
        val env = SecureEnvelope.sealRequest(key, body, ts, nonce).put("ts", ts + 1)
        expectFailure("bad_tag") { SecureEnvelope.open(SecureEnvelope.parse(env), key) }
    }

    @Test fun malformedFieldsAreBadEnvelope() {
        val good = SecureEnvelope.sealRequest(key, body, ts, nonce)
        val cases = listOf(
            JSONObject(good.toString()).put("nonce", "abcd"),
            JSONObject(good.toString()).put("tag", "zz"),
            JSONObject(good.toString()).put("tok", "1234"),
            JSONObject(good.toString()).put("ts", -5),
            JSONObject(good.toString()).put("ts", 1.5),
            JSONObject(good.toString()).put("ct", "not base64!"),
            JSONObject(good.toString()).apply { remove("ts") },
        )
        for (c in cases) expectFailure("bad_envelope") { SecureEnvelope.parse(c) }
    }

    @Test fun envelopeShapeDetection() {
        assertFalse(SecureEnvelope.looksLikeEnvelope(JSONObject().put("jsonrpc", "2.0").put("method", "x")))
        assertFalse(SecureEnvelope.looksLikeEnvelope(JSONObject().put("v", 2).put("tok", "a").put("ct", "b")))
        assertTrue(SecureEnvelope.looksLikeEnvelope(JSONObject().put("v", 1).put("tok", "a").put("ct", "b")))
    }

    @Test fun keystreamSpansSeveralBlocksAndIsAnInvolution() {
        val data = ByteArray(100) { (it * 7).toByte() }
        val once = SecureEnvelope.keystreamXor(key, nonce, data)
        assertFalse(once.contentEquals(data))
        assertArrayEquals(data, SecureEnvelope.keystreamXor(key, nonce, once))
    }

    @Test fun nonUtf8PlaintextIsRejected() {
        // A correctly MACed body that is not UTF-8 must not reach the JSON parser.
        val raw = byteArrayOf(0xff.toByte(), 0xfe.toByte())
        val tok4 = SecureEnvelope.sha256(key).copyOfRange(0, 4)
        val kEnc = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_ENC_REQ.toByteArray(), nonce)
        val kMac = SecureEnvelope.hmac(key, SecureEnvelope.LABEL_MAC.toByteArray(), nonce)
        val ct = SecureEnvelope.keystreamXor(kEnc, nonce, raw)
        val tag = SecureEnvelope.hmac(kMac, "v1".toByteArray(), tok4, SecureEnvelope.be64(ts), nonce, ct)
        val env = JSONObject().put("v", 1).put("tok", tok4.toHex()).put("ts", ts).put("nonce", nonce.toHex())
            .put("ct", Base64.getEncoder().encodeToString(ct)).put("tag", tag.toHex())
        expectFailure("bad_tag") { SecureEnvelope.open(SecureEnvelope.parse(env), key) }
    }

    @Test fun hexRoundTrip() {
        assertEquals("00ff10", Hex.encode(byteArrayOf(0, -1, 16)))
        assertArrayEquals(byteArrayOf(0, -1, 16), Hex.decode("00FF10"))
        assertEquals(null, Hex.decode("abc"))
        assertEquals(null, Hex.decode("zz"))
    }

    private fun expectFailure(code: String, block: () -> Unit) {
        try {
            block()
            fail("expected $code")
        } catch (f: SecureEnvelope.Failure) {
            assertEquals(code, f.code)
        }
    }
}
