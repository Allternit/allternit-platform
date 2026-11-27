package com.allternit.feature.chat

import android.provider.Settings
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * MCP 2027 UI, items 4 and 5 (Code Mode row, typed results). The same labels and order as iOS and web:
 * "Ran a script · N servers · M calls" with Script / Calls / Result stacked; "Got a <tool>" with a
 * key-value card, a Table/Raw switch and "Report a problem to this server".
 */

// MARK: - Metadata (state.metadata.mcp / .mcpScript, falling back to top-level metadata)

internal data class McpCall(val server: String, val tool: String, val ms: Long?, val ok: Boolean, val error: String?)

internal data class McpScriptMeta(val script: String, val servers: List<String>, val calls: List<McpCall>, val durationMs: Long?, val result: JsonElement?) {
    companion object {
        fun from(o: JsonObject?): McpScriptMeta? {
            o ?: return null
            val script = o.str("script") ?: return null
            val calls = o.arr("calls").mapNotNull { it as? JsonObject }.map {
                McpCall(it.str("server") ?: "", it.str("tool") ?: "", it.long("ms"), it.bool("ok") != false, it.str("error"))
            }
            val servers = o.arr("servers").mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }
                .ifEmpty { calls.map { it.server }.distinct() }
            return McpScriptMeta(script, servers, calls, o.long("durationMs"), o["result"])
        }
    }
}

/** [server] is the gizzi proxy name; [connectorId] is what `mcp/connectors/:id/feedback` expects. */
internal data class McpTypedMeta(val server: String, val tool: String, val structured: JsonObject?, val canReport: Boolean, val connectorId: String? = null, val connectorName: String? = null) {
    companion object {
        fun from(o: JsonObject?): McpTypedMeta? {
            o ?: return null
            val server = o.str("server") ?: return null
            return McpTypedMeta(server, o.str("tool") ?: "", o.obj("structuredContent"), o.bool("canReport") == true, o.str("connectorId"), o.str("connectorName"))
        }
    }
}

/** The carrier is `state.metadata`; older frames put it on the part, so [ToolCallRecord.metadata] is read first. */
internal fun ToolCallRecord.mcpMeta(): McpTypedMeta? = McpTypedMeta.from(metadata?.obj("mcp"))
internal fun ToolCallRecord.mcpScriptMeta(): McpScriptMeta? = McpScriptMeta.from(metadata?.obj("mcpScript"))

// MARK: - Motion (one-shot, respects the animator duration scale)

@Composable
fun rememberMcpReduceMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) { Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
}

/** A single pop (scale 1 to 1.18 and back) each time [trigger] changes; none at 0 or with animations off. */
@Composable
fun Modifier.mcpPop(trigger: Int): Modifier {
    val reduced = rememberMcpReduceMotion()
    val scale = remember { Animatable(1f) }
    LaunchedEffect(trigger) {
        if (trigger > 0 && !reduced) {
            scale.animateTo(1.18f, spring(stiffness = 900f))
            scale.animateTo(1f, spring(dampingRatio = 0.5f, stiffness = 500f))
        }
    }
    return graphicsLayer { scaleX = scale.value; scaleY = scale.value }
}

// MARK: - Code Mode row

