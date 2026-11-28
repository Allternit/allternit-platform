package com.allternit.feature.chat

import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** The no-computer chat client (iOS PersonChatClient): SSE reading, refusals, and which sends go to cloud-api. */
class PersonChatTest {
    private fun read(vararg lines: String): List<PersonChatEvent> {
        val r = CloudStreamReader("Allternit Cloud")
        val out = mutableListOf<PersonChatEvent>()
        for (l in lines) { out += r.line(l); if (r.finished) break }
        return out + r.end()
    }

    @Test
    fun textDeltasThenDone() {
        val events = read(
            ": keepalive",
            """data: {"choices":[{"delta":{"role":"assistant","content":"Hel"}}]}""",
            "",
            """data: {"choices":[{"delta":{"content":"lo"}}]}""",
            "data: [DONE]",
            """data: {"choices":[{"delta":{"content":"after done"}}]}""",
        )
        assertEquals(listOf(PersonChatEvent.Delta("Hel"), PersonChatEvent.Delta("lo"), PersonChatEvent.Done), events)
    }

    @Test
    fun toolCallDeltasJoinByIndex() {
        val events = read(
            """data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"bash","arguments":""}}]}}]}""",
            """data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"comm"}}]}}]}""",
            """data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_b","function":{"name":"read_file","arguments":"{\"path\":\"/etc/os-release\"}"}}]}}]}""",
            """data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"and\":\"ls /\"}"}}]}}]}""",
            """data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}""",
            "data: [DONE]",
        )
        assertEquals(
            listOf(
                PersonChatEvent.ToolCalls(listOf(
                    PersonToolCall("call_a", "bash", """{"command":"ls /"}"""),
                    PersonToolCall("call_b", "read_file", """{"path":"/etc/os-release"}"""),
                )),
                PersonChatEvent.Done,
            ),
            events,
        )
    }

    @Test
    fun codedErrorInStreamIsARefusal() {
        try {
            read("""data: {"error":{"code":"credits_exhausted","message":"no credits"}}""")
            fail("expected a refusal")
        } catch (r: PersonChatRefusal) {
            assertEquals("credits_exhausted", r.code)
            assertEquals("Your Allternit Cloud credits are used up.", r.plainText)
            assertTrue(r.isCreditsRefusal)
            assertEquals("Allternit Cloud didn't answer", r.card().title)
        }
    }

    @Test
    fun httpRefusalsMapToPlainWords() {
        val flat402 = CloudStreamReader.refusal(402, """{"error":"payment"}""", "Allternit Cloud")
        assertEquals("credits_required", flat402.code)
        assertEquals("Allternit Cloud models need credits.", flat402.plainText)
        val plan = CloudStreamReader.refusal(409, """{"error":{"code":"plan_turn_in_flight","message":"busy"}}""", "Claude")
        assertEquals("Your Claude plan is still answering another message. Wait for it to finish.", plan.plainText)
        assertEquals("signed_out_allternit", CloudStreamReader.refusal(401, "<html>", "Allternit Cloud").code)
        assertEquals("Allternit Cloud didn't answer. Try again.", CloudStreamReader.refusal(500, null, "Allternit Cloud").plainText)
    }

    @Test
    fun bodyCarriesBareCloudAliasAndStream() {
        val body = PersonChatClient.body("allternit/glm-5.3", listOf(PersonChatClient.text("user", "hi")))
        assertEquals(JsonPrimitive("glm-5.3"), body["model"])
        assertEquals(JsonPrimitive(true), body["stream"])
        assertTrue("tools" !in body)
        assertEquals(JsonPrimitive("plan/claude/sonnet"), PersonChatClient.body("plan/claude/sonnet", emptyList())["model"])
    }

    @Test
    fun noComputerBranchChoice() {
        // Plan models always go to cloud-api, even inside a computer chat.
        assertEquals(SendRoute.Person, SendRoutes.route("plan/claude/sonnet", chatOnComputer = true, computerLists = true))
        // A Cloud model with no computer catalog listing it: cloud-api, never the NoComputer gate.
        assertEquals(SendRoute.Person, SendRoutes.route("allternit/glm-5.3", chatOnComputer = false, computerLists = false))
        // Catalog unknown (bots, office): resolve the relay first.
        assertEquals(SendRoute.CheckComputer, SendRoutes.route("allternit/glm-5.3", chatOnComputer = false, computerLists = null))
        // A chat already on a computer keeps it (D8), and so does a computer that serves the model.
        assertEquals(SendRoute.Computer, SendRoutes.route("allternit/glm-5.3", chatOnComputer = true, computerLists = false))
        assertEquals(SendRoute.Computer, SendRoutes.route("allternit/glm-5.3", chatOnComputer = false, computerLists = true))
        // Computer models and no model at all stay on the computer path.
        assertEquals(SendRoute.Computer, SendRoutes.route("anthropic/claude-sonnet", chatOnComputer = false, computerLists = null))
        assertEquals(SendRoute.Computer, SendRoutes.route(null, chatOnComputer = false, computerLists = null))
    }
}
