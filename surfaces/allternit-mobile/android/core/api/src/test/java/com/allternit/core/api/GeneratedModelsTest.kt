package com.allternit.core.api

import com.allternit.core.api.gen.AgentChatMetadata
import com.allternit.core.api.gen.AgentChatRequest
import com.allternit.core.api.gen.AgentSession
import com.allternit.core.api.gen.AgentSessionList
import com.allternit.core.api.gen.AgentSessionMessage
import com.allternit.core.api.gen.CreateAgentSessionRequest
import com.allternit.core.api.gen.CreateOrganizationResponse
import com.allternit.core.api.gen.MeResponse
import com.allternit.core.api.gen.ProviderList
import com.allternit.core.api.gen.RegisterDeviceTokenRequest
import com.allternit.core.api.gen.RegisterDeviceTokenResponse
import com.allternit.core.api.gen.RuntimeModel
import com.allternit.core.api.gen.RuntimeModelTier
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Decodes the bodies the Rust handlers send (shapes copied from the handler named on each
 * fixture) and the iOS test fixtures, through the generated models and [ApiJson.default].
 */
class GeneratedModelsTest {
    private val json = ApiJson.default

    // ── account ── cmd/allternit-api/src/me_routes.rs

    @Test
    fun meDecodesFullProfileAndTheFallbackRow() {
        // get_current_user, DB row (`UserProfile` struct).
        val full = """{"user":{"id":"u_1","clerk_id":"user_2x","email":"a@b.co","name":"Ada","avatar_url":null,
            "role":"user","status":"active","created_at":"2026-11-01T10:00:00Z","organization_id":"org_9","organization_role":"admin"}}"""
        val me = json.decodeFromString(MeResponse.serializer(), full).user
        assertEquals("org_9", me.organizationId)
        assertEquals("user_2x", me.clerkId)
        assertNull(me.avatarUrl)

        // get_current_user, QueryReturnedNoRows fallback: no clerk_id / avatar_url / created_at.
        val fallback = """{"user":{"id":"u_1","email":null,"name":null,"role":"user","status":"active",
            "organization_id":null,"organization_role":null}}"""
        val bare = json.decodeFromString(MeResponse.serializer(), fallback).user
        assertNull(bare.createdAt)
        assertNull(bare.organizationId)
    }

    @Test
    fun createOrganizationDecodes() {
        // create_personal_organization: `{ "organization_id": existing, "created": false }`.
        val r = json.decodeFromString(CreateOrganizationResponse.serializer(), """{"organization_id":"org_1","created":false}""")
        assertEquals("org_1", r.organizationId)
        assertFalse(r.created)
    }

    // ── sessions ── cmd/allternit-api/src/agent_session_routes.rs

    @Test
    fun sessionListDecodesTransformSessionShape() {
        // list_sessions → { sessions, count }; each row is transform_session().
        val body = """{"sessions":[{"id":"ses_1","name":"Trip plan","description":null,
            "created_at":"2026-11-18T09:00:00Z","updated_at":"2026-11-18T09:05:00Z","last_accessed":"2026-11-18T09:05:00Z",
            "message_count":0,"active":true,"tags":[],"environment":null,
            "metadata":{"agentId":null,"originSurface":"chat","ephemeral":false,"paused":null,"persona":"gizzi","limit":{"kind":"x"}}}],
            "count":1}"""
        val list = json.decodeFromString(AgentSessionList.serializer(), body)
        assertEquals(1L, list.count)
        val s = list.sessions.single()
        assertEquals("Trip plan", s.name)
        assertEquals(true, s.active)
        assertEquals("chat", s.metadata!!.jsonObject["originSurface"]!!.jsonPrimitive.content)
    }

    @Test
    fun sessionDecodesTheIosShellSessionsStoreFixture() {
        // surfaces/allternit-mobile/ios/Tests/ShellSessionsStoreTests.swift session(_:updated:).
        val s = json.decodeFromString(
            AgentSession.serializer(),
            """{"id":"a","name":"a","created_at":"2026-01-02T00:00:00Z","updated_at":"2026-01-02T00:00:00Z"}""",
        )
        assertEquals("a", s.id)
        assertNull(s.messageCount)
    }

    @Test
    fun createSessionEncodesServerFieldNamesAndOmitsNulls() {
        // CreateSessionBody: snake_case fields, `environmentId` renamed.
        val body = CreateAgentSessionRequest(name = "New chat", originSurface = "chat", environmentId = "env_1")
        val encoded = json.parseToJsonElement(json.encodeToString(CreateAgentSessionRequest.serializer(), body)).jsonObject
        assertEquals(setOf("name", "origin_surface", "environmentId"), encoded.keys)
    }

