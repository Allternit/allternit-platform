package com.allternit.app.spike.s1

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
 * SPIKE S1: a tiny loopback-only HTTP/1.1 server (127.0.0.1 and ::1, never
 * other interfaces). Debug-only; it has no auth, so any app on the phone could
 * call it. A production localhost bridge would need a per-launch token.
 *
 *   GET /s1.html, /s1-client.js   the offline test page (APK assets/s1/)
 *   GET /.well-known/assetlinks.json  handle_all_urls + use_as_origin for this
 *                                 APK's package and signing cert. Chrome fetches
 *                                 it from the origin itself to verify the TWA and
 *                                 the postMessage source origin.
 *   GET /c2/ping                  candidate 2 round trip
 *   GET /c2/stream?n=500&bytes=16 candidate 2 stream, NDJSON, close-delimited
 *
 * Every response sends Access-Control-Allow-Origin: * so the same endpoints can
 * be probed from https://m.allternit.com (Local Network Access check).
 */
class S1HttpServer(
    private val assets: AssetManager,
    private val port: Int,
    /** Digital Asset Links statement for this APK (see [S1Service.assetLinks]). */
    private val assetLinksJson: String,
) {
    private val sockets = mutableListOf<ServerSocket>()

    /**
     * When true, assetlinks.json requests get the connection closed with no
     * response: the same net error Chrome sees for m.allternit.com when the
     * phone is offline, so Chrome falls back to its saved verification result.
     */
    @Volatile var dropAssetLinks = false
    @Volatile private var running = false

    fun start() {
        running = true
        for (host in listOf("127.0.0.1", "::1")) {
            val ss = try {
                ServerSocket().apply {
                    reuseAddress = true
                    bind(InetSocketAddress(InetAddress.getByName(host), port))
                }
            } catch (e: Exception) {
                Log.w(S1Bridge.TAG, "bind $host:$port failed: $e"); continue
            }
            sockets += ss
            thread(name = "s1-http-$host", isDaemon = true) { acceptLoop(ss) }
        }
        check(sockets.isNotEmpty()) { "could not bind loopback port $port" }
    }

    fun boundAddresses(): String = sockets.joinToString { it.localSocketAddress.toString() }

    fun stop() {
        running = false
        sockets.forEach { runCatching { it.close() } }
        sockets.clear()
    }

    private fun acceptLoop(ss: ServerSocket) {
        while (running) {
            val s = try { ss.accept() } catch (e: Exception) { break }
            thread(name = "s1-http-conn", isDaemon = true) { runCatching { serve(s) } ; runCatching { s.close() } }
        }
    }

    private fun serve(s: Socket) {
        s.tcpNoDelay = true
        val reader = BufferedReader(InputStreamReader(s.getInputStream(), Charsets.ISO_8859_1))
        val requestLine = reader.readLine() ?: return
        var origin: String? = null
        while (true) {
            val h = reader.readLine() ?: break
            if (h.isEmpty()) break
            if (h.startsWith("origin:", ignoreCase = true)) origin = h.substringAfter(':').trim()
        }
        val parts = requestLine.split(' ')
        val method = parts.getOrNull(0) ?: ""
        val target = parts.getOrNull(1) ?: "/"
        val path = target.substringBefore('?')
        val query = parseQuery(target.substringAfter('?', ""))
        val out = s.getOutputStream()
        Log.i(S1Bridge.TAG, "HTTP $method $target origin=$origin")
        if (method == "OPTIONS") {
            head(out, 204, "text/plain", 0, extra = "Access-Control-Allow-Methods: GET\r\nAccess-Control-Allow-Headers: *\r\n")
            return
        }
        when {
            path == "/.well-known/assetlinks.json" && dropAssetLinks -> {
                Log.i(S1Bridge.TAG, "assetlinks request dropped (simulated offline DAL)")
                s.setSoLinger(true, 0) // RST, no response
            }
            path == "/.well-known/assetlinks.json" ->
                body(out, 200, "application/json", assetLinksJson.toByteArray())
            path == "/c2/ping" -> body(out, 200, "text/plain", "pong".toByteArray())
            path == "/c2/stream" -> stream(out, query["n"]?.toIntOrNull() ?: 500, query["bytes"]?.toIntOrNull() ?: 16)
            path == "/" || path == "/s1.html" -> asset(out, "s1/s1.html", "text/html; charset=utf-8")
            path == "/s1-client.js" -> asset(out, "s1/s1-client.js", "text/javascript; charset=utf-8")
            else -> body(out, 404, "text/plain", "not found".toByteArray())
        }
    }

    private fun stream(out: OutputStream, n: Int, bytes: Int) {
        val pad = "x".repeat(bytes.coerceIn(0, 65536))
        head(out, 200, "application/x-ndjson", -1)
        val t0 = System.nanoTime()
        for (i in 0 until n) {
            out.write("""{"t":"tok","i":$i,"p":"$pad"}""".toByteArray())
            out.write('\n'.code)
            out.flush() // one write per token, like a real engine emitting tokens
        }
        Log.i(S1Bridge.TAG, "C2 STREAM n=$n bytes=$bytes nativeWriteMs=%.1f".format((System.nanoTime() - t0) / 1e6))
    }

    private fun asset(out: OutputStream, name: String, type: String) {
        val data = try { assets.open(name).use { it.readBytes() } } catch (e: Exception) { null }
        if (data == null) body(out, 404, "text/plain", "missing asset".toByteArray()) else body(out, 200, type, data)
    }

    private fun body(out: OutputStream, code: Int, type: String, data: ByteArray) {
        head(out, code, type, data.size)
        out.write(data)
        out.flush()
    }

    private fun head(out: OutputStream, code: Int, type: String, length: Int, extra: String = "") {
        val reason = when (code) { 200 -> "OK"; 204 -> "No Content"; 404 -> "Not Found"; else -> "Status" }
        val len = if (length >= 0) "Content-Length: $length\r\n" else ""
        out.write(
            ("HTTP/1.1 $code $reason\r\nContent-Type: $type\r\n$len" +
                "Cache-Control: no-store\r\nConnection: close\r\n" +
                "Access-Control-Allow-Origin: *\r\n$extra\r\n").toByteArray(),
        )
    }

    private fun parseQuery(q: String): Map<String, String> =
        q.split('&').filter { '=' in it }.associate { it.substringBefore('=') to it.substringAfter('=') }
}
