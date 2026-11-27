package com.allternit.core.api

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull

/** One skill from `GET /api/v1/mcp/skills` (skills the user's MCP servers and installed apps offer). */
data class ServerSkill(
    val id: String,
    val source: String,
    val from: String,
    val server: String?,
    val connectorId: String?,
    val uri: String?,
    val name: String,
    val description: String,
    val enabled: Boolean,
    val defaultEnabled: Boolean,
)

/** Reads the skills list tolerantly: a missing id becomes `server:name`, a missing `enabled` is on; non-list bodies are empty. */
fun parseServerSkills(body: JsonElement?): List<ServerSkill> {
    val arr = when (body) {
        is JsonArray -> body
        is JsonObject -> (body["skills"] ?: body["items"]) as? JsonArray
        else -> null
    } ?: return emptyList()
    return arr.mapNotNull { el ->
        val o = el as? JsonObject ?: return@mapNotNull null
        fun s(k: String) = (o[k] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotBlank() }
        val name = s("name") ?: s("id") ?: return@mapNotNull null
        val server = s("server")
        val enabled = (o["enabled"] as? JsonPrimitive)?.booleanOrNull ?: true
        ServerSkill(
            id = s("id") ?: "${server ?: s("from") ?: "server"}:$name",
            source = s("source") ?: "server",
            from = s("from") ?: server ?: "your server",
            server = server,
            connectorId = s("connectorId"),
            uri = s("uri"),
            name = name,
            description = s("description").orEmpty(),
            enabled = enabled,
            defaultEnabled = (o["defaultEnabled"] as? JsonPrimitive)?.booleanOrNull ?: true,
        )
    }
}

/** A `load_skill` result read for its steps (shared rule with web and iOS). */
data class ParsedSkillResult(val id: String, val from: String, val steps: List<String>)

object SkillResult {
    const val MAX_STEPS = 20
    private val HEAD = Regex("""^Skill (\S+) from (.+?)\. This text comes from that server\.\s*$""")
    private val STEP = Regex("""^\d+[.)]\s+(.+)""")

    /** Markdown out of a step: links, emphasis, code ticks. */
    fun plainStep(text: String): String = text
        .replace(Regex("""!?\[([^\]]*)\]\([^)]*\)"""), "$1")
        .replace(Regex("""(\*\*|__)(.*?)\1"""), "$2")
        .replace(Regex("""(^|[^\w])[*_]([^*_\n]+)[*_](?=$|[^\w])"""), "$1$2")
        .replace(Regex("""`([^`]*)`"""), "$1")
        .replace(Regex("""\s+"""), " ")
        .trim()

    /** Line 1 names id and server; steps are unindented numbered lines before `ARGUMENTS:`, at most 20. Null when line 1 does not match. */
    fun parse(outputText: String): ParsedSkillResult? {
        val lines = outputText.split('\n')
        val head = HEAD.find((lines.firstOrNull() ?: "").trim()) ?: return null
        val steps = mutableListOf<String>()
        for (line in lines.drop(1)) {
            if (line.startsWith("ARGUMENTS:")) break
            val m = STEP.find(line) ?: continue
            val step = plainStep(m.groupValues[1])
            if (step.isNotEmpty()) steps += step
            if (steps.size >= MAX_STEPS) break
        }
        return ParsedSkillResult(head.groupValues[1], head.groupValues[2], steps)
    }
}
