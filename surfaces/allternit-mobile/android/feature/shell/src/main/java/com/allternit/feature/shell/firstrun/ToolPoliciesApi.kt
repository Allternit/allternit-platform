package com.allternit.feature.shell.firstrun

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** The three answers a tool policy can hold. `ask` is the default and is never stored on the server. */
internal enum class ToolPolicy(val wire: String, val label: String) {
    Ask("ask", "Ask"), Allow("allow", "Allow"), Block("block", "Block");

    companion object {
        fun of(raw: String?) = entries.firstOrNull { it.wire == raw } ?: Ask
    }
}

/** One tool of a connection, as the connector directory lists it. */
internal data class ConnTool(val id: String, val readOnly: Boolean)

/** Same guess the web Customize Tools tab makes (`WRITE_VERB`); the grouping is a name guess, not exact. */
private val WRITE_VERB = Regex("^(create|update|delete|remove|write|edit|send|post|put|patch|set|add|upload|move|rename|archive|publish|run|execute)", RegexOption.IGNORE_CASE)

/**
 * `GET` and `PUT /api/v1/tool-policies` (account-wide Ask / Allow / Block per tool), plus the tool list of one
 * connector from `GET /api/v1/connectors`. The owner id is `connectors:<id>` (or `mcps:<id>`).
 */
internal object ToolPoliciesClient {
    fun owner(connectorId: String) = "connectors:$connectorId"

    /** owner id to (tool id to policy), only for tools that are not Ask. A 404 means this server can't save them yet. */
    suspend fun load(): Map<String, Map<String, ToolPolicy>> {
        val rows = SetupHttp.v1("GET", "tool-policies")["policies"].arr()
        val out = mutableMapOf<String, MutableMap<String, ToolPolicy>>()
        for (r in rows) {
            val owner = r["owner_id"].str() ?: continue
            val tool = r["tool_id"].str() ?: continue
            out.getOrPut(owner) { mutableMapOf() }[tool] = ToolPolicy.of(r["policy"].str())
        }
        return out
    }

    /** One PUT for any number of tools; `ask` deletes the row on the server. */
    suspend fun save(owner: String, changes: Map<String, ToolPolicy>) {
        val body = buildJsonObject {
            put("policies", JsonArray(changes.map { (tool, policy) ->
                buildJsonObject { put("owner_id", owner); put("tool_id", tool); put("policy", policy.wire) }
            }))
        }
        SetupHttp.v1("PUT", "tool-policies", body)
    }

    /** The tools of one connector: names as the backend lists them, without any `vendor.` prefix (iOS `toolNames`). */
    suspend fun tools(connectorId: String): List<ConnTool> {
        val json = SetupHttp.v1("GET", "connectors")
        val list = (json["connectors"] ?: json["items"]).arr().ifEmpty { json.arr() }
        val connector = list.firstOrNull { it["id"].str() == connectorId } ?: return emptyList()
        return connector["tools"].arr().mapNotNull { row -> toolName(row) }
            .map { it.substringAfter('.') }.filter { it.isNotEmpty() }.distinct()
            .map { ConnTool(it, readOnly = !WRITE_VERB.containsMatchIn(it)) }
    }

    private fun toolName(row: JsonElement): String? = when (row) {
        is JsonPrimitive -> row.str()
        is JsonObject -> row["name"].str() ?: row["id"].str()
        else -> null
    }?.takeIf { it.isNotEmpty() }

    /** True when the server has no such route (an older server). */
    fun unsupported(e: Throwable) = e is SetupError && e.status == 404
}

/** "2 allowed, 1 blocked", or null when every tool is Ask. */
internal fun toolPolicySummary(policies: Map<String, ToolPolicy>?): String? {
    val allowed = policies?.values?.count { it == ToolPolicy.Allow } ?: 0
    val blocked = policies?.values?.count { it == ToolPolicy.Block } ?: 0
    return listOfNotNull(
        allowed.takeIf { it > 0 }?.let { "$it allowed" },
        blocked.takeIf { it > 0 }?.let { "$it blocked" },
    ).joinToString(", ").ifEmpty { null }
}
