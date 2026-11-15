package com.allternit.phonelink.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.KeyPairGenerator
import java.security.Principal
import java.security.PrivateKey
import java.security.PublicKey
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager
import kotlin.concurrent.thread

/**
 * The client against a fake adbd that speaks the real sequence: CNXN → STLS →
 * TLS 1.3 with a required client certificate → CNXN → OPEN shell: → WRTE → CLSE.
 * The fake trusts exactly one RSA public key, the way adbd trusts paired keys.
 */
class AdbClientTest {
    private fun rsaPkcs8(): ByteArray =
        KeyPairGenerator.getInstance("RSA").apply { initialize(2048) }.generateKeyPair().private.encoded

    private fun cert(der: ByteArray) =
        CertificateFactory.getInstance("X.509").generateCertificate(ByteArrayInputStream(der)) as X509Certificate

    /** Runs one adbd session on a background thread; returns the port and the shell commands it saw. */
    private class FakeAdbd(trusted: PublicKey, private val output: String) {
        val server = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))
        val port get() = server.localPort
        val commands = ArrayList<String>()
        @Volatile var failure: Throwable? = null

        private val identity = AdbIdentity.fromPkcs8(
            KeyPairGenerator.getInstance("RSA").apply { initialize(2048) }.generateKeyPair().private.encoded,
        )
        private val ctx: SSLContext = SSLContext.getInstance("TLSv1.3").apply {
            val c = CertificateFactory.getInstance("X.509").generateCertificate(ByteArrayInputStream(identity.certificateDer)) as X509Certificate
            val km = object : X509ExtendedKeyManager() {
                override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?) = "adbd"
                override fun getCertificateChain(alias: String?) = arrayOf(c)
                override fun getPrivateKey(alias: String?): PrivateKey = identity.privateKey
                override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = null
                override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?): String? = null
                override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?) = arrayOf("adbd")
            }
            val tm = object : X509TrustManager {
                override fun checkClientTrusted(chain: Array<out X509Certificate>, authType: String?) {
                    if (!chain[0].publicKey.encoded.contentEquals(trusted.encoded)) throw CertificateException("unknown key")
                }
                override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) = Unit
                override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
            }
            init(arrayOf(km), arrayOf(tm), SecureRandom())
        }

        val worker = thread(isDaemon = true) {
            try {
                server.accept().use { plain ->
                    val pin = DataInputStream(plain.getInputStream())
                    val pout = DataOutputStream(plain.getOutputStream())
                    assertEquals(AdbProtocol.A_CNXN, AdbMessage.read(pin).command)
                    pout.write(AdbMessage(AdbProtocol.A_STLS, AdbProtocol.A_STLS_VERSION, 0).toByteArray())
                    assertEquals(AdbProtocol.A_STLS, AdbMessage.read(pin).command)
                    val tls = ctx.socketFactory.createSocket(plain, "127.0.0.1", plain.port, true) as SSLSocket
                    tls.useClientMode = false
                    tls.needClientAuth = true
                    tls.startHandshake()
                    val tin = DataInputStream(tls.inputStream)
                    val tout = DataOutputStream(tls.outputStream)
                    tout.write(AdbMessage(AdbProtocol.A_CNXN, AdbProtocol.A_VERSION, AdbProtocol.A_MAXDATA, "device::").toByteArray())
                    while (true) {
                        val open = try { AdbMessage.read(tin) } catch (e: java.io.EOFException) { break }
                        assertEquals(AdbProtocol.A_OPEN, open.command)
                        commands.add(String(open.data, Charsets.UTF_8).trimEnd('\u0000'))
                        val local = open.arg0
                        tout.write(AdbMessage(AdbProtocol.A_OKAY, 77, local).toByteArray())
                        tout.write(AdbMessage(AdbProtocol.A_WRTE, 77, local, output.toByteArray()).toByteArray())
                        assertEquals(AdbProtocol.A_OKAY, AdbMessage.read(tin).command)
                        tout.write(AdbMessage(AdbProtocol.A_CLSE, 77, local).toByteArray())
                        assertEquals(AdbProtocol.A_CLSE, AdbMessage.read(tin).command)
                    }
                }
            } catch (t: Throwable) {
                failure = t
            } finally {
                server.close()
            }
        }
    }

    @Test fun pairedKeyConnectsOverTlsAndRunsShellCommands() {
        val identity = AdbIdentity.fromPkcs8(rsaPkcs8())
        val adbd = FakeAdbd(identity.publicKey, output = "ok\n")
        AdbClient(identity).use { c ->
            c.connect("127.0.0.1", adbd.port)
            assertEquals("ok\n", c.shell("pkill -f x"))
            assertEquals("ok\n", c.shell("setsid app_process / Main"))
        }
        adbd.worker.join(5_000)
        adbd.failure?.let { throw it }
        assertEquals(listOf("shell:pkill -f x", "shell:setsid app_process / Main"), adbd.commands)
    }

    @Test fun anUnpairedKeyIsRefusedWithAPlainMessage() {
        val paired = AdbIdentity.fromPkcs8(rsaPkcs8())
        val stranger = AdbIdentity.fromPkcs8(rsaPkcs8())
        val adbd = FakeAdbd(paired.publicKey, output = "")
        try {
            AdbClient(stranger).use { it.connect("127.0.0.1", adbd.port) }
            fail("expected the handshake to fail")
        } catch (e: AdbException) {
            assertTrue(e.message!!.contains("refused"))
        }
    }

    @Test fun certificateIsSelfSignedOverTheSameKey() {
        val identity = AdbIdentity.fromPkcs8(rsaPkcs8())
        val c = cert(identity.certificateDer)
        c.verify(identity.publicKey) // throws if the signature or encoding is wrong
        assertArrayEquals(identity.publicKey.encoded, c.publicKey.encoded)
        assertEquals("CN=00", c.subjectX500Principal.name)
        c.checkValidity()
    }

    @Test fun pemTextAndBareBase64BothLoad() {
        val der = rsaPkcs8()
        val b64 = java.util.Base64.getMimeEncoder(64, "\n".toByteArray()).encodeToString(der)
        val pem = "-----BEGIN PRIVATE KEY-----\n$b64\n-----END PRIVATE KEY-----\n"
        assertArrayEquals(AdbIdentity.fromPkcs8(der).publicKey.encoded, AdbIdentity.fromPem(pem).publicKey.encoded)
    }

    @Test fun messagesRoundTripAndRejectBadMagic() {
        val m = AdbMessage(AdbProtocol.A_OPEN, 1, 0, "shell:id")
        val back = AdbMessage.read(DataInputStream(ByteArrayInputStream(m.toByteArray())))
        assertEquals(AdbProtocol.A_OPEN, back.command)
        assertEquals("shell:id\u0000", String(back.data))
        val bytes = m.toByteArray().also { it[20] = 0 }
        try {
            AdbMessage.read(DataInputStream(ByteArrayInputStream(bytes)))
            fail("bad magic must be rejected")
        } catch (e: IllegalArgumentException) {
        }
    }
}
