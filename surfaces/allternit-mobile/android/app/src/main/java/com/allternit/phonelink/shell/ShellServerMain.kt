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
 * user's rights the app lacks — starting scrcpy-server for live H.264 video,
 * real touch/key injection, texts, calls and inbox reads (only after Desktop's
 * approval gate) and turning the app's accessibility service back on —
 * and hands them out only to callers that present the app's token on 127.0.0.1 (see ShellServerProtocol).
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
                Command.A11yOn -> {
                    val current = exec(listOf("settings", "get", "secure", "enabled_accessibility_services")).second
                    val next = ShellServerProtocol.withAccessibility(current)
                    val put = exec(listOf("settings", "put", "secure", "enabled_accessibility_services", next))
                    val on = exec(listOf("settings", "put", "secure", "accessibility_enabled", "1"))
                    reply(output, if (put.first == 0 && on.first == 0) "OK" else "ERR settings put failed: ${(put.second + on.second).trim().take(200)}")
                    s.close()
                }
                is Command.Key -> {
                    val r = exec(listOf("input", "keyevent", cmd.code))
                    reply(output, if (r.first == 0) "OK" else "ERR input keyevent failed: ${r.second.trim().take(200)}")
                    s.close()
                }
                is Command.Sms -> {
                    val sub = if (cmd.subId >= 0) cmd.subId else defaultSmsSubscription()
                    if (sub == null) {
                        reply(output, "ERR could not tell which SIM to send from; pass subscription_id")
                    } else {
                        val r = exec(ShellServerProtocol.smsArgv(sub, cmd.to, cmd.body))
                        reply(output, if (r.first == 0 && ShellServerProtocol.smsAccepted(r.second)) "OK" else "ERR the phone did not accept the text (this Android build may use a different SMS service layout)")
                    }
                    s.close()
                }
                is Command.Dial -> {
                    val r = exec(ShellServerProtocol.dialArgv(cmd.number, cmd.subId))
                    val refused = r.first != 0 || Regex("Error|Exception|Permission Denial").containsMatchIn(r.second)
                    reply(output, if (refused) "ERR ${r.second.trim().take(200).ifEmpty { "the phone refused to place the call" }}" else "OK")
                    s.close()
                }
                is Command.SmsRead -> {
                    val r = exec(ShellServerProtocol.smsReadArgv(cmd.sinceMs, cmd.from), maxBytes = 256 * 1024)
                    if (r.first == 0) {
                        reply(output, "OK " + java.util.Base64.getEncoder().encodeToString(r.second.toByteArray(Charsets.UTF_8)))
                    } else {
                        reply(output, "ERR content query failed: ${r.second.trim().take(200)}")
                    }
                    s.close()
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

    private fun reply(output: java.io.OutputStream, line: String) {
        output.write("${line.replace('\n', ' ')}\n".toByteArray())
        output.flush()
    }

    /** Runs argv as the shell user (no `sh -c`): exit code and combined output, capped at [maxBytes]. */
    private fun exec(argv: List<String>, timeoutMs: Long = 8_000, maxBytes: Int = 16 * 1024): Pair<Int, String> {
        val proc = ProcessBuilder(argv).redirectErrorStream(true).start()
        val out = java.io.ByteArrayOutputStream()
        val reader = Thread({
            try {
                val buf = ByteArray(8192)
                while (true) {
                    val n = proc.inputStream.read(buf)
                    if (n < 0) break
                    if (out.size() < maxBytes) out.write(buf, 0, minOf(n, maxBytes - out.size()))
                }
            } catch (_: IOException) {
            }
        }, "shell-cmd").apply { isDaemon = true; start() }
        if (!proc.waitFor(timeoutMs, java.util.concurrent.TimeUnit.MILLISECONDS)) {
            proc.destroyForcibly()
            return -1 to "timed out"
        }
        reader.join(1_000)
        return proc.exitValue() to out.toString("UTF-8")
    }

    /**
     * The default SMS SIM: the platform setting Desktop's adb lane reads
     * (phone-device-manager.ts defaultSmsSubscription), else what the
     * subscription service reports as its default SMS sub.
     */
    private fun defaultSmsSubscription(): Int? {
        exec(listOf("settings", "get", "global", "multi_sim_sms_subscription")).second.trim().toIntOrNull()?.takeIf { it > 0 }?.let { return it }
        val isub = exec(listOf("dumpsys", "isub"), maxBytes = 64 * 1024).second
        return Regex("default ?sms ?sub ?id\\s*[=:]\\s*(\\d+)", RegexOption.IGNORE_CASE).find(isub)?.groupValues?.get(1)?.toIntOrNull()?.takeIf { it > 0 }
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
