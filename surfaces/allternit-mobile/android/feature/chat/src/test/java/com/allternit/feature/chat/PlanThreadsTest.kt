package com.allternit.feature.chat

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The stored no-computer chat client (iOS PlanThreadClient / PlanThreadClientTests): bodies, parsing, which chats are stored. */
class PlanThreadsTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject

    @Test
    fun storedModelsMatchIos() {
        assertTrue(PlanThreadClient.isStored("plan/claude/sonnet"))
        assertTrue(PlanThreadClient.isStored("copilot/gpt"))
        assertTrue(PlanThreadClient.isStored("linked/claude/sonnet"))
        assertTrue(PlanThreadClient.isStored("allternit/glm-5.3"))
        assertFalse(PlanThreadClient.isStored("chatgpt/gpt-5"))
        assertFalse(PlanThreadClient.isStored("allternit/"))
        assertFalse(PlanThreadClient.isStored("ondevice/bonsai"))
    }

    @Test
    fun threadIdsHaveThePthPrefix() {
        assertTrue(PlanThreadClient.isThreadId("pth_abc"))
        assertFalse(PlanThreadClient.isThreadId("ses_abc"))
        assertFalse(PlanThreadClient.isThreadId(null))
    }

    @Test
    fun createBodyCarriesModelTitleAndFirstTurn() {
        val body = PlanThreadClient.createBody("allternit/glm-5.3", "Hi there", listOf(PlanThreadMessage("user", "Hi there", "allternit/glm-5.3")))
        assertEquals("allternit/glm-5.3", (body["model"] as JsonPrimitive).content)
        assertEquals("Hi there", (body["title"] as JsonPrimitive).content)
        val m = (body["messages"] as JsonArray).single() as JsonObject
        assertEquals("user", (m["role"] as JsonPrimitive).content)
        assertEquals("Hi there", (m["content"] as JsonPrimitive).content)
        assertEquals("allternit/glm-5.3", (m["model"] as JsonPrimitive).content)
        assertFalse(m.containsKey("at"))
    }

    @Test
    fun appendBodyIsCappedAtFiftyMessages() {
        val many = (1..80).map { PlanThreadMessage("user", "m$it") }
        assertEquals(50, (PlanThreadClient.appendBody(many)["messages"] as JsonArray).size)
    }

    @Test
    fun titleIsTheFirstSixtyCharactersOfTheTrimmedTurn() {
        assertEquals("Hello", PlanThreadClient.titleFor("  Hello \n"))
        assertEquals(60, PlanThreadClient.titleFor("x".repeat(100)).length)
        // Never splits an emoji in half.
        assertEquals("😀".repeat(60), PlanThreadClient.titleFor("😀".repeat(70)))
    }

    @Test
    fun listReadsTheThreadsEnvelopeAndDropsBrokenRows() {
        val rows = PlanThreadClient.listFrom(obj("""{"threads":[
            {"id":"pth_1","title":"One","model":"plan/claude/sonnet","message_count":2,"created_at":"2026-11-27T10:00:00Z","updated_at":"2026-11-27T10:05:00Z"},
            {"id":"pth_2","model":"allternit/glm-5.3","message_count":0,"created_at":"a","updated_at":"b"},
            {"title":"no id"}]}"""))!!
        assertEquals(listOf("pth_1", "pth_2"), rows.map { it.id })
        assertEquals("One", rows[0].title)
        assertNull(rows[1].title)
        assertEquals(2, rows[0].messageCount)
    }

    @Test
    fun detailReadsMessagesAndAMissingListIsEmpty() {
        val full = PlanThreadClient.detailFrom(obj("""{"id":"pth_1","title":"One","model":"m","message_count":2,"created_at":"a","updated_at":"b",
            "messages":[{"role":"user","content":"Hi","at":"t"},{"role":"assistant","content":"Hello","model":"m"}]}"""))!!
        assertEquals(listOf("user" to "Hi", "assistant" to "Hello"), full.messages.map { it.role to it.content })
        assertEquals("t", full.messages[0].at)
        val bare = PlanThreadClient.detailFrom(obj("""{"id":"pth_1","model":"m","message_count":0,"created_at":"a","updated_at":"b"}"""))!!
        assertTrue(bare.messages.isEmpty())
        assertNull(PlanThreadClient.detailFrom(obj("""{"title":"x"}""")))
    }

    @Test
    fun aThreadShowsInHistoryAsAChatRow() {
        val s = PlanThreadSummary("pth_1", "One", "m", 3, "2026-11-27T10:00:00Z", "2026-11-27T10:05:00Z").asSession()
        assertEquals("pth_1", s.id)
        assertEquals("One", s.name)
        assertEquals(3L, s.messageCount)
        assertEquals("chat", ((s.metadata as JsonObject)["originSurface"] as JsonPrimitive).content)
        assertNull(PlanThreadSummary("pth_2", "", "m", 0, "a", "b").asSession().name)
    }
}
