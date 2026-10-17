package com.allternit.phonelink.core

import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketException
import java.util.concurrent.ExecutorService
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

class HttpRequest(
    val method: String,
    /** Path without the query string. */
    val path: String,
    val query: String,
    /** Header names lower-cased. */
    val headers: Map<String, String>,
    val body: ByteArray?,
    val remote: InetAddress?,
    /**
     * True when the connection reached this server through the mesh bridge
     * (the dedicated internal loopback listener the mesh forwards to). Such
     * connections still have a loopback remote address, but they must NOT be
     * treated as local: the one-shot armed pairing grant is adb-only.
     */
    val viaMesh: Boolean = false,
) {
    val isLoopback: Boolean get() = remote?.isLoopbackAddress == true ||
        // ::ffff:127.x.x.x arrives as an Inet4Address on the JVM already; keep the check for odd stacks.
        (remote?.hostAddress?.startsWith("::ffff:127.") == true)

    /** Same request, marked as arriving through the mesh bridge. */
    fun markViaMesh(): HttpRequest =
        HttpRequest(method, path, query, headers, body, remote, viaMesh = true)
}

class HttpResponse(
    val status: Int,
    val contentType: String,
    val body: ByteArray,
    val headers: Map<String, String> = emptyMap(),
) {
    companion object {
        fun text(status: Int, body: String, contentType: String = "text/plain; charset=utf-8", headers: Map<String, String> = emptyMap()) =
            HttpResponse(status, contentType, body.toByteArray(Charsets.UTF_8), headers)

        fun json(status: Int, body: String, headers: Map<String, String> = emptyMap()) =
            text(status, body, "application/json", headers)
    }
}

/**
 * A deliberately tiny HTTP/1.1 server: one request per connection
 * (Connection: close, like the iCTRL runner), Content-Length bodies only,
 * bounded header and body sizes. No dependency, so nothing to licence-check
 * and nothing that pulls in more surface than /whoami, /pair and /rpc need.
 */
