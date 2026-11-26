@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package com.allternit.feature.shell

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

private data class Pick(val id: String, val label: String)

private fun items(e: JsonElement, key: String, label: (JsonObject) -> String?, id: (JsonObject) -> String?): List<Pick> =
    (((e as? JsonObject)?.get(key) as? JsonArray) ?: (e as? JsonArray)).orEmpty().mapNotNull { it as? JsonObject }
        .mapNotNull { o -> id(o)?.let { Pick(it, label(o) ?: it) } }

private fun JsonObject.s(k: String) = (this[k] as? JsonPrimitive)?.contentOrNull

/** "Watch an app event" (iOS McpWatchAppEventSheet): pick a connected app, then one of its events, then the watch starts. */
@Composable
internal fun WatchEventSheet(sessionId: String?, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var connectors by remember { mutableStateOf<List<Pick>>(emptyList()) }
    var picked by remember { mutableStateOf<Pick?>(null) }
    var events by remember { mutableStateOf<List<Pick>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var message by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        (LibraryNet.get("mcp/connectors") as? ApiResult.Ok)?.let { r -> connectors = items(r.value, "connectors", { it.s("name") }, { it.s("id") }) }
        loading = false
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Watch an app event", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 8.dp))
            if (sessionId == null) Text("Send a message first, then watch an event from this chat.", color = c.textSecondary, fontSize = 14.sp)
            else if (picked != null) {
                val p = picked!!
                Text(p.label, color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                if (events.isEmpty()) Text(if (loading) "Loading…" else "This app offers no events.", color = c.textSecondary, fontSize = 14.sp)
                events.forEach { e ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(role = Role.Button) {
                        scope.launch {
                            val r = LibraryNet.send("POST", "mcp/watches", buildJsonObject { put("server", p.id); put("event", e.id); put("session_id", sessionId); put("delivery", "poll") })
                            if (r is ApiResult.Ok) onDismiss() else message = "Couldn't start this watch. Try again."
                        }
                    }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Icon(painterResource(DesignR.drawable.ph_broadcast), null, tint = c.textPrimary, modifier = Modifier.size(20.dp))
                        Text(e.label, color = c.textPrimary, fontSize = 15.sp)
                    }
                }
            } else {
                if (connectors.isEmpty()) Text(if (loading) "Loading…" else "No connected apps.", color = c.textSecondary, fontSize = 14.sp)
                connectors.forEach { k ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(role = Role.Button) {
                        picked = k; loading = true; message = null
                        scope.launch {
                            (LibraryNet.get("mcp/connectors/${LibraryNet.escape(k.id)}/events") as? ApiResult.Ok)?.let { r -> events = items(r.value, "events", { it.s("title") ?: it.s("name") }, { it.s("name") }) }
                            loading = false
                        }
                    }, verticalAlignment = Alignment.CenterVertically) {
                        Text(k.label, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                        Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                    }
                }
            }
            message?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
        }
    }
}
