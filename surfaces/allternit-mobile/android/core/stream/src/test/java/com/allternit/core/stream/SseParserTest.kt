package com.allternit.core.stream

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Vectors from iOS `Tests/SelfHostedClientTests.swift` (SSEParserTests), plus event
 * names, a UTF-8 character split across chunks, and ignored fields.
 */
class SseParserTest {

    @Test
    fun parsesFramesAcrossChunks() {
        val p = SseParser()
        val frames = p.feed("id: 1\nevent: message\ndata: {\"seq\"").toMutableList()
        assertTrue(frames.isEmpty())
        frames += p.feed(":1}\n\n: keepalive\n\nid: 2\nevent: status\ndata: x\n\n")
        assertEquals(
            listOf(
                SseEvent(id = "1", event = "message", data = "{\"seq\":1}"),
                SseEvent(id = "2", event = "status", data = "x"),
            ),
            frames,
        )
        assertEquals("2", p.lastEventId)
    }

    @Test
    fun crlfMultilineDataAndComments() {
        val p = SseParser()
        val frames = p.feed(": hi\r\nid:7\r\ndata: a\r\ndata: b\r\n\r\n")
        assertEquals(listOf(SseEvent(id = "7", event = null, data = "a\nb")), frames)
    }

    @Test
    fun blankLineWithoutDataDispatchesNothing() {
        val p = SseParser(lastEventId = "5")
        assertTrue(p.feed("\n\n: keepalive\n\n").isEmpty())
        assertEquals("5", p.lastEventId)
    }

    @Test
    fun eventNameResetsBetweenFramesAndOnlyOneLeadingSpaceIsStripped() {
        val p = SseParser()
        val frames = p.feed("event: delta\ndata:  two spaces\n\ndata: plain\n\n")
        assertEquals(
            listOf(
                SseEvent(id = null, event = "delta", data = " two spaces"),
                SseEvent(id = null, event = null, data = "plain"),
            ),
            frames,
        )
    }

    @Test
    fun utf8CharacterSplitAcrossChunksDecodesWhole() {
        val p = SseParser()
        val bytes = "data: héllo ✓\n\n".toByteArray(Charsets.UTF_8)
        val split = bytes.indexOfFirst { it == 0xC3.toByte() } + 1 // inside "é"
        val frames = p.feed(bytes.copyOfRange(0, split)) + p.feed(bytes.copyOfRange(split, bytes.size))
        assertEquals(listOf(SseEvent(id = null, event = null, data = "héllo ✓")), frames)
    }

    @Test
    fun retryUnknownFieldsAndNulIdsAreIgnored() {
        val p = SseParser(lastEventId = "9")
        val frames = p.feed("retry: 3000\nfoo: bar\nid: a\u0000b\ndata\n\n")
        // A bare "data" line is a data field with an empty value.
        assertEquals(listOf(SseEvent(id = null, event = null, data = "")), frames)
        assertEquals("9", p.lastEventId)
    }
}
