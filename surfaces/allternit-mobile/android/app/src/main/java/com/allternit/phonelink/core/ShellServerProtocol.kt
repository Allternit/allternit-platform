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
 *   KEY <keycode>\n                  → `input keyevent`, OK\n
 *   SMS <subId> <to> <body-b64>\n    → `service call isms` send (subId -1 = default SIM), OK\n
 *   DIAL <number> [<subId>]\n         → `am start -a android.intent.action.CALL`, OK\n
 *   SMSREAD <since_ms> [<from>]\n     → `content query` of the inbox, OK <stdout-b64>\n
 *   A11YON\n                         → turns the app's accessibility service back on, OK\n
 *
 * Errors answer `ERR <message>\n` and close. The one-shot commands run argv
 * directly (no `sh -c`), and every argument is checked here first, so nothing
 * a caller sends is ever read as shell syntax. Desktop owns the approval gate,
 * rate limits and emergency-number block for texts and calls; DIAL refuses
 * emergency numbers here too. Raw mode carries the scrcpy wire
 * protocol untouched, so Desktop's phone-scrcpy parser reads it as it reads
 * an adb-forwarded socket.
 */
object ShellServerProtocol {
    const val VERSION = 1
    const val MAGIC = "ALLTERNIT-SHELL"
    const val DEFAULT_PORT = 18210
    /** Command lines: an SMS body (1000 chars, base64 of UTF-8) fits. */
    const val MAX_LINE = 8192
    /** Reply lines: SMSREAD's base64 stdout. */
    const val MAX_REPLY = 512 * 1024
    /** The app's accessibility service, as `enabled_accessibility_services` lists it. */
    const val ACCESSIBILITY_COMPONENT = "com.allternit.phonelink/.PhoneLinkAccessibilityService"
    /** scrcpy-server's own pinned version; the jar Desktop pushes must match it. */
    const val SCRCPY_VERSION = "5.0.1"
    const val SCRCPY_JAR = "/data/local/tmp/allternit-scrcpy-server.jar"

    const val MAIN_CLASS = "com.allternit.phonelink.shell.ShellServerMain"
    const val LOG_FILE = "/data/local/tmp/allternit-shell.log"
    /**
     * Stops a server left over from an older token (it would hold the port).
     * The bracket keeps pkill -f from matching its own `sh -c` line; run it as
     * its own shell call, before [LaunchLines.start].
     */
    const val KILL_LINE = "pkill -f 'phonelink[.]shell[.][S]hellServerMain'"

    /**
     * The two shell lines that (re)start the server as the shell uid. The app
     * runs them over its own adb connection, and Allternit Desktop runs the
     * same lines over its adb lane (`screen_service` RPC), so both paths start
     * an identical server. Desktop checks them against this exact shape.
     */
    data class LaunchLines(val kill: String, val start: String)

    private val APK = Regex("^/data/app/[A-Za-z0-9._~=+-]+(/[A-Za-z0-9._~=+-]+)*/base\\.apk$")

