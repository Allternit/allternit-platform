package com.allternit.core.stream

import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The `tool_progress` frame (platform #2812) and the typed `error` frame (platform #2807); plain frames stay as they were. */
class GenerationFramesTest {
    @Test
    fun toolProgressCarriesTheLatestMetadata() {
        val e = AgentChatFrames.decode(
            """{"type":"tool_progress","toolCallId":"c1","toolName":"media_generate","metadata":{"progress":"Frame 3 of 12","progress_detail":{"lane":"motion","stage":"rendering","frame":3,"frames":12}}}""",
        )
        assertTrue(e is ChatStreamEvent.ToolProgress)
        e as ChatStreamEvent.ToolProgress
        assertEquals("c1", e.toolCallId)
        assertEquals("media_generate", e.toolName)
        assertEquals("rendering", e.metadata["progress_detail"]!!.jsonObject["stage"].let { (it as JsonPrimitive).content })
    }

    @Test
    fun toolProgressWithoutAnIdOrMetadataIsIgnored() {
        assertNull(AgentChatFrames.decode("""{"type":"tool_progress","metadata":{"progress":"x"}}"""))
        assertNull(AgentChatFrames.decode("""{"type":"tool_progress","toolCallId":"c1"}"""))
    }

    @Test
    fun plainErrorFrameStaysPlain() {
        assertEquals(ChatStreamEvent.StreamError("bad"), AgentChatFrames.decode("""{"type":"error","error":"bad"}"""))
    }

    @Test
    fun typedErrorFrameCarriesCodeAndResetTime() {
        val seconds = AgentChatFrames.decode("""{"type":"error","error":"limit","code":"usage_limit_reached","reset_at":1790000000}""")
        assertEquals(ChatStreamEvent.StreamError("limit", "usage_limit_reached", 1_790_000_000_000L), seconds)
        val millis = AgentChatFrames.decode("""{"type":"error","error":"limit","code":"usage_limit","reset_at":1790000000000}""")
        assertEquals(1_790_000_000_000L, (millis as ChatStreamEvent.StreamError).resetAtMs)
        val text = AgentChatFrames.decode("""{"type":"error","error":"limit","reset_at":"1790000000"}""")
        assertEquals(1_790_000_000_000L, (text as ChatStreamEvent.StreamError).resetAtMs)
        val iso = AgentChatFrames.decode("""{"type":"error","error":"limit","reset_at":"2026-11-22T15:40:00Z"}""")
        assertEquals(java.time.Instant.parse("2026-11-22T15:40:00Z").toEpochMilli(), (iso as ChatStreamEvent.StreamError).resetAtMs)
    }

    @Test
    fun badResetValuesAreDropped() {
        assertNull(AgentChatFrames.resetAtMillis(null))
        assertNull(AgentChatFrames.resetAtMillis(JsonPrimitive(0)))
        assertNull(AgentChatFrames.resetAtMillis(JsonPrimitive("soon")))
    }
}
