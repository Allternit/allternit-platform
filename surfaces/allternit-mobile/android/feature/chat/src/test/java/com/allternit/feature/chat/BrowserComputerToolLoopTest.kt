package com.allternit.feature.chat

import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** CONTRACT-colibri-unkernel section 7: the browser computer tool loop with a fake computer and a fake model stream (the two edges). */
class BrowserComputerToolLoopTest {
    private class FakeComputer(var reply: BoxExecResult = BoxExecResult("ok\n", "", 0)) : BrowserComputerExecutor {
        val calls = mutableListOf<BoxExecParams>()
        override suspend fun exec(params: BoxExecParams): BoxExecResult { calls += params; return reply }
    }

    private fun user(text: String) = PersonChatClient.text("user", text)
    private fun bashCall(n: Int) = BrowserToolCall("call_$n", "bash", """{"command":"echo $n"}""")

    @Test fun roundCapSendsOneLastRequestWithNoToolsAndShowsTheCount() = runBlocking {
        val computer = FakeComputer()
        val requests = mutableListOf<BrowserToolRequest>()
        // The model asks for a tool on every request, even the last one.
        val result = BrowserToolLoop.run(BrowserToolBrain.Tools, listOf(user("go")), computer, { request ->
            requests += request
            BrowserToolResponse(listOf(bashCall(requests.size)))
        })
        assertEquals(8, result.rounds)
        assertEquals("Used 8 of 8 tool rounds", result.roundsLine)
        assertEquals(9, requests.size)
        assertTrue(requests.last().toolChoiceNone)
        assertFalse(requests.dropLast(1).any { it.toolChoiceNone })
        // The call in the last request is ignored: only 8 commands ran.
        assertEquals(8, computer.calls.size)
    }

    @Test fun outputOverSixteenKiBKeepsBothEndsAndSaysSo() = runBlocking {
        val computer = FakeComputer(BoxExecResult("a".repeat(10_000) + "z".repeat(30_000), "", 0))
        var sent: List<JsonObject> = emptyList()
        var turn = 0
        BrowserToolLoop.run(BrowserToolBrain.Tools, listOf(user("go")), computer, { request ->
            turn += 1
            sent = request.messages
            BrowserToolResponse(if (turn == 1) listOf(bashCall(1)) else emptyList())
        })
        val content = sent.last()["content"]!!.jsonPrimitive.content
        assertTrue(content.toByteArray().size <= BrowserToolCaps.RESULT_MESSAGE_BYTES)
        val json = Json.parseToJsonElement(content).jsonObject
        assertTrue(json["truncated"]!!.jsonPrimitive.boolean)
        val stdout = json["stdout"]!!.jsonPrimitive.content
        assertTrue(stdout.startsWith("aaaa"))
        assertTrue(stdout.endsWith("zzzz"))
        assertTrue(stdout.contains("bytes cut ...]"))
    }

    @Test fun planBrainGetsNoToolsAndThePlainLine() = runBlocking {
        val computer = FakeComputer()
        val requests = mutableListOf<BrowserToolRequest>()
        val text = StringBuilder()
        val result = BrowserToolLoop.run(BrowserToolBrain.Plan, listOf(user("hi")), computer, { request ->
            requests += request
            request.onDelta("Hello.")
            BrowserToolResponse()
        }, BrowserToolLoopCallbacks(onChunk = { text.append(it) }))
        assertEquals(1, requests.size)
        assertNull(requests[0].tools)
        assertEquals("Hello.\n\nPlan models cannot run commands. Open the Terminal to use your computer yourself.", text.toString())
        assertEquals(0, result.rounds)
        assertTrue(computer.calls.isEmpty())
    }

    @Test fun badArgumentsAreAnErrorTheLoopSurvives() = runBlocking {
        val computer = FakeComputer()
        val errors = mutableListOf<String>()
        var turn = 0
        val result = BrowserToolLoop.run(BrowserToolBrain.Tools, emptyList(), computer, {
            turn += 1
            BrowserToolResponse(if (turn == 1) listOf(BrowserToolCall("c", "bash", "{nope")) else emptyList())
        }, BrowserToolLoopCallbacks(onToolError = { _, _, e -> errors += e }))
        assertEquals(listOf("The tool arguments were not valid JSON."), errors)
        assertEquals(1, result.rounds)
        assertTrue(computer.calls.isEmpty())
    }
}
