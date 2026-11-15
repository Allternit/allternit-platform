package com.allternit.phonelink.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test
import java.math.BigInteger
import java.security.KeyFactory
import java.security.SecureRandom
import java.security.spec.NamedParameterSpec
import java.security.spec.XECPrivateKeySpec
import java.security.spec.XECPublicKeySpec
import javax.crypto.KeyAgreement

class X25519Test {
    private fun h(s: String) = Hex.decode(s)!!

    @Test fun rfc7748Section5Vector() {
        val out = X25519.scalarMult(
            h("a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4"),
            h("e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c"),
        )
        assertEquals("c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552", out.toHex())
    }

    @Test fun rfc7748Section6DiffieHellman() {
        val a = h("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val b = h("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
        assertEquals("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a", X25519.publicKey(a).toHex())
        assertEquals("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f", X25519.publicKey(b).toHex())
        val shared = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742"
        assertEquals(shared, X25519.sharedSecret(a, X25519.publicKey(b)).toHex())
        assertEquals(shared, X25519.sharedSecret(b, X25519.publicKey(a)).toHex())
    }

    /** Cross-check against the host JDK's XDH (JDK 11+) on random keys. */
    @Test fun agreesWithJdkXdh() {
        val rng = SecureRandom()
        val kf = KeyFactory.getInstance("XDH")
        repeat(20) {
            val a = X25519.generatePrivateKey(rng)
            val b = X25519.generatePrivateKey(rng)
            val bPub = X25519.publicKey(b)
            val priv = kf.generatePrivate(XECPrivateKeySpec(NamedParameterSpec.X25519, a))
            // XECPublicKeySpec takes u as an integer: decode little-endian (top bit masked).
            val u = bPub.copyOf().also { it[31] = (it[31].toInt() and 0x7f).toByte() }
            val pub = kf.generatePublic(XECPublicKeySpec(NamedParameterSpec.X25519, BigInteger(1, u.reversedArray())))
            val ka = KeyAgreement.getInstance("XDH")
            ka.init(priv)
            ka.doPhase(pub, true)
            assertArrayEquals(ka.generateSecret(), X25519.sharedSecret(a, bPub))
        }
    }

    @Test(expected = IllegalArgumentException::class)
    fun allZeroSharedSecretIsRefused() {
        X25519.sharedSecret(X25519.generatePrivateKey(), ByteArray(32))
    }

    /** token_ct from the reference client's pairing math (ictrl_rpc.py pair()). */
    @Test fun pairingWrapMatchesReference() {
        val key = ByteArray(32) { it.toByte() }
        val clientPriv = h("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb")
        val devicePriv = h("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a")
        val clientPub = X25519.publicKey(clientPriv)
        val devicePub = X25519.publicKey(devicePriv)
        val pad = SecureEnvelope.pairingPad(X25519.sharedSecret(devicePriv, clientPub), clientPub, devicePub)
        assertEquals("6688b1239f4a462cf5e5e22ec13f599949cfb3426a89b2fff73f71f82479a322", SecureEnvelope.xor(key, pad).toHex())
        // The client side derives the same pad from its own private key.
        val clientPad = SecureEnvelope.pairingPad(X25519.sharedSecret(clientPriv, devicePub), clientPub, devicePub)
        assertArrayEquals(pad, clientPad)
    }
}
