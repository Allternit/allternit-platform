package com.allternit.feature.bots

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** One message between two bots (`GET /api/factory/deliveries`, iOS FactoryDelivery). `from` and `preview` come with platform #3052; older records have neither. */
data class FactoryDelivery(val id: String, val from: String?, val to: String?, val preview: String?, val at: java.time.Instant?)

private fun JsonObject.text(vararg keys: String): String? = keys.firstNotNullOfOrNull { k -> str(k)?.takeIf { it.isNotEmpty() } }

/** iOS BotProjectClient.deliveries: oldest first; a bare array or `{deliveries: [...]}`. */
internal suspend fun ChatBackend.factoryDeliveries(): BotsResult<List<FactoryDelivery>> =
    when (val r = runtimeResult("GET", "/api/factory/deliveries", doing = "load team messages")) {
        is BotsResult.Failed -> r
        is BotsResult.Ok -> {
            val body: JsonElement = r.value
            val items = (body as? JsonArray) ?: (body as? JsonObject)?.get("deliveries") as? JsonArray
            val list = items.orEmpty().mapIndexedNotNull { i, e ->
                val o = e as? JsonObject ?: return@mapIndexedNotNull null
                FactoryDelivery(
                    id = o.text("id") ?: "delivery$i",
                    from = o.text("from", "sender"), to = o.text("to", "receiver"), preview = o.text("preview"),
                    at = o.text("at", "createdAt")?.let { runCatching { java.time.Instant.parse(it) }.getOrNull() },
                )
            }.sortedBy { it.at ?: java.time.Instant.EPOCH }
            BotsResult.Ok(list)
        }
    }

private fun deliveryTime(d: FactoryDelivery): String = d.at?.let {
    java.time.format.DateTimeFormatter.ofLocalizedDateTime(java.time.format.FormatStyle.MEDIUM, java.time.format.FormatStyle.SHORT).withZone(java.time.ZoneId.systemDefault()).format(it)
} ?: "time not saved"

/** iOS ProjectTeamMessages: step back and forward through the deliveries between bots, or stay Live on the latest; a tap on the row opens from → to, time and the preview. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TeamReplayScreen(store: BotModeStore, backend: ChatBackend, nav: BotNavigator) {
    val c = AllternitTheme.colors
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var deliveries by remember { mutableStateOf<List<FactoryDelivery>>(emptyList()) }
    var index by remember { mutableStateOf<Int?>(null) } // null = Live (the newest message)
    var opened by remember { mutableStateOf<FactoryDelivery?>(null) }
    LaunchedEffect(Unit) {
        when (val r = backend.factoryDeliveries()) { is BotsResult.Ok -> { deliveries = r.value; error = null }; is BotsResult.Failed -> error = r.message }
        loading = false
    }
    val bots = store.bots
    fun name(id: String?): String = if (id.isNullOrEmpty()) "Unknown" else bots.firstOrNull { it.id == id }?.name ?: id
    val current = if (deliveries.isEmpty()) null else minOf(index ?: (deliveries.size - 1), deliveries.size - 1)

    BotScreen("Team messages", nav, testTag = "teamReplay") {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Messages", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            if (!ListState(loading, error?.let { com.allternit.core.design.FriendlyErrorMessage.from(it) }, false, "Couldn't load team messages", "", null) { }) {
                if (current == null) {
                    Text("Your team is starting", color = c.textTertiary, fontSize = 14.sp)
                    bots.forEach { b -> Text("${b.name} · ${botStateWord(b)}", color = c.textSecondary, fontSize = 13.sp) }
                } else {
                    val message = deliveries[current]
                    Row(Modifier.heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = { index = maxOf(current - 1, 0) }, enabled = current > 0, modifier = Modifier.testTag("teamReplayPrev")) {
                            Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_left), "Previous message")
                        }
                        IconButton(onClick = { index = if (current + 1 >= deliveries.size) null else current + 1 }, enabled = current < deliveries.size - 1, modifier = Modifier.testTag("teamReplayNext")) {
                            Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), "Next message")
                        }
                        TextButton(onClick = { index = null }, enabled = index != null, modifier = Modifier.testTag("teamReplayLive")) { Text("Live") }
                        Spacer(Modifier.weight(1f))
                        Text("${current + 1} of ${deliveries.size} · ${deliveryTime(message)}", color = c.textTertiary, fontSize = 12.sp)
                    }
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        bots.forEach { b ->
                            val role = if (b.id == message.from) "sender" else if (b.id == message.to) "receiver" else null
                            Column(
                                Modifier.clip(RoundedCornerShape(8.dp)).border(BorderStroke(1.dp, if (role == null) androidx.compose.ui.graphics.Color.Transparent else c.borderSubtle), RoundedCornerShape(8.dp)).padding(6.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                            ) {
                                Text(b.name, color = c.textPrimary, fontSize = 11.sp, maxLines = 1)
                                if (role != null) Text(role, color = c.textSecondary, fontSize = 10.sp, fontWeight = FontWeight.SemiBold)
                            }
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable { opened = message }.testTag("teamReplayRow"),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("${name(message.from)} → ${name(message.to)}", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                        Icon(painterResource(com.allternit.core.design.R.drawable.ph_caret_right), "Open message", tint = c.textTertiary)
                    }
                }
            }
        }
    }

    opened?.let { m ->
        ModalBottomSheet(onDismissRequest = { opened = null }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPanel) {
            Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 32.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("${name(m.from)} → ${name(m.to)}", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                Text(deliveryTime(m), color = c.textSecondary, fontSize = 14.sp)
                Text(m.preview?.takeIf { it.isNotEmpty() } ?: "No text was saved for this message", color = c.textPrimary, fontSize = 16.sp)
            }
        }
    }
}

/** A bot's state in words for the empty "Your team is starting" list. */
private fun botStateWord(b: BotRecord): String = b.raw.str("status")?.takeIf { it.isNotEmpty() } ?: "starting"
