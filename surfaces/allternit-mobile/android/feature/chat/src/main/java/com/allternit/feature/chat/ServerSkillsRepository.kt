package com.allternit.feature.chat

import com.allternit.core.api.ServerSkill
import com.allternit.core.api.parseServerSkills
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Server skills for this account: `GET /api/v1/mcp/skills` and `PUT /api/v1/mcp/skills/prefs`, relayed
 * to the person's computer like the other `/api/v1/mcp` routes. Cached 5 minutes. A missing route,
 * no computer or any failure reads as an empty list, so the composer shows nothing and no error.
 * The shell attaches its chat backend once with [attach].
 */
object ServerSkillsRepository {
    private const val TTL_MS = 5 * 60 * 1000L

    @Volatile private var backend: ChatBackend? = null
    @Volatile private var cached: List<ServerSkill>? = null
    @Volatile private var at = 0L

    fun attach(b: ChatBackend) {
        if (backend !== b) {
            backend = b
            cached = null
        }
    }

    /** All skills (on or off). [force] skips the cache. */
    suspend fun list(force: Boolean = false): List<ServerSkill> {
        val hit = cached
        if (!force && hit != null && System.currentTimeMillis() - at < TTL_MS) return hit
        val b = backend ?: return emptyList()
        val fresh = when (val r = b.runtimeJson("GET", "/api/v1/mcp/skills")) {
            is RuntimeCall.Ok -> parseServerSkills(r.body)
            else -> return hit ?: emptyList()
        }
        cached = fresh
        at = System.currentTimeMillis()
        return fresh
    }

    /** The skills a turn may use. */
    suspend fun enabled(): List<ServerSkill> = list().filter { it.enabled }

    /** Saves one switch. True when the server accepted it; the cache is dropped so the next read is fresh. */
    suspend fun setEnabled(id: String, enabled: Boolean): Boolean {
        val b = backend ?: return false
        val body = JsonObject(mapOf("id" to JsonPrimitive(id), "enabled" to JsonPrimitive(enabled))).toString()
        val ok = b.runtimeJson("PUT", "/api/v1/mcp/skills/prefs", body) is RuntimeCall.Ok
        if (ok) cached = null
        return ok
    }

    /** The enabled skill a message starts with (`/<id>` then a space or the end), plus the rest as arguments. */
    fun match(skills: List<ServerSkill>, text: String): Pair<ServerSkill, String?>? {
        if (!text.startsWith("/")) return null
        val token = text.drop(1).substringBefore(' ').substringBefore('\n')
        val skill = skills.firstOrNull { it.enabled && it.id == token } ?: return null
        val rest = text.drop(1 + token.length).trim()
        return skill to rest.ifEmpty { null }
    }
}
