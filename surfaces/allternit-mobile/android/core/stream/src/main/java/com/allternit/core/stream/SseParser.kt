package com.allternit.core.stream

import java.io.ByteArrayOutputStream

/** One dispatched server-sent-events frame (iOS `SSEFrame`). */
data class SseEvent(
    val id: String?,
    val event: String?,
    val data: String,
)

/**
 * Incremental SSE parser (WHATWG event-stream rules), a port of iOS
 * `Core/SelfHosted/SSEParser.swift`. Fed raw bytes so frame-ending blank lines are
 * never lost and a UTF-8 character split across chunks decodes whole. Comment lines
 * (`: keepalive`) and `retry`/unknown fields are ignored. [lastEventId] survives across
 * frames so a reconnect can send it as `Last-Event-ID`. Not thread-safe: one per stream.
 */
class SseParser(lastEventId: String? = null) {
    private val buffer = ByteArrayOutputStream()
    private var pendingId: String? = null
    private var pendingEvent: String? = null
    private val pendingData = mutableListOf<String>()

    var lastEventId: String? = lastEventId
        private set

    /** Feeds [bytes]; returns every frame they complete. */
    fun feed(bytes: ByteArray): List<SseEvent> {
        val frames = mutableListOf<SseEvent>()
        for (b in bytes) {
            if (b == LF) {
                var line = buffer.toByteArray()
                buffer.reset()
                if (line.isNotEmpty() && line.last() == CR) line = line.copyOf(line.size - 1)
                handle(String(line, Charsets.UTF_8))?.let(frames::add)
            } else {
                buffer.write(b.toInt())
            }
        }
        return frames
    }

    /** Convenience for text chunks (tests, already-decoded sources). */
    fun feed(text: String): List<SseEvent> = feed(text.toByteArray(Charsets.UTF_8))

    private fun handle(line: String): SseEvent? {
        if (line.isEmpty()) return dispatch()
        if (line.startsWith(":")) return null
        val colon = line.indexOf(':')
        val field: String
        var value: String
        if (colon >= 0) {
            field = line.substring(0, colon)
            value = line.substring(colon + 1)
            if (value.startsWith(" ")) value = value.substring(1)
        } else {
            field = line
            value = ""
        }
        when (field) {
            "id" -> if ('\u0000' !in value) pendingId = value
            "event" -> pendingEvent = value
            "data" -> pendingData.add(value)
            else -> Unit // retry and unknown fields are ignored
        }
        return null
    }

    private fun dispatch(): SseEvent? {
        try {
            pendingId?.let { lastEventId = it }
            if (pendingData.isEmpty()) return null
            return SseEvent(id = pendingId, event = pendingEvent, data = pendingData.joinToString("\n"))
        } finally {
            pendingId = null
            pendingEvent = null
            pendingData.clear()
        }
    }

    private companion object {
        const val LF: Byte = 0x0A
        const val CR: Byte = 0x0D
    }
}
