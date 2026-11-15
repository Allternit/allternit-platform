package com.allternit.phonelink.core

import java.io.ByteArrayOutputStream
import java.security.KeyFactory
import java.security.PrivateKey
import java.security.PublicKey
import java.security.Signature
import java.security.interfaces.RSAPrivateCrtKey
import java.security.spec.PKCS8EncodedKeySpec
import java.security.spec.RSAPublicKeySpec

/**
 * The phone's own adb identity: the per-phone RSA key Desktop pairs with
 * wireless debugging during cable setup, plus the self-signed certificate adb
 * TLS wants. adbd checks only the public key in the certificate, so the
 * certificate is the smallest valid X.509 v1 (CN=00, like adb's own), built
 * here in DER to keep the app free of BouncyCastle.
 */
class AdbIdentity(val privateKey: PrivateKey, val publicKey: PublicKey, val certificateDer: ByteArray) {
    companion object {
        /** @param pkcs8 the private key as DER PKCS#8 (adb's `adbkey` file, base64-decoded PEM body). */
        fun fromPkcs8(pkcs8: ByteArray): AdbIdentity {
            val kf = KeyFactory.getInstance("RSA")
            val priv = kf.generatePrivate(PKCS8EncodedKeySpec(pkcs8))
            val crt = priv as? RSAPrivateCrtKey ?: throw IllegalArgumentException("adb key must be an RSA private key")
            val pub = kf.generatePublic(RSAPublicKeySpec(crt.modulus, crt.publicExponent))
            return AdbIdentity(priv, pub, AdbCert.selfSigned(priv, pub))
        }

        /** Accepts the `adbkey` PEM text or its bare base64 body. */
        fun fromPem(text: String): AdbIdentity {
            val body = text.lineSequence().filterNot { it.startsWith("-----") }.joinToString("").trim()
            return fromPkcs8(java.util.Base64.getDecoder().decode(body))
        }
    }
}

object AdbCert {
    private val SHA256_WITH_RSA = byteArrayOf(0x2a, 0x86.toByte(), 0x48, 0x86.toByte(), 0xf7.toByte(), 0x0d, 0x01, 0x01, 0x0b)
    private val COMMON_NAME = byteArrayOf(0x55, 0x04, 0x03)

    fun selfSigned(privateKey: PrivateKey, publicKey: PublicKey): ByteArray {
        val algorithm = seq(tlv(0x06, SHA256_WITH_RSA), tlv(0x05, ByteArray(0)))
        val name = seq(tlv(0x31, seq(tlv(0x06, COMMON_NAME), tlv(0x0c, "00".toByteArray()))))
        val validity = seq(tlv(0x17, "700101000000Z".toByteArray()), tlv(0x17, "491231235959Z".toByteArray()))
        val tbs = seq(tlv(0x02, byteArrayOf(1)), algorithm, name, validity, name, publicKey.encoded)
        val sig = Signature.getInstance("SHA256withRSA").run {
            initSign(privateKey)
            update(tbs)
            sign()
        }
        return seq(tbs, algorithm, tlv(0x03, byteArrayOf(0) + sig))
    }

    private fun seq(vararg parts: ByteArray): ByteArray = tlv(0x30, parts.fold(ByteArray(0)) { a, b -> a + b })

    private fun tlv(tag: Int, value: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        out.write(tag)
        val n = value.size
        when {
            n < 0x80 -> out.write(n)
            n < 0x100 -> { out.write(0x81); out.write(n) }
            n < 0x10000 -> { out.write(0x82); out.write(n shr 8); out.write(n and 0xff) }
            else -> { out.write(0x83); out.write(n shr 16); out.write((n shr 8) and 0xff); out.write(n and 0xff) }
        }
        out.write(value)
        return out.toByteArray()
    }
}
