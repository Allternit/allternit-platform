/*
 * Connection flow adapted from Shizuku (https://github.com/RikkaApps/Shizuku,
 * manager/src/main/java/moe/shizuku/manager/adb/AdbClient.kt), Copyright
 * RikkaApps, Apache License 2.0. See THIRD-PARTY-NOTICES.md.
 */
package com.allternit.phonelink.core

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.security.Principal
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedKeyManager
import javax.net.ssl.X509TrustManager

/**
 * Minimal adb client for the phone talking to its own adbd over wireless
 * debugging (127.0.0.1:<connect port>): CNXN → STLS → TLS 1.3 with the
 * per-phone key → CNXN, then `shell:` commands. TLS only — wireless debugging
 * (Android 11+) never offers the legacy AUTH handshake.
 */
class AdbClient(
    private val identity: AdbIdentity,
    private val connect: (host: String, port: Int) -> Socket = { host, port ->
        Socket().apply { connect(InetSocketAddress(host, port), CONNECT_TIMEOUT_MS) }
    },
) : Closeable {
    companion object {
        const val CONNECT_TIMEOUT_MS = 5_000
        const val READ_TIMEOUT_MS = 15_000
    }

    private var socket: Socket? = null
    private var tls: SSLSocket? = null
    private lateinit var input: DataInputStream
    private lateinit var output: DataOutputStream
    private var nextLocalId = 1

    fun connect(host: String, port: Int) {
        val plain = connect.invoke(host, port)
        socket = plain
        plain.tcpNoDelay = true
        plain.soTimeout = READ_TIMEOUT_MS
        input = DataInputStream(plain.getInputStream())
        output = DataOutputStream(plain.getOutputStream())

        write(AdbMessage(AdbProtocol.A_CNXN, AdbProtocol.A_VERSION, AdbProtocol.A_MAXDATA, "host::"))
        var msg = AdbMessage.read(input)
        if (msg.command != AdbProtocol.A_STLS) {
            throw AdbException("adbd did not offer TLS (${AdbMessage.name(msg.command)}); wireless debugging needs Android 11+")
        }
        write(AdbMessage(AdbProtocol.A_STLS, AdbProtocol.A_STLS_VERSION, 0))
        val secured = sslContext().socketFactory.createSocket(plain, host, port, true) as SSLSocket
        secured.useClientMode = true
        tls = secured
        try {
            secured.startHandshake()
            input = DataInputStream(secured.inputStream)
            output = DataOutputStream(secured.outputStream)
            // TLS 1.3: the client's handshake completes before adbd checks our
            // certificate, so an unknown key surfaces as an alert on this first read.
            msg = AdbMessage.read(input)
        } catch (e: java.io.IOException) {
            // An alert, an EOF or a reset: adbd hangs up on an unknown key at any of these points.
            throw AdbException("adbd refused this phone's adb key (not paired, or the pairing was revoked)", e)
        }
        if (msg.command != AdbProtocol.A_CNXN) throw AdbException("expected CNXN after TLS, got ${AdbMessage.name(msg.command)}")
    }

    /** Runs `shell:<command>` and returns its output once adbd closes the stream. */
    fun shell(command: String, maxOutput: Int = 64 * 1024): String {
        val localId = nextLocalId++
        write(AdbMessage(AdbProtocol.A_OPEN, localId, 0, "shell:$command"))
        val out = ByteArrayOutputStream()
        var remoteId = 0
        while (true) {
            val msg = AdbMessage.read(input)
            if (msg.arg1 != localId) continue // another stream's message (none expected)
            when (msg.command) {
                AdbProtocol.A_OKAY -> remoteId = msg.arg0
                AdbProtocol.A_WRTE -> {
                    remoteId = msg.arg0
                    if (out.size() < maxOutput) out.write(msg.data, 0, minOf(msg.data.size, maxOutput - out.size()))
                    write(AdbMessage(AdbProtocol.A_OKAY, localId, remoteId))
                }
                AdbProtocol.A_CLSE -> {
                    if (remoteId != 0) write(AdbMessage(AdbProtocol.A_CLSE, localId, remoteId))
                    return out.toString("UTF-8")
                }
                else -> throw AdbException("unexpected ${AdbMessage.name(msg.command)} on a shell stream")
            }
        }
    }

    private fun write(msg: AdbMessage) {
        output.write(msg.toByteArray())
        output.flush()
    }

    private fun sslContext(): SSLContext {
        val cert = CertificateFactory.getInstance("X.509")
            .generateCertificate(ByteArrayInputStream(identity.certificateDer)) as X509Certificate
        val keyManager = object : X509ExtendedKeyManager() {
            override fun chooseClientAlias(keyType: Array<out String>?, issuers: Array<out Principal>?, socket: Socket?) = "adb"
            override fun getCertificateChain(alias: String?) = arrayOf(cert)
            override fun getPrivateKey(alias: String?): PrivateKey = identity.privateKey
            override fun getClientAliases(keyType: String?, issuers: Array<out Principal>?) = arrayOf("adb")
            override fun getServerAliases(keyType: String?, issuers: Array<out Principal>?): Array<String>? = null
            override fun chooseServerAlias(keyType: String?, issuers: Array<out Principal>?, socket: Socket?): String? = null
        }
        // adbd's certificate is self-signed and per-boot; the connection is to
        // 127.0.0.1 on this same phone, so there is no third party to pin against.
        @Suppress("CustomX509TrustManager")
        val trustAll = object : X509TrustManager {
            override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) = Unit
            override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) = Unit
            override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
        }
        return SSLContext.getInstance("TLSv1.3").apply { init(arrayOf(keyManager), arrayOf(trustAll), SecureRandom()) }
    }

    override fun close() {
        try { tls?.close() } catch (_: Exception) {}
        try { socket?.close() } catch (_: Exception) {}
    }
}

class AdbException(message: String, cause: Throwable? = null) : Exception(message, cause)
