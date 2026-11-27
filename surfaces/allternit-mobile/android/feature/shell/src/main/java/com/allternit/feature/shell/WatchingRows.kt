package com.allternit.feature.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.chat.mcpPop
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.longOrNull

/** MCP 2027 UI item 6: one row per watch this chat holds (`GET mcp/watches?session_id=`), above the composer. */
private data class WatchRow(
    val id: String,
    val server: String,
    val event: String,
    val mode: String,
    val pollMs: Long?,
    val tail: List<String>,
    val lineCount: Int,
    val wakes: Boolean,
    val lastWokeAt: String? = null,
    val lastWakeError: String? = null,
)

private fun JsonObject.s(k: String) = (this[k] as? JsonPrimitive)?.contentOrNull

private fun wokeText(iso: String?): String? {
    val ms = iso?.takeIf { it.isNotBlank() }?.let { runCatching { java.time.Instant.parse(it).toEpochMilli() }.getOrNull() } ?: return null
    val rel = android.text.format.DateUtils.getRelativeTimeSpanString(ms, System.currentTimeMillis(), android.text.format.DateUtils.MINUTE_IN_MILLIS)
    return "Woke this chat $rel"
}

private fun parseWatches(e: JsonElement): List<WatchRow> {
    val list = ((e as? JsonObject)?.get("watches") as? JsonArray) ?: (e as? JsonArray) ?: return emptyList()
    return list.mapNotNull { it as? JsonObject }.mapNotNull { o ->
        val id = o.s("watchId") ?: o.s("watch_id") ?: o.s("id") ?: return@mapNotNull null
        WatchRow(
            id = id, server = o.s("server") ?: "", event = o.s("event") ?: "",
            mode = o.s("mode") ?: "poll",
            pollMs = (o["pollMs"] as? JsonPrimitive)?.longOrNull,
            tail = ((o["tail"] as? JsonArray).orEmpty()).mapNotNull { (it as? JsonPrimitive)?.contentOrNull }.takeLast(3),
            lineCount = (o["lineCount"] as? JsonPrimitive)?.intOrNull ?: 0,
            wakes = (o["wakes"] as? JsonPrimitive)?.contentOrNull != "false",
            lastWokeAt = o.s("lastWokeAt"),
            lastWakeError = o.s("lastWakeError"),
        )
    }
}

private data class PausedWatch(val key: String, val server: String, val event: String, val eventCount: Int)

private val pausedStore = androidx.compose.runtime.mutableStateMapOf<String, List<PausedWatch>>()

