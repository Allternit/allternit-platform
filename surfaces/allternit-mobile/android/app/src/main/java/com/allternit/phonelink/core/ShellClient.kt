package com.allternit.phonelink.core

import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.Socket

/** The app's side of [ShellServerProtocol]: ping, and raw streams for scrcpy. */
class ShellClient(private val port: () -> Int, private val token: () -> String) {

    /** True when a shell server answers with our token. */
    fun ping(): Boolean = try {
        open("PING", timeoutMs = 1_500).use { s ->
            ShellServerProtocol.readLine(s.getInputStream())?.startsWith("OK ") == true
        }
    } catch (e: IOException) {
        false
    }

    /** Asks the server to stop its scrcpy sessions and exit. True when it acknowledged. */
    fun quit(): Boolean = try {
        open("QUIT", timeoutMs = 1_500).use { s -> ShellServerProtocol.readLine(s.getInputStream()) == "OK" }
    } catch (e: IOException) {
        false
    }

    /**
     * Sends hello + [command] and waits for `OK`. Returns the socket in raw mode
     * (scrcpy bytes from here on), or throws with the server's ERR message.
     */
    fun openRaw(command: String): Socket {
        val s = open(command, timeoutMs = 10_000)
        val line = try { ShellServerProtocol.readLine(s.getInputStream()) } catch (e: IOException) { s.close(); throw e }
        if (line != "OK") {
            s.close()
            throw IOException(line?.removePrefix("ERR ")?.ifEmpty { null } ?: "shell server closed the stream")
        }
        s.soTimeout = 0
        return s
    }

    private fun open(command: String, timeoutMs: Int): Socket {
        val s = Socket()
        try {
            s.connect(InetSocketAddress(InetAddress.getByName("127.0.0.1"), port()), timeoutMs)
            s.soTimeout = timeoutMs
            s.tcpNoDelay = true
            s.getOutputStream().apply {
                write(ShellServerProtocol.hello(token()).toByteArray())
                write("$command\n".toByteArray())
                flush()
            }
            return s
        } catch (e: IOException) {
            s.close()
            throw e
        }
    }
}
