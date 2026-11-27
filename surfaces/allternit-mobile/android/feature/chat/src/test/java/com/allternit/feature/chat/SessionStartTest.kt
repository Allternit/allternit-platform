package com.allternit.feature.chat

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.int
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Android client against the contract fixtures (docs/contracts/session-start, copied into test resources). */
class SessionStartTest {
    private fun fixture(name: String) =
        Json.parseToJsonElement(checkNotNull(javaClass.classLoader?.getResource("session-start/$name.json")) { "missing $name" }.readText())

    @Test
    fun decodesOptions() {
        val o = checkNotNull(StartDecode.options(fixture("options.response").jsonObject))
        assertEquals("rt_8f3a21c9", o.device.runtimeId)
        assertTrue(o.device.remoteStart)
        assertEquals(4, o.folders.size)
        assertEquals("allternit", o.folders[0].name)
        assertEquals("main", o.folders[0].git?.branch)
        assertEquals(true, o.folders[1].git?.dirty)
        assertEquals(2, o.folders[1].git?.ahead)
        assertNull(o.folders[2].git)
        assertEquals("allternit", o.defaultRuntime)
        assertEquals(StartMode.Default, o.modes["allternit"]?.default)
        // codex has no plan mode (#3172 fix e46ef04a3).
        assertEquals(listOf(StartMode.Auto, StartMode.Default, StartMode.Bypass), o.modes["codex"]?.supported)
        assertEquals(StartMode.Auto, o.modes["codex"]?.default)
    }

    @Test
    fun decodesSnapshots() {
        val body = fixture("presence.delta").jsonObject["getStartOptions"]!!.jsonObject["response"]!!.jsonObject
        val s = StartDecode.snapshots(body).single()
        assertEquals("desktop", s.kind)
        assertEquals("https://github.com/example-org/allternit.git", s.folders.single().git?.remote)
    }

    @Test
    fun decodesBothResponseKinds() {
        val engine = StartDecode.response(fixture("start.response.engine").jsonObject) as StartResponse.Engine
        assertEquals("ses_01JD4Z8K2Q7W3M9X5T6V1N0B8C", engine.sessionId)
        assertEquals("/Users/dev/code/allternit", engine.directory)
        val terminal = StartDecode.response(fixture("start.response.terminal").jsonObject) as StartResponse.Terminal
        assertEquals("term_7c1e9a40", terminal.terminalId)
        assertEquals("claude", terminal.agent)
    }

    @Test
    fun encodesRequestsLikeTheFixtures() {
        val origin = StartOrigin("ios", "Eoj's iPhone", depth = 0)
        val prompt = "Find why the sidebar flickers on resize and fix it."
        val path = StartRequest(StartFolderRef.Path("/Users/dev/code/allternit"), "allternit", StartMode.Default, prompt = prompt, origin = origin)
        assertEquals(fixture("start.request.path"), path.toJson())
        val git = StartRequest(StartFolderRef.Git("https://github.com/example-org/allternit.git", "main"), "allternit", StartMode.Default, prompt = prompt, origin = origin)
        assertEquals(fixture("start.request.git"), git.toJson())
    }

    @Test
    fun errorWordingMatchesTheContract() {
        val rows = fixture("errors") as JsonArray
        assertEquals(rows.size, StartDecode.ERRORS.size)
        rows.forEach { el ->
            val r = el as JsonObject
            val code = r["error"]!!.jsonPrimitive.content
            // A refusal with no body message still reads the contract's words.
            val e = StartDecode.error(r["status"]!!.jsonPrimitive.int, code, null)
            assertEquals(r["message"]!!.jsonPrimitive.content, e.message)
            assertEquals(r["action"]!!.jsonPrimitive.content, e.action)
        }
    }

    @Test
    fun chipRuntimeIdsMapToContractIds() {
        // Same table as iOS SessionStartClient.contractRuntime: a chip id sent as-is would be runtime_missing.
        assertEquals("allternit", contractRuntime("gizzi"))
        assertEquals("allternit", contractRuntime("allternit"))
        assertEquals("claude", contractRuntime("claude-code"))
        assertEquals("codex", contractRuntime("codex-cli"))
        assertEquals("kimi", contractRuntime("kimi-cli"))
        assertEquals("opencode", contractRuntime("opencode"))
    }

    @Test
    fun pickAppliesOnlyToItsOwnChat() {
        PendingStart.clear()
        PendingStart.openChatId = null
        PendingStart.runtimeId = "rt_1"
        PendingStart.chatId = null
        assertTrue(PendingStart.appliesTo(null, hasMessages = false))
        assertEquals(false, PendingStart.appliesTo(null, hasMessages = true))
        PendingStart.chatId = "chat_a"
        assertEquals(false, PendingStart.appliesTo("chat_b", hasMessages = false))
        assertTrue(PendingStart.appliesTo("chat_a", hasMessages = true))
        PendingStart.clear()
        assertEquals(false, PendingStart.appliesTo(null, hasMessages = false))
    }

    @Test
    fun bindingRoundTripsThroughMetadata() {
        val b = StartBinding("rt_1", "MacBook Pro", "/Users/dev/code/allternit", "allternit", StartMode.Plan, "engine", sessionId = "ses_1", origin = StartOrigin("android", "Pixel 9"))
        val meta = JsonObject(mapOf(StartBinding.KEY to b.toJson()))
        assertEquals(b, StartBinding.read(meta))
        assertNull(StartBinding.read(JsonObject(mapOf(StartBinding.KEY to JsonObject(emptyMap())))))
    }
}