    fun launchLines(apk: String, token: String, port: Int): LaunchLines {
        require(APK.matches(apk) && !apk.contains("..")) { "unexpected apk path" }
        require(isValidToken(token)) { "bad token" }
        require(port in 1024..65535) { "bad port" }
        return LaunchLines(
            kill = KILL_LINE,
            start = "setsid app_process -Djava.class.path=$apk / $MAIN_CLASS $token $port </dev/null >$LOG_FILE 2>&1 &",
        )
    }

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
        data class Key(val code: String) : Command()
        data class Sms(val subId: Int, val to: String, val body: String) : Command()
        data class Dial(val number: String, val subId: Int?) : Command()
        data class SmsRead(val sinceMs: Long, val from: String?) : Command()
        object A11yOn : Command()
    }

    private val KEYCODE = Regex("^([0-9]{1,3}|KEYCODE_[A-Z0-9_]{1,40})$")
    private val NUMBER = Regex("^\\+?[0-9]{3,15}$")
    private val FROM = Regex("^\\+?[0-9]{3,16}$")
    private val EMERGENCY = setOf("911", "112", "999", "110", "000", "988", "933")

    fun isEmergency(number: String): Boolean {
        val n = number.removePrefix("+")
        return n in EMERGENCY || (n.startsWith("1") && n.drop(1) in EMERGENCY)
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
            "A11YON" -> if (parts.size == 1) Result.success(Command.A11yOn) else bad("A11YON takes no arguments")
            "KEY" -> if (parts.size == 2 && KEYCODE.matches(parts[1])) Result.success(Command.Key(parts[1])) else bad("KEY <keycode>")
            "SMS" -> {
                val sub = parts.getOrNull(1)?.toIntOrNull()
                if (parts.size != 4 || sub == null || sub < -1 || sub > 1000 || !NUMBER.matches(parts[2])) return bad("SMS <subId> <to> <body-b64>")
                if (isEmergency(parts[2])) return bad("emergency numbers are not allowed")
                val body = try { String(java.util.Base64.getDecoder().decode(parts[3]), Charsets.UTF_8) } catch (e: IllegalArgumentException) { return bad("body is not base64") }
                if (body.isEmpty() || body.length > 1000) return bad("body must be 1–1000 characters")
                Result.success(Command.Sms(sub, parts[2], body))
            }
            "DIAL" -> {
                if (parts.size !in 2..3 || !NUMBER.matches(parts[1])) return bad("DIAL <number> [<subId>]")
                if (isEmergency(parts[1])) return bad("emergency numbers are not allowed")
                val sub = parts.getOrNull(2)?.let { it.toIntOrNull()?.takeIf { s -> s in 0..1000 } ?: return bad("bad subId") }
                Result.success(Command.Dial(parts[1], sub))
            }
            "SMSREAD" -> {
                val since = parts.getOrNull(1)?.toLongOrNull()
                if (parts.size !in 2..3 || since == null || since < 0) return bad("SMSREAD <since_ms> [<from>]")
                val from = parts.getOrNull(2)
                if (from != null && !FROM.matches(from)) return bad("bad from number")
                Result.success(Command.SmsRead(since, from))
            }
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

    /** `service call isms` sendTextForSubscriber, the same layout as Desktop's adb lane (phone-tools.ts buildIsmsSend). */
    fun smsArgv(subId: Int, to: String, body: String): List<String> = listOf(
        "service", "call", "isms", "5", "i32", subId.toString(), "s16", "com.android.mms.service", "s16", "null",
        "s16", to, "s16", "null", "s16", body, "s16", "null", "s16", "null", "i32", "0", "i64", "0",
    )

    /** `service call` prints a Parcel; a leading zero word without an exception is the success shape. */
    fun smsAccepted(stdout: String): Boolean =
        Regex("Result: Parcel\\(\\s*00000000").containsMatchIn(stdout) && !stdout.contains("Exception", ignoreCase = true)

    fun dialArgv(number: String, subId: Int?): List<String> =
        listOf("am", "start", "-a", "android.intent.action.CALL", "-d", "tel:" + number.replace("+", "%2B")) +
            (subId?.let { listOf("--ei", "android.telecom.extra.PHONE_ACCOUNT_SUBSCRIPTION_ID", it.toString()) } ?: emptyList())

    /** The inbox rows since [sinceMs] (newest first), optionally from one number; validated values only. */
    fun smsReadArgv(sinceMs: Long, from: String?): List<String> {
        val where = "date>$sinceMs" + (from?.let { " AND address LIKE '%${it.removePrefix("+")}%'" } ?: "")
        return listOf("content", "query", "--uri", "content://sms/inbox", "--projection", "address:body:date", "--where", where, "--sort", "date DESC")
    }

    /** `enabled_accessibility_services` with the app's service added (colon list; "null" when unset). */
    fun withAccessibility(current: String): String {
        val list = current.trim().takeUnless { it.isEmpty() || it == "null" }?.split(':')?.filter { it.isNotEmpty() } ?: emptyList()
        return (if (ACCESSIBILITY_COMPONENT in list) list else list + ACCESSIBILITY_COMPONENT).joinToString(":")
    }

    /** scrcpy names its abstract socket after the session id. */
    fun socketName(scid: String) = "scrcpy_$scid"

    /** Reads one \n-terminated line without buffering past it (the rest of the stream may be raw). */
    fun readLine(input: InputStream, max: Int = MAX_LINE): String? {
        val out = ByteArrayOutputStream()
        while (true) {
            val b = input.read()
            if (b < 0) return if (out.size() == 0) null else out.toString("UTF-8")
            if (b == '\n'.code) return out.toString("UTF-8")
            out.write(b)
            if (out.size() > max) throw IOException("line too long")
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