class HttpServer(
    private val handler: (HttpRequest) -> HttpResponse,
    private val maxThreads: Int = 8,
    private val log: (String) -> Unit = {},
    /** When true, every request served is marked [HttpRequest.viaMesh]. */
    private val viaMesh: Boolean = false,
) {
    companion object {
        const val MAX_HEADER_BYTES = 32 * 1024
        const val MAX_BODY_BYTES = 4 * 1024 * 1024
        const val READ_TIMEOUT_MS = 30_000
    }

    @Volatile private var server: ServerSocket? = null
    @Volatile private var pool: ExecutorService? = null
    private var acceptThread: Thread? = null

    val isRunning: Boolean get() = server?.isClosed == false
    val localPort: Int get() = server?.localPort ?: -1

    /** @param bind null = all interfaces, else e.g. 127.0.0.1. */
    @Synchronized
    fun start(bind: InetAddress?, port: Int) {
        stop()
        val ss = ServerSocket()
        ss.reuseAddress = true
        ss.bind(if (bind == null) InetSocketAddress(port) else InetSocketAddress(bind, port), 64)
        val exec = ThreadPoolExecutor(maxThreads, maxThreads, 30, TimeUnit.SECONDS, LinkedBlockingQueue(256)) { r ->
            Thread(r, "phonelink-http").apply { isDaemon = true }
        }
        exec.allowCoreThreadTimeOut(true)
        server = ss
        pool = exec
        acceptThread = Thread({
            while (!ss.isClosed) {
                val s = try { ss.accept() } catch (e: IOException) { break }
                try {
                    exec.execute { serve(s) }
                } catch (e: Exception) {
                    try { s.close() } catch (_: IOException) {}
                }
            }
        }, "phonelink-accept").apply { isDaemon = true; start() }
        log("listening on ${bind?.hostAddress ?: "*"}:${ss.localPort}")
    }

    /** Stops listening at once and drops in-flight connections. */
    @Synchronized
    fun stop() {
        server?.let { try { it.close() } catch (_: IOException) {} }
        server = null
        pool?.shutdownNow()
        pool = null
        acceptThread = null
    }

    private fun serve(socket: Socket) {
        socket.use { s ->
            try {
                s.soTimeout = READ_TIMEOUT_MS
                val req = readRequest(s.getInputStream(), s.inetAddress)
                val resp = when (req) {
                    is ReadResult.Ok -> try {
                        val request = if (viaMesh) req.request.markViaMesh() else req.request
                        handler(request)
                    } catch (e: Exception) {
                        log("handler error: ${e.javaClass.simpleName}: ${e.message}")
                        HttpResponse.text(500, "Internal Server Error")
                    }
                    is ReadResult.Error -> HttpResponse.text(req.status, req.message)
                }
                write(s, resp)
            } catch (e: SocketException) {
                // client went away
            } catch (e: IOException) {
                // timeout or reset
            }
        }
    }

    sealed class ReadResult {
        class Ok(val request: HttpRequest) : ReadResult()
        class Error(val status: Int, val message: String) : ReadResult()
    }

    fun readRequest(input: InputStream, remote: InetAddress?): ReadResult {
        val head = ByteArrayOutputStream()
        var matched = 0
        val terminator = byteArrayOf(13, 10, 13, 10)
        while (matched < 4) {
            val b = input.read()
            if (b < 0) return ReadResult.Error(400, "Bad Request")
            head.write(b)
            matched = if (b.toByte() == terminator[matched]) matched + 1 else if (b == 13) 1 else 0
            if (head.size() > MAX_HEADER_BYTES) return ReadResult.Error(431, "Request Header Fields Too Large")
        }
        val lines = head.toString("UTF-8").split("\r\n")
        val parts = lines.first().split(" ")
        if (parts.size < 2) return ReadResult.Error(400, "Bad Request")
        val headers = HashMap<String, String>()
        for (line in lines.drop(1)) {
            if (line.isEmpty()) break
            val idx = line.indexOf(':')
            if (idx <= 0) continue
            headers[line.substring(0, idx).trim().lowercase()] = line.substring(idx + 1).trim()
        }
        if (headers["transfer-encoding"]?.lowercase()?.contains("chunked") == true) {
            return ReadResult.Error(411, "Length Required")
        }
        val length = headers["content-length"]?.toIntOrNull() ?: 0
        if (length < 0 || length > MAX_BODY_BYTES) return ReadResult.Error(413, "Payload Too Large")
        var body: ByteArray? = null
        if (length > 0) {
            val buf = ByteArray(length)
            var off = 0
            while (off < length) {
                val n = input.read(buf, off, length - off)
                if (n < 0) return ReadResult.Error(400, "Bad Request")
                off += n
            }
            body = buf
        }
        val target = parts[1]
        val q = target.indexOf('?')
        val path = if (q >= 0) target.substring(0, q) else target
        val query = if (q >= 0) target.substring(q + 1) else ""
        return ReadResult.Ok(HttpRequest(parts[0].uppercase(), path, query, headers, body, remote))
    }

    private fun write(s: Socket, resp: HttpResponse) {
        val sb = StringBuilder()
        sb.append("HTTP/1.1 ").append(resp.status).append(' ').append(reason(resp.status)).append("\r\n")
        val all = LinkedHashMap<String, String>()
        all["Content-Type"] = resp.contentType
        all["Content-Length"] = resp.body.size.toString()
        all["Connection"] = "close"
        all.putAll(resp.headers)
        for ((k, v) in all) sb.append(k).append(": ").append(v).append("\r\n")
        sb.append("\r\n")
        val out = s.getOutputStream()
        out.write(sb.toString().toByteArray(Charsets.UTF_8))
        out.write(resp.body)
        out.flush()
    }

    private fun reason(status: Int) = when (status) {
        200 -> "OK"; 400 -> "Bad Request"; 401 -> "Unauthorized"; 403 -> "Forbidden"
        404 -> "Not Found"; 411 -> "Length Required"; 413 -> "Payload Too Large"
        429 -> "Too Many Requests"; 431 -> "Request Header Fields Too Large"
        500 -> "Internal Server Error"; 503 -> "Service Unavailable"; else -> "Unknown"
    }
}
