package com.allternit.phonelink.core

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

/**
 * The line protocol between Phone Link (app uid) and its shell server (shell
 * uid, started over the phone's own wireless debugging, see ShellServerMain).
 * The server listens on 127.0.0.1 only. Every connection starts with
 *
 *   ALLTERNIT-SHELL 1 <token>\n      token: 64 hex chars, set by the app at launch
 *
 * followed by one command line:
 *
 *   PING\n                           → OK <version>\n, then close
 *   QUIT\n                           → OK\n, stops every scrcpy session and exits
 *   SCRCPY <scid> <arg> [<arg>…]\n   → starts scrcpy-server for scid, connects its
 *                                       socket, OK\n, then raw bytes both ways (video)
 *   ATTACH <scid>\n                  → second socket of that session, OK\n, raw (control)
 *
 * Errors answer `ERR <message>\n` and close. Raw mode carries the scrcpy wire
 * protocol untouched, so Desktop's phone-scrcpy parser reads it as it reads
 * an adb-forwarded socket.
 */
object ShellServerProtocol {
    const val VERSION = 1
    const val MAGIC = "ALLTERNIT-SHELL"
    const val DEFAULT_PORT = 18210
    const val MAX_LINE = 4096
    /** scrcpy-server's own pinned version; the jar Desktop pushes must match it. */
    const val SCRCPY_VERSION = "5.0.1"
    const val SCRCPY_JAR = "/data/local/tmp/allternit-scrcpy-server.jar"

    private val TOKEN = Regex("^[0-9a-f]{64}$")
    private val SCID = Regex("^[0-9a-f]{8}$")
    /** scrcpy server options are key=value with a small alphabet; nothing a shell could read as syntax. */
    private val ARG = Regex("^[a-z_]{1,40}=[A-Za-z0-9_.:,+-]{0,200}$")
    /** Options the caller may not set: the server owns the session id and the tunnel mode. */
    private val RESERVED = setOf("scid", "tunnel_forward", "log_level")

    sealed class Command {
        object Ping : Command()
        object Quit : Command()
        data class Scrcpy(val scid: String, val args: List<String>) : Command()
        data class Attach(val scid: String) : Command()
    }

    fun hello(token: String) = "$MAGIC $VERSION $token\n"

    fun isValidToken(token: String) = TOKEN.matches(token)

    /** Checks the hello line against the expected token in constant time. */
    fun checkHello(line: String, expectedToken: String): Boolean {
        val parts = line.split(' ')
        if (parts.size != 3 || parts[0] != MAGIC || parts[1] != VERSION.toString()) return false
        return MessageDigest.isEqual(parts[2].toByteArray(), expectedToken.toByteArray())
    }

    fun parseCommand(line: String): Result<Command> {
        val parts = line.split(' ').filter { it.isNotEmpty() }
        return when (parts.firstOrNull()) {
            "PING" -> if (parts.size == 1) Result.success(Command.Ping) else bad("PING takes no arguments")
            "QUIT" -> if (parts.size == 1) Result.success(Command.Quit) else bad("QUIT takes no arguments")
            "ATTACH" -> if (parts.size == 2 && SCID.matches(parts[1])) Result.success(Command.Attach(parts[1])) else bad("ATTACH <scid>")
            "SCRCPY" -> {
                if (parts.size < 2 || !SCID.matches(parts[1])) return bad("SCRCPY <scid> <arg>…")
                val args = parts.drop(2)
                for (a in args) {
                    if (!ARG.matches(a)) return bad("bad scrcpy option")
                    if (a.substringBefore('=') in RESERVED) return bad("reserved scrcpy option ${a.substringBefore('=')}")
                }
                Result.success(Command.Scrcpy(parts[1], args))
            }
            else -> bad("unknown command")
        }
    }

    /** The scrcpy-server argv (after `app_process / com.genymobile.scrcpy.Server`). */
    fun scrcpyServerArgs(cmd: Command.Scrcpy): List<String> =
        listOf(SCRCPY_VERSION, "scid=${cmd.scid}", "tunnel_forward=true", "log_level=warn") + cmd.args

    /** scrcpy names its abstract socket after the session id. */
    fun socketName(scid: String) = "scrcpy_$scid"

    /** Reads one \n-terminated line without buffering past it (the rest of the stream may be raw). */
    fun readLine(input: InputStream): String? {
        val out = ByteArrayOutputStream()
        while (true) {
            val b = input.read()
            if (b < 0) return if (out.size() == 0) null else out.toString("UTF-8")
            if (b == '\n'.code) return out.toString("UTF-8")
            out.write(b)
            if (out.size() > MAX_LINE) throw IOException("line too long")
        }
    }

    private fun bad(message: String): Result<Command> = Result.failure(IllegalArgumentException(message))
}

/** Copies bytes both ways between two endpoints until either side closes, then closes both. */
object Pipe {
    fun copy(input: InputStream, output: OutputStream, onDone: () -> Unit): Thread = Thread({
        val buf = ByteArray(64 * 1024)
        try {
            while (true) {
                val n = input.read(buf)
                if (n < 0) break
                output.write(buf, 0, n)
                output.flush()
            }
        } catch (_: IOException) {
        } finally {
            onDone()
        }
    }, "phonelink-pipe").apply { isDaemon = true; start() }

    /** Blocks until one direction ends; [close] must close both endpoints so the other direction ends too. */
    fun join(aIn: InputStream, aOut: OutputStream, bIn: InputStream, bOut: OutputStream, close: () -> Unit) {
        val once = java.util.concurrent.atomic.AtomicBoolean(false)
        val done = { if (once.compareAndSet(false, true)) close() }
        val t1 = copy(aIn, bOut, done)
        val t2 = copy(bIn, aOut, done)
        t1.join()
        t2.join()
    }
}
