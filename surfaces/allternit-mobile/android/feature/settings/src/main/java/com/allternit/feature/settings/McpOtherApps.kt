package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.chat.mcpPop
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

private const val MCP_URL = "https://mcp.allternit.com"

private data class ConnectedApp(val client: String, val name: String, val lastUsed: String?, val ids: List<String>)

private fun parseApps(e: JsonElement): List<ConnectedApp> {
    val rows = ((e as? JsonObject)?.let { it["approvals"] ?: it["items"] } as? JsonArray) ?: (e as? JsonArray) ?: return emptyList()
    fun JsonObject.s(k: String) = (this[k] as? JsonPrimitive)?.contentOrNull
    return rows.mapNotNull { it as? JsonObject }.filter { it.s("id") != null }
        .groupBy { it.s("client") ?: it.s("clientId") ?: "" }
        .map { (client, rs) ->
            ConnectedApp(client, rs.firstNotNullOfOrNull { it.s("clientName") } ?: client, rs.mapNotNull { it.s("lastUsedAt") }.maxOrNull(), rs.mapNotNull { it.s("id") })
        }
}

/** MCP 2027 UI item 7: shown on the Connectors screen (Android has no MCP settings screen). */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
internal fun UseAllternitInOtherApps() {
    val c = AllternitTheme.colors
    val clipboard = LocalClipboardManager.current
    val scope = rememberCoroutineScope()
    var copied by remember { mutableStateOf(false) }
    var tick by remember { mutableIntStateOf(0) }
    var error by remember { mutableStateOf<String?>(null) }
    val (load, reload) = rememberLoad("mcpApprovals") { Net.cloud("GET", "api/v1/mcp/approvals").toLoad("load connected apps") }

    Text("Use Allternit in other apps", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 6.dp, vertical = 6.dp))
    CCard {
        Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(MCP_URL, color = c.textPrimary, fontSize = 13.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
            SButton(if (copied) "Copied" else "Copy", tag = "mcpCopyUrl") {
                clipboard.setText(AnnotatedString(MCP_URL)); copied = true; tick++
            }
            Icon(painterResource(if (copied) DesignR.drawable.ph_check else DesignR.drawable.ph_copy), null, tint = c.textSecondary, modifier = Modifier.size(16.dp).mcpPop(tick))
        }
        FlowRow(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("Claude", "ChatGPT", "Cursor", "Gizzi").forEach { Text(it, color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.heightIn(min = 28.dp).padding(horizontal = 4.dp)) }
        }
        Text("Settings, Connectors, Add custom connector. Paste the URL and sign in.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(12.dp))
    }
    LoadView(load, reload, "load connected apps") { body ->
        val apps = parseApps(body)
        if (apps.isNotEmpty()) CCard {
            apps.forEachIndexed { i, a ->
                if (i > 0) SDivider()
                SRow(a.name, "mcpApp_${a.client}", detail = a.lastUsed?.let { "Last used $it" }, chevron = false, onClick = {}, trailing = {
                    Row(Modifier.heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        Icon(painterResource(DesignR.drawable.ph_link_break), null, tint = Danger, modifier = Modifier.size(16.dp))
                        SButton("Disconnect", tag = "mcpDisconnect_${a.client}", destructive = true) {
                            scope.launch {
                                error = null
                                val failed = a.ids.map { Net.cloud("DELETE", "api/v1/mcp/approvals/${LibraryNet.escape(it)}") }.firstOrNull { it !is ApiResult.Ok }
                                if (failed != null) error = failed.friendly("disconnect") else reload()
                            }
                        }
                    }
                })
            }
        }
    }
    SError(error)
}
