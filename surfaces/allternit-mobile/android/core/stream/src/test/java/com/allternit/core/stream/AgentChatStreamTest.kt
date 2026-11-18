package com.allternit.core.stream

import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Feeds a recorded agent-chat transcript through [SseParser] and [AgentChatFrames], the same
 * decode path the live okhttp-sse stream uses. Frame shapes are the ones allternit-api emits:
 * message_start / content_block_delta (cmd/allternit-api/src/gizzi_chat_stream.rs:192-222),
 * tool_result with run_code files (iOS Tests/RunCodeResultTests.swift frame), finish
 * (gizzi_chat_stream.rs cowork_turn_finish_frame), BackendChatChunk (iOS AgentChatEvent.swift).
 */
class AgentChatStreamTest {
    private val transcript = listOf(
        ": keepalive\n\n",
        "data: {\"type\":\"message_start\",\"messageId\":\"m1\",\"modelId\":\"anthropic/x\",\"runtimeModelId\":\"anthropic/x\"}\n\n",
        "data: {\"type\":\"content_block_delta\",\"messageId\":\"m1\",\"partId\":\"p0\",\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Let me check\"}}\n\n",
        // A frame split across network chunks, mid-UTF-8 character (é = C3 A9).
        "data: {\"type\":\"content_block_delta\",\"messageId\":\"m1\",\"partId\":\"p1\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Caf",
        "é\"}}\r\n\r\n",
        "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"tool_use\",\"id\":\"c1\",\"name\":\"run_code\"}}\n\n",
        "data: {\"type\":\"tool_result\",\"toolCallId\":\"c1\",\"toolName\":\"run_code\",\"files\":[],\"run\":{\"exitCode\":0}}\n\n",
        "data: {\"chunk_type\":\"tool_call\",\"chunk\":\"{\\\"toolCallId\\\":\\\"c2\\\",\\\"toolName\\\":\\\"lookup\\\"}\",\"session_id\":\"s1\"}\n\n",
        "data: {\"chunk_type\":\"tool_call\",\"chunk\":\"not json\",\"session_id\":\"s1\"}\n\n",
        "data: {\"type\":\"some.future.frame\",\"x\":1}\n\n",
        "data: {\"type\":\"question.asked\",\"properties\":{\"id\":\"q1\",\"questions\":[]}}\n\n",
        "data: {\"type\":\"question.replied\",\"properties\":{\"requestID\":\"q1\"}}\n\n",
        "data: {\"type\":\"artifact\",\"artifactId\":\"a1\",\"kind\":\"document\",\"title\":\"  Notes \"}\n\n",
        "data: {\"type\":\"finish\",\"messageId\":\"m1\",\"status\":\"error\",\"metadata\":{\"status\":\"error\",\"error\":\"boom\",\"errorDetails\":{\"name\":\"ProviderModelNotFoundError\",\"message\":\"no model\"}},\"usage\":{\"inputTokens\":3}}\n\n",
        "data: [DONE]\n\n",
    )

    private fun decodeAll(): List<ChatStreamEvent> {
        val parser = SseParser()
        val bytes = transcript.joinToString("").toByteArray(Charsets.UTF_8)
        // Feed in 7-byte slices so frames and characters straddle chunk boundaries.
        return bytes.toList().chunked(7).flatMap { parser.feed(it.toByteArray()) }.mapNotNull { AgentChatFrames.decode(it) }
    }

    @Test
    fun transcriptDecodesToTheIosEventSequence() {
        val events = decodeAll()
        assertEquals(
            listOf(
                ChatStreamEvent.MessageStart("m1", "anthropic/x", "anthropic/x"),
                ChatStreamEvent.ThinkingDelta("Let me check"),
                ChatStreamEvent.TextDelta("m1", "p1", "Café"),
                ChatStreamEvent.ToolCall("c1", "run_code"),
            ),
            events.take(4),
        )
        val result = events[4] as ChatStreamEvent.ToolResult
        assertEquals("c1", result.toolCallId)
        assertEquals("0", (result.frame["run"] as kotlinx.serialization.json.JsonObject)["exitCode"]!!.jsonPrimitive.content)
        // Stringified chunk payload → tool call; a plain-string payload and unknown frames are skipped.
        assertEquals(ChatStreamEvent.ToolCall("c2", "lookup"), events[5])
        assertEquals("q1", (events[6] as ChatStreamEvent.Question).request["id"]!!.jsonPrimitive.content)
        assertEquals(ChatStreamEvent.QuestionClosed("q1"), events[7])
        assertEquals(ChatStreamEvent.Artifact("a1", "document", "Notes", null, null), events[8])
        val finish = events[9] as ChatStreamEvent.Finish
        assertEquals("boom", finish.error)
        assertEquals("ProviderModelNotFoundError", finish.errorName)
        assertTrue(finish.isTerminal)
        assertEquals(ChatStreamEvent.Done, events[10])
        assertEquals(11, events.size)
    }

    @Test
    fun backendChunkFormatAndErrors() {
        assertEquals(ChatStreamEvent.TextDelta(null, null, "hi"), AgentChatFrames.decode("""{"chunk_type":"text","chunk":"hi","session_id":"s"}"""))
        assertEquals(ChatStreamEvent.Done, AgentChatFrames.decode("""{"chunk_type":"done","session_id":"s"}"""))
        assertEquals(ChatStreamEvent.StreamError("bad"), AgentChatFrames.decode("""{"type":"error","error":"bad"}"""))
        assertEquals(ChatStreamEvent.Done, AgentChatFrames.decode("""{"type":"message_stop"}"""))
        // An artifact without a title is dropped, like iOS.
        assertNull(AgentChatFrames.decode("""{"type":"artifact","id":"a","kind":"document","title":"  "}"""))
        assertNull(AgentChatFrames.decode("not json"))
    }
}
