package com.allternit.phonelink.core

import com.allternit.phonelink.core.ShellServerProtocol.Command
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.net.InetAddress
import java.net.ServerSocket
import kotlin.concurrent.thread

class ShellServerProtocolTest {
    private val token = "ab".repeat(32)

    @Test fun helloNeedsTheExactTokenAndVersion() {
        assertTrue(ShellServerProtocol.checkHello(ShellServerProtocol.hello(token).trimEnd(), token))
        assertFalse(ShellServerProtocol.checkHello("ALLTERNIT-SHELL 1 ${"cd".repeat(32)}", token))
        assertFalse(ShellServerProtocol.checkHello("ALLTERNIT-SHELL 2 $token", token))
        assertFalse(ShellServerProtocol.checkHello("PING", token))
        assertFalse(ShellServerProtocol.isValidToken("xyz"))
    }

    @Test fun parsesTheCommands() {
        assertEquals(Command.Ping, ShellServerProtocol.parseCommand("PING").getOrThrow())
        assertEquals(Command.Quit, ShellServerProtocol.parseCommand("QUIT").getOrThrow())
        assertEquals(Command.Attach("0000abcd"), ShellServerProtocol.parseCommand("ATTACH 0000abcd").getOrThrow())
        val s = ShellServerProtocol.parseCommand("SCRCPY 0000abcd audio=false video_codec=h264 max_size=1080").getOrThrow() as Command.Scrcpy
        assertEquals(listOf("audio=false", "video_codec=h264", "max_size=1080"), s.args)
        assertEquals(
            listOf("5.0.1", "scid=0000abcd", "tunnel_forward=true", "log_level=warn", "audio=false", "video_codec=h264", "max_size=1080"),
            ShellServerProtocol.scrcpyServerArgs(s),
        )
        assertEquals("scrcpy_0000abcd", ShellServerProtocol.socketName("0000abcd"))
    }

    @Test fun refusesAnythingAShellCouldReadAsSyntaxOrThatTheServerOwns() {
        for (bad in listOf(
            "SCRCPY 0000abcd a=b;reboot", "SCRCPY 0000abcd a=\$(id)", "SCRCPY 0000abcd a=`id`", "SCRCPY 0000abcd a=b|c",
            "SCRCPY 0000abcd noequals", "SCRCPY 0000abcd scid=00000001", "SCRCPY 0000abcd tunnel_forward=false",
            "SCRCPY ../../x", "ATTACH 1", "PING now", "REBOOT",
        )) assertTrue(bad, ShellServerProtocol.parseCommand(bad).isFailure)
    }

    @Test fun parsesTheOneShotCommandsAndRefusesBadArguments() {
        val b64 = java.util.Base64.getEncoder().encodeToString("Hi, it's me".toByteArray())
        assertEquals(Command.Key("KEYCODE_ENTER"), ShellServerProtocol.parseCommand("KEY KEYCODE_ENTER").getOrThrow())
        assertEquals(Command.Sms(-1, "+15551234567", "Hi, it's me"), ShellServerProtocol.parseCommand("SMS -1 +15551234567 $b64").getOrThrow())
        assertEquals(Command.Dial("5551234567", 2), ShellServerProtocol.parseCommand("DIAL 5551234567 2").getOrThrow())
        assertEquals(Command.SmsRead(1700000000000, null), ShellServerProtocol.parseCommand("SMSREAD 1700000000000").getOrThrow())
        assertEquals(Command.A11yOn, ShellServerProtocol.parseCommand("A11YON").getOrThrow())
        for (bad in listOf(
            "KEY 1;reboot", "KEY keycode_enter", "DIAL 911", "DIAL +1911", "SMS -1 112 $b64", "SMS 1 555;id $b64",
            "SMS 1 5551234567 not*base64", "SMSREAD -5", "SMSREAD 1 55'5", "A11YON now",
        )) assertTrue(bad, ShellServerProtocol.parseCommand(bad).isFailure)
        assertEquals(
            "a/.B:${ShellServerProtocol.ACCESSIBILITY_COMPONENT}",
            ShellServerProtocol.withAccessibility("a/.B"),
        )
        assertEquals(ShellServerProtocol.ACCESSIBILITY_COMPONENT, ShellServerProtocol.withAccessibility("null"))
    }

