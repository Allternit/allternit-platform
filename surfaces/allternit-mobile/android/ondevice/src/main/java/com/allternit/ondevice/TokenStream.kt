package com.allternit.ondevice

import org.json.JSONObject

/**
 * One reply being generated, as numbered frames for the page.
 *
 * Tokens are batched into `tok` frames (flushed every [flushMs] or [flushTokens]
 * tokens) to cut Binder calls into Chrome. Every frame is kept until the next
 * reply starts, so the page can ask "resume from seq N" after it was in the
 * background: Chrome refuses postMessage while the TWA is not in front
 * (CustomTabsConnection RESULT_FAILURE_DISALLOWED), so a failed post pauses live
 * delivery and generation carries on into the buffer.
 *
 * Not thread-safe by itself: the engine worker appends, the bridge worker
 * resumes; both go through the synchronized methods.
 */
class TokenStream(
    val id: String,
    private val send: (String) -> Boolean,
    private val clock: () -> Long = System::currentTimeMillis,
    private val flushMs: Long = 30,
    private val flushTokens: Int = 16,
) {
    private val frames = ArrayList<String>()
    private val pending = StringBuilder()
    private var pendingTokens = 0
    private var lastFlush = clock()
    private var live = true
    var done = false
        private set

    @Synchronized
    fun append(text: String) {
        if (done) return
        pending.append(text)
        pendingTokens++
        if (pendingTokens >= flushTokens || clock() - lastFlush >= flushMs) flush()
    }

    @Synchronized
    fun finish(finish: String, stats: JSONObject) {
        if (done) return
        flush()
        val frame = JSONObject()
            .put("v", Protocol.VERSION).put("t", "end").put("id", id)
            .put("seq", frames.size).put("finish", finish).put("stats", stats)
            .toString()
        frames.add(frame)
        done = true
        deliver(frame)
    }

    /** Replays frames from [fromSeq] and goes live again. Returns how many were sent. */
    @Synchronized
    fun resume(fromSeq: Int): Int {
        live = true
        var sent = 0
        for (i in fromSeq.coerceAtLeast(0) until frames.size) {
            if (!send(frames[i])) { live = false; break }
            sent++
        }
        return sent
    }

    @Synchronized
    fun frameCount(): Int = frames.size

    private fun flush() {
        lastFlush = clock()
        if (pending.isEmpty()) return
        val frame = JSONObject()
            .put("v", Protocol.VERSION).put("t", "tok").put("id", id)
            .put("seq", frames.size).put("text", pending.toString())
            .toString()
        pending.setLength(0)
        pendingTokens = 0
        frames.add(frame)
        deliver(frame)
    }

    private fun deliver(frame: String) {
        if (!live) return
        if (!send(frame)) live = false
    }
}
