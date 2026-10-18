package com.allternit.phonelink.shell

import android.net.LocalSocket
import android.net.LocalSocketAddress
import com.allternit.phonelink.core.Pipe
import com.allternit.phonelink.core.ShellServerProtocol
import com.allternit.phonelink.core.ShellServerProtocol.Command
import java.io.File
import java.io.IOException
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.ConcurrentHashMap
import kotlin.system.exitProcess

/**
 * The Phone Link shell server: runs as the shell user, started by the app
 * over the phone's own wireless debugging (ShellServerLauncher) with
 *
 *   setsid app_process -Djava.class.path=<base.apk> / \
 *     com.allternit.phonelink.shell.ShellServerMain <token> <port>
 *
 * setsid detaches it from adbd, so it keeps running when wireless debugging
 * turns off (leaving Wi-Fi) and stops only at reboot. It holds the shell
 * user's rights the app lacks — starting scrcpy-server for live H.264 video
 * and real touch/key injection — and hands them out only to callers that
 * present the app's token on 127.0.0.1 (see ShellServerProtocol).
 */
object ShellServerMain {
    private val sessions = ConcurrentHashMap<String, Process>()

    @JvmStatic
    fun main(args: Array<String>) {
        val token = args.getOrNull(0)
        val port = args.getOrNull(1)?.toIntOrNull() ?: ShellServerProtocol.DEFAULT_PORT
        if (token == null || !ShellServerProtocol.isValidToken(token)) {
            System.err.println("usage: ShellServerMain <64-hex token> [port]")
            exitProcess(2)
        }
        val server = try {
            ServerSocket().apply {
                reuseAddress = true
                bind(InetSocketAddress(InetAddress.getByName("127.0.0.1"), port), 16)
            }
        } catch (e: IOException) {
            // Already running (the port is ours): the launcher re-launches freely.
            System.err.println("shell server: port $port busy, exiting")
            exitProcess(0)
        }
        println("shell server ${ShellServerProtocol.VERSION} listening on 127.0.0.1:$port pid=${android.os.Process.myPid()}")
        while (true) {
            val s = try { server.accept() } catch (e: IOException) { continue }
            Thread({ serve(s, token) }, "shell-conn").apply { isDaemon = true; start() }
        }
    }

    private fun serve(s: Socket, token: String) {
        try {
            s.soTimeout = 10_000
            s.tcpNoDelay = true
            val input = s.getInputStream()
            val output = s.getOutputStream()
            val hello = ShellServerProtocol.readLine(input)
            if (hello == null || !ShellServerProtocol.checkHello(hello, token)) {
                s.close()
                return
            }
            val line = ShellServerProtocol.readLine(input) ?: run { s.close(); return }
            val cmd = ShellServerProtocol.parseCommand(line).getOrElse { e ->
                output.write("ERR ${e.message}\n".toByteArray())
                s.close()
                return
            }
            when (cmd) {
                Command.Ping -> {
                    output.write("OK ${ShellServerProtocol.VERSION}\n".toByteArray())
                    s.close()
                }
                Command.Quit -> {
                    output.write("OK\n".toByteArray())
                    s.close()
                    sessions.values.forEach { it.destroy() }
                    exitProcess(0)
                }
                is Command.Scrcpy -> {
                    if (!File(ShellServerProtocol.SCRCPY_JAR).isFile) {
                        output.write("ERR scrcpy-server is not on the phone; run Phone Link setup again\n".toByteArray())
                        s.close()
                        return
                    }
                    sessions.remove(cmd.scid)?.destroy()
                    val proc = ProcessBuilder(listOf("app_process", "/", "com.genymobile.scrcpy.Server") + ShellServerProtocol.scrcpyServerArgs(cmd))
                        .redirectErrorStream(true)
                        .apply { environment()["CLASSPATH"] = ShellServerProtocol.SCRCPY_JAR }
                        .start()
                    sessions[cmd.scid] = proc
                    Thread({ drain(proc) }, "scrcpy-log").apply { isDaemon = true; start() }
                    val local = connectScrcpy(cmd.scid, proc)
                    if (local == null) {
                        proc.destroy()
                        sessions.remove(cmd.scid, proc)
                        output.write("ERR scrcpy-server did not start\n".toByteArray())
                        s.close()
                        return
                    }
                    output.write("OK\n".toByteArray())
                    output.flush()
                    raw(s, local)
                    // The video socket is the session: when it ends, so does scrcpy.
                    proc.destroy()
                    sessions.remove(cmd.scid, proc)
                }
                is Command.Attach -> {
                    val proc = sessions[cmd.scid]
                    val local = proc?.let { connectScrcpy(cmd.scid, it) }
                    if (local == null) {
                        output.write("ERR no such session\n".toByteArray())
                        s.close()
                        return
                    }
                    output.write("OK\n".toByteArray())
                    output.flush()
                    raw(s, local)
                }
            }
        } catch (_: IOException) {
            try { s.close() } catch (_: IOException) {}
        }
    }

    /** scrcpy-server needs a moment after spawn before its socket accepts. */
    private fun connectScrcpy(scid: String, proc: Process): LocalSocket? {
        repeat(40) {
            if (!proc.isAlive) return null
            val ls = LocalSocket()
            try {
                ls.connect(LocalSocketAddress(ShellServerProtocol.socketName(scid), LocalSocketAddress.Namespace.ABSTRACT))
                return ls
            } catch (_: IOException) {
                try { ls.close() } catch (_: IOException) {}
                Thread.sleep(150)
            }
        }
        return null
    }

    private fun raw(remote: Socket, local: LocalSocket) {
        remote.soTimeout = 0
        Pipe.join(remote.getInputStream(), remote.getOutputStream(), local.inputStream, local.outputStream) {
            try { remote.close() } catch (_: IOException) {}
            try { local.close() } catch (_: IOException) {}
        }
    }

    private fun drain(proc: Process) {
        try {
            proc.inputStream.bufferedReader().forEachLine { println("scrcpy: $it") }
        } catch (_: IOException) {
        }
    }
}