@Composable
internal fun McpScriptRow(meta: McpScriptMeta) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    var tick by remember { mutableIntStateOf(0) }
    val serversLabel = "${meta.servers.size} ${if (meta.servers.size == 1) "server" else "servers"}"
    val callsLabel = "${meta.calls.size} ${if (meta.calls.size == 1) "call" else "calls"}"
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)),
    ) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 44.dp)
                .clickable(role = Role.Button) { open = !open; tick++ }
                .semantics { contentDescription = "Ran a script, $serversLabel, $callsLabel. ${if (open) "Collapse" else "Expand"}" }
                .padding(horizontal = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_code), null, tint = c.textSecondary, modifier = Modifier.size(20.dp).mcpPop(tick))
            Text("Ran a script", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            Text("· $serversLabel · $callsLabel", color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Icon(painterResource(if (open) DesignR.drawable.ph_caret_up else DesignR.drawable.ph_caret_down), null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
        }
        if (open) Column(Modifier.padding(start = 12.dp, end = 12.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            McpLabel("Script")
            Text(
                meta.script, color = c.textPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace, lineHeight = 17.sp,
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).horizontalScroll(rememberScrollState()).padding(10.dp),
            )
            McpLabel("Calls")
            meta.calls.forEach { call ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("${call.server}.${call.tool}", color = c.textPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text(if (call.ok) (call.ms?.let { "$it ms" } ?: "") else (call.error ?: "failed"), color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                }
            }
            McpLabel("Result")
            McpValueCard(meta.result)
        }
    }
}

@Composable
private fun McpLabel(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
}

/** Objects as key-value rows, arrays of objects as a small table, anything else as text. */
@Composable
private fun McpValueCard(value: JsonElement?) {
    val c = AllternitTheme.colors
    val rows = (value as? JsonArray)?.mapNotNull { it as? JsonObject }?.takeIf { it.isNotEmpty() && it.size == (value as JsonArray).size }
    when {
        rows != null -> {
            val cols = rows.flatMap { it.keys }.distinct().take(4)
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    cols.forEach { Text(it, color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f), maxLines = 1) }
                }
                rows.forEach { r ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        cols.forEach { k -> Text(r.text(k) ?: "", color = c.textPrimary, fontSize = 12.sp, modifier = Modifier.weight(1f), maxLines = 2, overflow = TextOverflow.Ellipsis) }
                    }
                }
            }
        }
        value is JsonObject -> McpKeyValues(value)
        else -> Text(
            (value?.takeIf { it !is JsonNull })?.let { if (it is JsonPrimitive) it.content else it.toString() } ?: "",
            color = c.textPrimary, fontSize = 12.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).padding(10.dp),
        )
    }
}

private fun keyLabel(k: String): String = k.replace(Regex("([a-z])([A-Z])"), "$1 $2").replace('_', ' ').replaceFirstChar { it.uppercase() }

@Composable
private fun McpKeyValues(o: JsonObject) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).padding(10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        o.entries.forEach { (k, v) ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(keyLabel(k), color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.widthIn(max = 120.dp))
                Text(if (v is JsonPrimitive) v.content else v.toString(), color = c.textPrimary, fontSize = 12.sp, modifier = Modifier.weight(1f), maxLines = 3, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

// MARK: - Typed result

/** `POST mcp/connectors/:id/feedback {tool, message?}`; 422 `feedback_unsupported` when the server doesn't declare it. */
internal suspend fun ChatBackend.reportMcpProblem(server: String, tool: String, message: String? = null): ApiResult<Unit> {
    val body = buildJsonObject {
        put("tool", tool)
        if (!message.isNullOrBlank()) put("message", message)
    }.toString()
    return ApiCall.executeUnit(authedHttp, mcpJson, ApiCall.request("POST", mcpUrl("connectors", server, "feedback"), body))
}

@Composable
internal fun McpTypedResultRow(call: ToolCallRecord, meta: McpTypedMeta, backend: ChatBackend) {
    val structured = meta.structured ?: return
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var raw by remember { mutableStateOf(false) }
    var sent by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var tick by remember { mutableIntStateOf(0) }
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(DesignR.drawable.ph_table), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
            Text("Got a ${keyLabel(meta.tool).lowercase()}", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            listOf("Table", "Raw").forEach { t ->
                val on = (t == "Raw") == raw
                Text(
                    t, color = c.textPrimary, fontSize = 12.sp,
                    modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).clickable(role = Role.Tab) { raw = t == "Raw" }
                        .padding(vertical = 10.dp)
                        .background(if (on) c.fillNeutral else androidx.compose.ui.graphics.Color.Transparent, CircleShape)
                        .border(BorderStroke(1.dp, c.borderSubtle), CircleShape)
                        .padding(horizontal = 10.dp, vertical = 3.dp),
                )
            }
        }
        if (raw) Text(
            structured.toString(), color = c.textPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).horizontalScroll(rememberScrollState()).padding(10.dp),
        ) else McpKeyValues(structured)
        if (meta.canReport) Row(
            Modifier.heightIn(min = 44.dp)
                .clickable(enabled = !sent, role = Role.Button) {
                    scope.launch {
                        failed = false
                        if (backend.reportMcpProblem(meta.connectorId ?: meta.server, meta.tool) is ApiResult.Ok) { sent = true; tick++ } else failed = true
                    }
                },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Icon(painterResource(DesignR.drawable.ph_flag), null, tint = c.textSecondary, modifier = Modifier.size(16.dp).mcpPop(tick))
            Text(
                when { sent -> "Thanks, sent to the server"; failed -> "Couldn't send. Try again."; else -> "Report a problem to this server" },
                color = c.textSecondary, fontSize = 12.5.sp, textDecoration = androidx.compose.ui.text.style.TextDecoration.Underline,
            )
        }
    }
}
