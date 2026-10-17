package com.allternit.phonelink.core

import java.math.BigInteger
import java.security.SecureRandom

/**
 * X25519 (RFC 7748 §5) in pure Kotlin on java.math.BigInteger.
 *
 * Why in-tree: java.security "XDH" only exists on Android API 33+, and minSdk
 * is 26. It is used once per pairing, with a fresh device key each time, so
 * BigInteger's non-constant-time arithmetic exposes nothing reusable (see
 * README "Security model"). Verified against the RFC 7748 vectors and the
 * host JDK's XDH in X25519Test.
 */
object X25519 {
    private val P: BigInteger = BigInteger.ONE.shiftLeft(255).subtract(BigInteger.valueOf(19))
    private val A24: BigInteger = BigInteger.valueOf(121665)
    private val TWO: BigInteger = BigInteger.valueOf(2)

    val BASE_POINT: ByteArray = ByteArray(32).also { it[0] = 9 }

    fun generatePrivateKey(random: SecureRandom = SecureRandom()): ByteArray =
        ByteArray(32).also { random.nextBytes(it) }

    fun publicKey(privateKey: ByteArray): ByteArray = scalarMult(privateKey, BASE_POINT)

    /** X25519(k, u). Throws if the result is all zeros (low-order peer point). */
    fun sharedSecret(privateKey: ByteArray, peerPublic: ByteArray): ByteArray {
        val out = scalarMult(privateKey, peerPublic)
        if (out.all { it.toInt() == 0 }) throw IllegalArgumentException("X25519 produced the all-zero secret")
        return out
    }

    fun scalarMult(scalar: ByteArray, u: ByteArray): ByteArray {
        require(scalar.size == 32 && u.size == 32) { "X25519 inputs must be 32 bytes" }
        val k = decodeScalar(scalar)
        val x1 = decodeU(u)
        var x2 = BigInteger.ONE
        var z2 = BigInteger.ZERO
        var x3 = x1
        var z3 = BigInteger.ONE
        var swap = 0
        for (t in 254 downTo 0) {
            val kt = if (k.testBit(t)) 1 else 0
            swap = swap xor kt
            if (swap == 1) {
                val tx = x2; x2 = x3; x3 = tx
                val tz = z2; z2 = z3; z3 = tz
            }
            swap = kt
            val a = x2.add(z2).mod(P)
            val aa = a.multiply(a).mod(P)
            val b = x2.subtract(z2).mod(P)
            val bb = b.multiply(b).mod(P)
            val e = aa.subtract(bb).mod(P)
            val c = x3.add(z3).mod(P)
            val d = x3.subtract(z3).mod(P)
            val da = d.multiply(a).mod(P)
            val cb = c.multiply(b).mod(P)
            val s1 = da.add(cb).mod(P)
            x3 = s1.multiply(s1).mod(P)
            val s2 = da.subtract(cb).mod(P)
            z3 = x1.multiply(s2.multiply(s2)).mod(P)
            x2 = aa.multiply(bb).mod(P)
            z2 = e.multiply(aa.add(A24.multiply(e))).mod(P)
        }
        if (swap == 1) {
            val tx = x2; x2 = x3; x3 = tx
            val tz = z2; z2 = z3; z3 = tz
        }
        val result = x2.multiply(z2.modPow(P.subtract(TWO), P)).mod(P)
        return encodeU(result)
    }

    private fun decodeScalar(k: ByteArray): BigInteger {
        val b = k.copyOf()
        b[0] = (b[0].toInt() and 248).toByte()
        b[31] = ((b[31].toInt() and 127) or 64).toByte()
        return littleEndian(b)
    }

    private fun decodeU(u: ByteArray): BigInteger {
        val b = u.copyOf()
        b[31] = (b[31].toInt() and 127).toByte()
        return littleEndian(b).mod(P)
    }

    private fun littleEndian(b: ByteArray): BigInteger = BigInteger(1, b.reversedArray())

    private fun encodeU(v: BigInteger): ByteArray {
        val be = v.toByteArray() // big-endian, maybe with a leading sign byte
        val out = ByteArray(32)
        var i = 0
        while (i < 32 && i < be.size) {
            out[i] = be[be.size - 1 - i]
            i++
        }
        return out
    }
}