    @Test
    fun messagesDecodeTransformMessageShape() {
        // list_messages → bare array of transform_message_with().
        val body = """[{"id":"msg_1","role":"assistant","content":"Hi","thinking":null,"timestamp":"2026-11-18T09:00:01Z",
            "metadata":{"agent":"gizzi","model":{"providerID":"anthropic","modelID":"x"},"telemetry":null,"handoff":null,
            "cloudContinue":null,"parts":[{"type":"text","text":"Hi"}],"error":null}}]"""
        val messages = json.decodeFromString(ListSerializer(AgentSessionMessage.serializer()), body)
        assertEquals("Hi", messages.single().content)
        assertTrue(messages.single().metadata is JsonObject)
    }

    // ── models ── cmd/allternit-api/src/v1_routes.rs live_model_catalog + model_thinking.rs thinking_json

    @Test
    fun modelCatalogDecodesAndUnknownTierFallsBack() {
        val body = """[
            {"id":"anthropic/claude-x","name":"Claude X (Anthropic)","provider":"anthropic","description":"Best",
             "tier":"flagship","supports_effort":true,"default":true,"rank":1,
             "thinking":{"vendor":"anthropic","vendor_name":"Anthropic","term":"Effort","note":"","toggle":false,
               "levels":[{"id":"low","label":"Low"},{"id":"high","label":"High"}],"default":"high","fast":false,"source":"table"}},
            {"id":"local/qwen","name":"Qwen (Local)","provider":"local","description":null,"tier":"experimental",
             "supports_effort":false,"thinking":null,"default":false,"rank":null,"hidden":true}
        ]"""
        val models = json.decodeFromString(ListSerializer(RuntimeModel.serializer()), body)
        assertEquals(RuntimeModelTier.Flagship, models[0].tier)
        assertEquals(listOf("low", "high"), models[0].thinking!!.levels.map { it.id })
        // A tier this build does not know must not fail the whole catalog.
        assertEquals(RuntimeModelTier.Unknown, models[1].tier)
        assertEquals(true, models[1].hidden)
    }

    @Test
    fun providersDecode() {
        // provider_routes.rs list_providers → { providers, all } of ProviderInfo.
        val body = """{"providers":[{"id":"openai","name":"OpenAI","provider_type":"openai","base_url":null,"api_key_set":true,
            "models":[{"id":"gpt-5","name":"GPT-5"}],"status":"connected"}],"all":[]}"""
        val p = json.decodeFromString(ProviderList.serializer(), body)
        assertEquals("gpt-5", p.providers.single().models.single().id)
    }

    // ── push ── cmd/allternit-cloud-api/src/routes/apns.rs

    @Test
    fun deviceTokenUsesCamelCaseWireNames() {
        // RegisterBody is #[serde(rename_all = "camelCase")].
        val body = RegisterDeviceTokenRequest(token = "ab".repeat(32), platform = "ios", deviceId = "d1", osVersion = "17", appBuild = "1")
        val keys = json.parseToJsonElement(json.encodeToString(RegisterDeviceTokenRequest.serializer(), body)).jsonObject.keys
        assertEquals(setOf("token", "platform", "deviceId", "osVersion", "appBuild"), keys)
        // register_route 201: { ok, id, readToken }.
        val r = json.decodeFromString(RegisterDeviceTokenResponse.serializer(), """{"ok":true,"id":"apns_1","readToken":"adr_1_ff"}""")
        assertEquals("adr_1_ff", r.readToken)
    }

    // ── chat request ── cmd/allternit-api/src/v1_routes.rs agent_chat_bridge

    @Test
    fun chatRequestCarriesOnlySetKeys() {
        val body = AgentChatRequest(chatId = "ses_1", message = "hi", metadata = AgentChatMetadata(formFactor = "phone", answerCards = "widgets"))
        val obj = json.parseToJsonElement(json.encodeToString(AgentChatRequest.serializer(), body)).jsonObject
        assertEquals(setOf("chatId", "message", "metadata"), obj.keys)
        assertEquals(setOf("form_factor", "answer_cards"), obj["metadata"]!!.jsonObject.keys)
    }

    // ── errors ──

    @Test
    fun errorBodiesParseBothShapes() {
        // v1_routes.rs models_unavailable(): { error, message }.
        val a = ApiCall.httpError(503, """{"error":"models_unavailable","message":"The model list is unavailable right now. Try again in a moment."}""")
        assertEquals("models_unavailable", a.code)
        assertEquals(503, a.status)
        // apns.rs error(): { error } only.
        assertEquals("too_many_devices", ApiCall.httpError(429, """{"error":"too_many_devices"}""").code)
        // Object form `{"error":{"type","message","code"}}`.
        val c = ApiCall.httpError(400, """{"error":{"type":"invalid_request","message":"bad","code":"bad_field"}}""")
        assertEquals("bad_field", c.code)
        assertEquals("bad", c.message)
        // Not JSON (a proxy's HTML page).
        assertNull(ApiCall.httpError(502, "<html>").code)
    }
}