@Composable
internal fun WatchingRows(sessionId: String?, modifier: Modifier = Modifier) {
    if (sessionId == null) return
    val paused = pausedStore[sessionId].orEmpty()
    var watches by remember(sessionId) { mutableStateOf<List<WatchRow>>(emptyList()) }
    var refresh by remember(sessionId) { mutableIntStateOf(0) }
    val anyLive = watches.any { it.mode == "live" }
    // Live rows refresh every 2 s; the rest every 15 s (the server holds a 2 h lease, so the list is re-read, not trusted).
    LaunchedEffect(sessionId, refresh, anyLive) {
        while (true) {
            val r = LibraryNet.get("mcp/watches?session_id=${LibraryNet.escape(sessionId)}")
            if (r is ApiResult.Ok) watches = parseWatches(r.value)
            delay(if (anyLive) 2_000 else 15_000)
        }
    }
    if (watches.isEmpty() && paused.isEmpty()) return
    Column(modifier.fillMaxWidth().padding(bottom = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        watches.forEach { w ->
            WatchingRow(sessionId, w, onChanged = { refresh++ }, onPaused = { pausedStore[sessionId] = paused + PausedWatch(w.id, w.server, w.event, w.lineCount) })
        }
        paused.forEach { p -> PausedRow(sessionId, p, paused, onChanged = { refresh++ }) }
    }
}

@Composable
private fun WatchingRow(sessionId: String, w: WatchRow, onChanged: () -> Unit, onPaused: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var all by remember(w.id) { mutableStateOf<List<String>?>(null) }
    var tick by remember(w.id) { mutableIntStateOf(0) }
    var busy by remember(w.id) { mutableStateOf(false) }
    val icon = when (w.mode) { "live" -> DesignR.drawable.ph_broadcast; "webhook" -> DesignR.drawable.ph_bell; else -> DesignR.drawable.ph_clock }
    val label = when (w.mode) {
        "live" -> "Live"
        "webhook" -> "Wakes this chat when it fires"
        else -> "Checks every ${((w.pollMs ?: 30_000L) / 1000).coerceAtLeast(1)} s while open"
    }
    // One short pulse each time a new line lands.
    LaunchedEffect(w.lineCount) { if (w.lineCount > 0) tick++ }
    fun act(method: String, path: String, after: () -> Unit = {}) {
        scope.launch {
            busy = true
            LibraryNet.send(method, path)
            busy = false
            after(); onChanged()
        }
    }
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(icon), null, tint = c.textSecondary, modifier = Modifier.size(20.dp).mcpPop(tick))
            Column(Modifier.weight(1f)) {
                Text(w.event.ifBlank { w.server }, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(label, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (w.mode == "webhook") {
                    wokeText(w.lastWokeAt)?.let { Text(it, color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                    w.lastWakeError?.takeIf { it.isNotBlank() }?.let { Text(it, color = AllternitStatic.StatusWarning, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                }
            }
            if (w.mode == "live") WatchAction("Pause", DesignR.drawable.ph_pause_fill, !busy) { act("DELETE", "mcp/watches/${LibraryNet.escape(w.id)}", onPaused) }
            WatchAction("Stop", DesignR.drawable.ph_stop_fill, !busy) { act("DELETE", "mcp/watches/${LibraryNet.escape(w.id)}") }
        }
        val lines = all ?: w.tail
        if (lines.isNotEmpty()) Text(
            lines.joinToString("\n"), color = c.textPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace, lineHeight = 16.sp,
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).padding(8.dp),
        )
        if (w.lineCount > w.tail.size || all != null) Text(
            if (all == null) "Open all ${w.lineCount} lines" else "Show less",
            color = c.textSecondary, fontSize = 12.sp,
            modifier = Modifier.heightIn(min = 44.dp).clickable(role = Role.Button) {
                if (all != null) all = null else scope.launch {
                    val r = LibraryNet.get("mcp/watches/${LibraryNet.escape(w.id)}/lines?after=0")
                    if (r is ApiResult.Ok) {
                        all = ((r.value as? JsonObject)?.get("lines") as? JsonArray).orEmpty().mapNotNull { (it as? JsonObject)?.s("text") }
                    }
                }
            }.padding(vertical = 12.dp),
        )
    }
}

/** Pause = DELETE on the server and keep the request here; Resume = POST again (a new watch id), as on web and iOS. */
@Composable
private fun PausedRow(sessionId: String, p: PausedWatch, all: List<PausedWatch>, onChanged: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var busy by remember(p.key) { mutableStateOf(false) }
    var failed by remember(p.key) { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Icon(painterResource(DesignR.drawable.ph_pause_fill), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
        Column(Modifier.weight(1f)) {
            Text("Watching ${p.event}", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(if (failed) "Couldn't resume this watch. Try again." else "${p.server} · Paused · ${p.eventCount} events", color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        WatchAction("Resume", DesignR.drawable.ph_play_fill, !busy) {
            scope.launch {
                busy = true; failed = false
                val r = LibraryNet.send("POST", "mcp/watches", buildJsonObject { put("server", p.server); put("event", p.event); put("session_id", sessionId); put("delivery", "poll") })
                if (r is ApiResult.Ok) { pausedStore[sessionId] = all.filter { it.key != p.key }; onChanged() } else failed = true
                busy = false
            }
        }
        WatchAction("Stop", DesignR.drawable.ph_stop_fill, !busy) { pausedStore[sessionId] = all.filter { it.key != p.key } }
    }
}

@Composable
private fun WatchAction(title: String, icon: Int, enabled: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.heightIn(min = 44.dp).widthIn(min = 44.dp).clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Icon(painterResource(icon), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
        Text(title, color = c.textSecondary, fontSize = 12.sp)
    }
}
