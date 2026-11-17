package com.allternit.phonelink

import android.util.Log
import com.allternit.phonelink.core.Cancellable
import com.allternit.phonelink.core.HttpReply
import com.allternit.phonelink.core.JsonHttp
import com.allternit.phonelink.core.LinkRuntime
import com.allternit.phonelink.core.SocketConnector
import com.allternit.phonelink.core.SocketLink
import com.allternit.phonelink.core.SocketListener
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import java.util.concurrent.Executors
import java.util.concurrent.ThreadFactory
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The platform pieces AppPhoneLink needs, on OkHttp 4.12: blocking JSON POST,
 * WebSockets with a ping every 20 s (the relay drops a silent socket), and the
 * worker / one ordered serial thread / timer the link runs on.
 */
object OkHttpLink {
    private const val TAG = "AppLink"
    /** Above this many queued bytes a video frame is dropped instead of queued (OkHttp fails the socket at 16 MiB). */
    const val MAX_QUEUED_BYTES = 1L shl 20

    val client: OkHttpClient by lazy {
        OkHttpClient.Builder()
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(30, TimeUnit.SECONDS)
            .writeTimeout(30, TimeUnit.SECONDS)
            .pingInterval(20, TimeUnit.SECONDS)
            .build()
    }

    private val JSON = "application/json; charset=utf-8".toMediaType()

    val http = object : JsonHttp {
        override fun postJson(url: String, headers: Map<String, String>, body: String): HttpReply {
            val req = Request.Builder().url(url).post(body.toRequestBody(JSON))
            headers.forEach { (k, v) -> if (!k.equals("Content-Type", ignoreCase = true)) req.header(k, v) }
            client.newCall(req.build()).execute().use { r -> return HttpReply(r.code, r.body?.string().orEmpty()) }
        }
    }

    val sockets = object : SocketConnector {
        override fun connect(url: String, listener: SocketListener): SocketLink {
            val closed = AtomicBoolean(false)
            fun closedOnce() { if (closed.compareAndSet(false, true)) listener.onClosed() }
            val ws = client.newWebSocket(Request.Builder().url(url).build(), object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) = listener.onOpen()
                override fun onMessage(webSocket: WebSocket, text: String) = listener.onText(text)
                override fun onMessage(webSocket: WebSocket, bytes: ByteString) = listener.onBinary(bytes.toByteArray())
                override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                    webSocket.close(1000, null)
                    closedOnce()
                }
                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) = closedOnce()
                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    Log.i(TAG, "socket failed: ${t.javaClass.simpleName}")
                    closedOnce()
                }
            })
            return object : SocketLink {
                override fun sendText(text: String) = ws.send(text)
                override fun sendBinary(bytes: ByteArray): Boolean {
                    if (ws.queueSize() > MAX_QUEUED_BYTES) return false
                    return ws.send(bytes.toByteString())
                }
                override fun close(code: Int, reason: String) { ws.close(code, reason) }
            }
        }
    }

    private fun named(name: String) = ThreadFactory { r -> Thread(r, name).apply { isDaemon = true } }

    val runtime = object : LinkRuntime {
        private val bg = Executors.newCachedThreadPool(named("applink-bg"))
        private val serialExec = Executors.newSingleThreadExecutor(named("applink-input"))
        private val timer = Executors.newSingleThreadScheduledExecutor(named("applink-timer"))

        override fun nowMs() = System.currentTimeMillis()
        override fun background(task: () -> Unit) { bg.execute { guard(task) } }
        override fun serial(task: () -> Unit) { serialExec.execute { guard(task) } }
        override fun schedule(delayMs: Long, task: () -> Unit): Cancellable {
            val f = timer.schedule({ background(task) }, delayMs, TimeUnit.MILLISECONDS)
            return object : Cancellable { override fun cancel() { f.cancel(false) } }
        }
        private fun guard(task: () -> Unit) = try { task() } catch (e: Exception) { Log.w(TAG, "link task failed: ${e.javaClass.simpleName}: ${e.message}") }
    }
}
