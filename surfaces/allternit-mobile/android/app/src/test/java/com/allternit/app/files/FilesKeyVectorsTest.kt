package com.allternit.app.files

import com.allternit.core.files.Blake3
import com.allternit.core.files.FilesKeyCrypto
import com.allternit.core.files.XChaCha20Poly1305

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.fail
import org.junit.Test

/**
 * The Android Files key crypto against contracts/files-drive/vectors/keys-v1.json (made with
 * node:crypto and @noble, independent of every client): the same bytes the engine, the web
 * keyring and iOS open.
 */
class FilesKeyVectorsTest {
    private val v: JSONObject by lazy { JSONObject(contractFile("vectors/keys-v1.json").readText()) }
    private fun b(s: String) = FilesKeyCrypto.unb64url(s)!!

    @Test
    fun confirmCodeMatchesTheVector() {
        val c = v.getJSONObject("confirm_code")
        assertEquals(c.getString("code"), FilesKeyCrypto.confirmCode(b(c.getString("device_pub"))))
        val e = v.getJSONObject("envelope")
        assertEquals(e.getString("confirm_code"), FilesKeyCrypto.confirmCode(b(e.getString("device_pub"))))
    }

    @Test
    fun envelopeVectorOpensToTheRootKey() {
        val e = v.getJSONObject("envelope")
        val secret = b(e.getString("device_secret"))
        assertArrayEquals(b(e.getString("device_pub")), FilesKeyCrypto.x25519Public(secret))
        val env = FilesKeyCrypto.Envelope.fromJson(e.getJSONObject("envelope"))
        val frk = FilesKeyCrypto.unwrap(env, secret)
        assertEquals("fk_vector1", frk.keyId)
        assertArrayEquals(b(e.getString("frk")), frk.bytes)
    }

    @Test
    fun tamperedEnvelopeIsRefused() {
        val e = v.getJSONObject("envelope")
        val o = JSONObject(e.getJSONObject("envelope").toString())
        // Another device id changes both the KEK and the AAD.
        o.put("device_id", "dev_someone_else")
        try {
            FilesKeyCrypto.unwrap(FilesKeyCrypto.Envelope.fromJson(o), b(e.getString("device_secret")))
            fail("an envelope for another device opened")
        } catch (expected: java.io.IOException) {
        }
    }

    @Test
    fun wrapThenUnwrapRoundTrips() {
        val secret = FilesKeyCrypto.x25519Secret()
        val frk = FilesKeyCrypto.RootKey.generate()
        val env = FilesKeyCrypto.wrap(frk, FilesKeyCrypto.x25519Public(secret), "dev_round_trip", "2026-11-16T00:00:00Z")
        assertEquals(48, FilesKeyCrypto.unb64url(env.wrappedKey)!!.size)
        assertArrayEquals(frk.bytes, FilesKeyCrypto.unwrap(env, secret).bytes)
    }

    @Test
    fun replicaVectorsOpen() {
        val e = v.getJSONObject("envelope")
        val frk = FilesKeyCrypto.unwrap(FilesKeyCrypto.Envelope.fromJson(e.getJSONObject("envelope")), b(e.getString("device_secret")))
        val r = v.getJSONObject("replica")
        val rid = FilesKeyCrypto.replicaId(r.getString("source_computer_id"), r.getString("root_id"), r.getString("host_device_id"))
        assertEquals(r.getString("replica_id"), rid)

        val sealer = FilesKeyCrypto.ChunkSealer(frk, rid)
        val hash = r.getString("plaintext_hash")
        assertEquals(r.getString("locator"), sealer.locator(hash))
        val blob = b(r.getString("blob_b64"))
        val plain = sealer.open(blob, hash)
        assertArrayEquals(b(r.getString("plaintext_b64")), plain)

        val headBlob = b(r.getString("head_blob_b64"))
        assertEquals(frk.keyId, FilesKeyCrypto.headKeyId(headBlob))
        val head = FilesKeyCrypto.openHead(headBlob, frk, rid)
        assertEquals(r.getJSONObject("head").getString("catalog_hash"), head.getString("catalog_hash"))
    }

    @Test
    fun aFlippedCiphertextByteFailsTheTag() {
        val key = ByteArray(32) { it.toByte() }
        val nonce = ByteArray(24) { (it + 1).toByte() }
        val sealed = XChaCha20Poly1305.seal("hello".toByteArray(), key, nonce, "aad".toByteArray())
        sealed[0] = (sealed[0].toInt() xor 1).toByte()
        assertNull(XChaCha20Poly1305.open(sealed, key, nonce, "aad".toByteArray()))
    }

    @Test
    fun hchacha20MatchesTheXChaChaDraftVector() {
        // draft-irtf-cfrg-xchacha-03 section 2.2.1 (HChaCha20 test vector).
        val key = ByteArray(32) { it.toByte() }
        val nonce = FilesKeyCrypto.unhex("000000090000004a0000000031415927")!!
        assertEquals(
            "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc",
            Blake3.hex(XChaCha20Poly1305.hchacha20(key, nonce)),
        )
    }
}
