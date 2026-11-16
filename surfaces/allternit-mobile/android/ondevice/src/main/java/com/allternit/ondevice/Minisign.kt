package com.allternit.ondevice

import org.bouncycastle.crypto.digests.Blake2bDigest
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import java.util.Base64

/**
 * Minisign check (https://jedisct1.github.io/minisign/), the format Freeweights signs manifests
 * with (`<id>.json.minisig`). Port of iOS FreeweightsVerify/Minisign.swift: checks the file
 * signature (prehashed "ED" = BLAKE2b-512, or legacy "Ed"), the global signature over the trusted
 * comment, and the key id. Throws [Protocol.Failure] with code `bad_signature`.
 */
object Minisign {
    /** freeweights-v0, key id 2DC28CC7B26A283B (freeweights spec/keys/freeweights-v0.pub). Rotation adds a key here. */
    val FREEWEIGHTS_KEYS = listOf("RWQ7KGqyx4zCLXtUFOqvfv8L3g+8fpKJ1FsuxPyOYMF+4KufbUxKq1Oq")

    private fun fail(why: String): Nothing =
        throw Protocol.Failure("bad_signature", "The Freeweights manifest failed its signature check ($why), so the download was refused.")

    private class Key(val id: ByteArray, val pub: Ed25519PublicKeyParameters)

    private fun key(b64: String): Key {
        val raw = try { Base64.getDecoder().decode(b64.trim()) } catch (e: IllegalArgumentException) { fail("bad public key") }
        if (raw.size != 42 || raw[0] != 0x45.toByte() || raw[1] != 0x64.toByte()) fail("bad public key")
        return Key(raw.copyOfRange(2, 10), Ed25519PublicKeyParameters(raw, 10))
    }

    private fun ed(pub: Ed25519PublicKeyParameters, msg: ByteArray, sig: ByteArray): Boolean {
        val v = Ed25519Signer()
        v.init(false, pub)
        v.update(msg, 0, msg.size)
        return v.verifySignature(sig)
    }

    private fun blake2b512(msg: ByteArray): ByteArray {
        val d = Blake2bDigest(512)
        d.update(msg, 0, msg.size)
        return ByteArray(64).also { d.doFinal(it, 0) }
    }

    /** Verifies [message] against the text of a .minisig file with one key. Returns the trusted comment. */
    fun verify(message: ByteArray, signatureFile: String, publicKeyB64: String): String {
        val k = key(publicKeyB64)
        val lines = signatureFile.lines().filter { it.isNotBlank() }
        if (lines.size < 4 || !lines[0].startsWith("untrusted comment:") || !lines[2].startsWith("trusted comment: ")) fail("not a minisign file")
        val sigRaw = try { Base64.getDecoder().decode(lines[1].trim()) } catch (e: IllegalArgumentException) { fail("not a minisign file") }
        val globalSig = try { Base64.getDecoder().decode(lines[3].trim()) } catch (e: IllegalArgumentException) { fail("not a minisign file") }
        if (sigRaw.size != 74 || globalSig.size != 64) fail("not a minisign file")
        if (sigRaw[0] != 0x45.toByte()) fail("unknown algorithm")
        val prehashed = when (sigRaw[1]) { 0x44.toByte() -> true; 0x64.toByte() -> false; else -> fail("unknown algorithm") }
        if (!sigRaw.copyOfRange(2, 10).contentEquals(k.id)) throw KeyMismatch()
        val sig = sigRaw.copyOfRange(10, 74)
        if (!ed(k.pub, if (prehashed) blake2b512(message) else message, sig)) fail("signature does not match")
        val trusted = lines[2].removePrefix("trusted comment: ")
        if (!ed(k.pub, sig + trusted.toByteArray(Charsets.UTF_8), globalSig)) fail("trusted comment does not match")
        return trusted
    }

    private class KeyMismatch : Exception()

    fun verifyFreeweights(message: ByteArray, signatureFile: String): String {
        for (k in FREEWEIGHTS_KEYS) {
            try { return verify(message, signatureFile, k) } catch (_: KeyMismatch) { continue }
        }
        fail("signed by a different key")
    }
}
