package com.allternit.ondevice.harness

import android.content.res.AssetManager
import android.util.Log
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import kotlin.concurrent.thread

/**
 * DEBUG ONLY. Loopback-only (127.0.0.1, ::1) static server for the harness page in
 * assets/harness/ plus the assetlinks.json statement. From spike S1's S1HttpServer.
 */
class HarnessServer(private val assets: AssetManager, private val port: Int, private val assetLinksJson: String) {
    private val sockets = mutableListOf<ServerSocket>()
    @Volatile private var running = false

    fun start() {
        running = true
        for (host in listOf("127.0.0.1", "::1")) {
            val ss = try {
                ServerSocket().apply { reuseAddress = true; bind(InetSocketAddress(InetAddress.getByName(host), port)) }
            } catch (e: Exception) { Log.w(HarnessService.TAG, "bind $host:$port failed: $e"); continue }
            sockets += ss
            thread(name = "harness-http", isDaemon = true) {
                while (running) {
                    val s = try { ss.accept() } catch (e: Exception) { break }
                    thread(isDaemon = true) { runCatching { serve(s) }; runCatching { s.close() } }
                }
            }
        }
        check(sockets.isNotEmpty()) { "could not bind loopback port $port" }
    }

    fun stop() {
        running = false
        sockets.forEach { runCatching { it.close() } }
        sockets.clear()
    }

    private fun serve(s: Socket) {
        val reader = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.ISO_8859_1))
        val requestLine = reader.readLine() ?: return
        while (true) { val h = reader.readLine() ?: break; if (h.isEmpty()) break }
        val path = requestLine.split(' ').getOrNull(1)?.substringBefore('?') ?: "/"
        val out = s.getOutputStream()
        Log.i(HarnessService.TAG, "HTTP $path")
        when {
            path == "/.well-known/assetlinks.json" -> body(out, 200, "application/json", assetLinksJson.toByteArray())
            path == "/" || path == "/index.html" -> asset(out, "harness/index.html", "text/html; charset=utf-8")
            path == "/android-app.js" -> asset(out, "harness/android-app.js", "text/javascript; charset=utf-8")
            else -> body(out, 404, "text/plain", "not found".toByteArray())
        }
    }

    private fun asset(out: OutputStream, name: String, type: String) {
        val data = try { assets.open(name).use { it.readBytes() } } catch (e: Exception) { null }
        if (data == null) body(out, 404, "text/plain", "missing asset".toByteArray()) else body(out, 200, type, data)
    }

    private fun body(out: OutputStream, code: Int, type: String, data: ByteArray) {
        out.write(("HTTP/1.1 $code ${if (code == 200) "OK" else "Not Found"}\r\nContent-Type: $type\r\n" +
            "Content-Length: ${data.size}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").toByteArray())
        out.write(data)
        out.flush()
    }
}