    @Test fun readLineStopsAtTheNewlineAndLeavesRawBytes() {
        val input = ByteArrayInputStream("OK\n\u0001\u0002".toByteArray())
        assertEquals("OK", ShellServerProtocol.readLine(input))
        assertEquals(1, input.read())
    }

    /** ShellClient against a stand-in server that answers like ShellServerMain. */
    @Test fun clientPingsAndOpensRawStreams() {
        val ss = ServerSocket(0, 4, InetAddress.getByName("127.0.0.1"))
        val t = thread(isDaemon = true) {
            repeat(3) {
                ss.accept().use { s ->
                    val i = s.getInputStream()
                    val o = s.getOutputStream()
                    if (!ShellServerProtocol.checkHello(ShellServerProtocol.readLine(i)!!, token)) return@use
                    when (val cmd = ShellServerProtocol.parseCommand(ShellServerProtocol.readLine(i)!!).getOrNull()) {
                        Command.Ping -> o.write("OK 1\n".toByteArray())
                        is Command.Attach -> o.write("ERR no such session\n".toByteArray())
                        is Command.Scrcpy -> { o.write("OK\n".toByteArray()); o.write(byteArrayOf(0)); o.write(i.read()) }
                        else -> Unit
                    }
                }
            }
        }
        val client = ShellClient(port = { ss.localPort }, token = { token })
        assertTrue(client.ping())
        try {
            client.openRaw("ATTACH 0000abcd")
            throw AssertionError("expected ERR")
        } catch (e: java.io.IOException) {
            assertEquals("no such session", e.message)
        }
        client.openRaw("SCRCPY 0000abcd audio=false").use { raw ->
            assertEquals(0, raw.getInputStream().read()) // scrcpy's dummy byte, raw
            raw.getOutputStream().write(42)
            assertEquals(42, raw.getInputStream().read())
        }
        t.join(5_000)
        ss.close()
        assertFalse(ShellClient(port = { ss.localPort }, token = { token }).ping())
    }

    @Test fun launchLinesAreTheExactShellLines() {
        val apk = "/data/app/~~Ab12-_==/com.allternit.phonelink-Xy_9==/base.apk"
        val l = ShellServerProtocol.launchLines(apk, token, ShellServerProtocol.DEFAULT_PORT)
        assertEquals("pkill -f 'phonelink[.]shell[.][S]hellServerMain'", l.kill)
        assertEquals(
            "setsid app_process -Djava.class.path=$apk / com.allternit.phonelink.shell.ShellServerMain $token 18210 " +
                "</dev/null >/data/local/tmp/allternit-shell.log 2>&1 &",
            l.start,
        )
    }

    @Test fun launchLinesRefuseAnythingOddInTheirInputs() {
        val apk = "/data/app/com.allternit.phonelink-1/base.apk"
        for (bad in listOf("/data/app/x;reboot/base.apk", "/data/app/x y/base.apk", "/sdcard/base.apk", "/data/app/../../x/base.apk", "/data/app/x/base.apk.bak")) {
            assertTrue(bad, runCatching { ShellServerProtocol.launchLines(bad, token, 18210) }.isFailure)
        }
        assertTrue(runCatching { ShellServerProtocol.launchLines(apk, "xyz", 18210) }.isFailure)
        assertTrue(runCatching { ShellServerProtocol.launchLines(apk, token, 80) }.isFailure)
        assertTrue(runCatching { ShellServerProtocol.launchLines(apk, token, 18210) }.isSuccess)
    }
}
