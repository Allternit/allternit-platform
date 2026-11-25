package com.allternit.feature.bots

import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

// -----------------------------------------------------------------------------
// The memory map (iOS Features/Shared/ImpactMapView.swift, SPEC-hermes-parity
// section 4). On a phone the groups stack top to bottom with the focus card in
// the middle: Could change, Used by, the focus card, Uses. Tap a box to move
// focus. Forget (memory) and Turn off (skill) name what could change first.
// -----------------------------------------------------------------------------

private fun impactHex(kind: String) = when (kind) {
    "skill" -> "#2457c5"; "memory" -> "#0d8064"; "thread" -> "#7a48c8"; "routine" -> "#c4461b"; "artifact" -> "#b8285c"; "soul" -> "#9a6a06"; "entity" -> "#0e7490"; else -> "#66778a"
}

private fun impactTitle(kind: String) = when (kind) {
    "skill" -> "Skill"; "memory" -> "Memory"; "thread" -> "Thread"; "routine" -> "Routine"; "artifact" -> "Artifact"; "soul" -> "Soul"; "entity" -> "Entity"; else -> kind.replaceFirstChar { it.uppercase() }
}

/** One map item (`GET memory-map` MemoryMapItem); [ref] is what Open and Forget use. */
data class MapItem(val id: String, val kind: String, val label: String, val ref: String?, val uses: Int?, val lastAt: String?)

/** iOS ImpactGraph. Direction: a link's `from` depends on its `to`. */
internal class ImpactGraph(val items: List<MapItem>, links: List<Pair<String, String>>) {
    private val byId = items.associateBy { it.id }
    private val usesOf = mutableMapOf<String, MutableList<String>>()
    private val usedByOf = mutableMapOf<String, MutableList<String>>()

    init {
        val seen = mutableSetOf<String>()
        links.filter { (f, t) -> byId.containsKey(f) && byId.containsKey(t) && f != t }.forEach { (f, t) ->
            if (seen.add("$f>$t")) {
                usesOf.getOrPut(f) { mutableListOf() }.add(t)
                usedByOf.getOrPut(t) { mutableListOf() }.add(f)
            }
        }
    }

    fun item(id: String) = byId[id]

    private fun sorted(ids: List<String>) = ids.mapNotNull { byId[it] }.sortedWith(compareByDescending<MapItem> { it.uses ?: 0 }.thenBy { it.label.lowercase() })

    fun uses(id: String) = sorted(usesOf[id].orEmpty())
    fun usedBy(id: String) = sorted(usedByOf[id].orEmpty())

    /** Everything that depends on this item two or more hops away. */
    fun couldChange(id: String): List<MapItem> {
        val direct = usedByOf[id].orEmpty()
        val seen = (direct + id).toMutableSet()
        var frontier = direct
        val far = mutableListOf<String>()
        while (frontier.isNotEmpty()) {
            val next = mutableListOf<String>()
            frontier.forEach { node -> usedByOf[node].orEmpty().forEach { parent -> if (seen.add(parent)) { far.add(parent); next.add(parent) } } }
            frontier = next
        }
        return sorted(far)
    }

    /** Opens on something connected: most recently used, else most used, else the first. */
    val defaultFocus: MapItem?
        get() {
            val linked = items.filter { usesOf[it.id].orEmpty().isNotEmpty() || usedByOf[it.id].orEmpty().isNotEmpty() }
            val pool = linked.ifEmpty { items }
            return pool.filter { parseTime(it.lastAt) > 0L }.maxByOrNull { parseTime(it.lastAt) }
                ?: pool.maxByOrNull { it.uses ?: 0 }?.takeIf { (it.uses ?: 0) > 0 } ?: pool.firstOrNull()
        }
}

/** What Open does for a map item inside a bot (iOS BotMapOpen), or null when there is nothing to open. */
internal fun impactOpen(bot: BotRecord, nav: BotNavigator, item: MapItem): (() -> Unit)? = when (item.kind) {
    "thread" -> item.ref?.takeIf { it.isNotEmpty() }?.let { ref -> { nav.open(BotRoute.Chat(bot.id, ref)) } }
    "routine" -> ({ nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Routines)) })
    "artifact" -> ({ nav.open(BotRoute.BotHome(bot.id, BotHomeTab.Artifacts)) })
    "skill", "soul" -> ({ nav.open(BotRoute.Settings(bot.id)) })
    else -> null
}

private const val GROUP_LIMIT = 5
private val SKILL_ROOTS = listOf("~/.allternit/skills", "~/.agents/skills", "~/.codex/skills")

/** iOS CapabilitiesClient.scanSkills + setEnabled(false), cut to finding one skill by name. */
private suspend fun turnOffSkill(backend: ChatBackend, name: String): String? {
    suspend fun list(path: String): List<JsonObject>? = (backend.runtimeJson("GET", "/api/v1/files/list?path=${BotsApi.esc(path)}&details=true") as? RuntimeCall.Ok)
        ?.body?.obj?.get("entries")?.let { it as? JsonArray }?.mapNotNull { it as? JsonObject }
    suspend fun read(path: String): String? = ((backend.runtimeJson("GET", "/api/v1/files/read?path=${BotsApi.esc(path)}") as? RuntimeCall.Ok)?.body?.obj)?.str("content")
    var anyListed = false
    for (root in SKILL_ROOTS) {
        val queue = ArrayDeque(listOf(root to 0))
        while (queue.isNotEmpty()) {
            val (dir, depth) = queue.removeFirst()
            val entries = list(dir) ?: continue
            anyListed = true
            if (entries.any { it.str("type") != "directory" && it.str("name")?.lowercase() == "skill.md" }) {
                val configText = if (entries.any { it.str("name") == "config.json" }) read("$dir/config.json") else null
                val config = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(configText ?: "{}") as? JsonObject }.getOrNull() ?: JsonObject(emptyMap())
                val skillName = config.str("name") ?: Regex("(?m)^name:\\s*(.+)$").find(read("$dir/SKILL.md").orEmpty())?.groupValues?.get(1)?.trim()?.trim('"') ?: dir.substringAfterLast('/')
                if (!skillName.equals(name, ignoreCase = true)) continue
                val next = JsonObject(config + mapOf("enabled" to JsonPrimitive(false), "updatedAt" to JsonPrimitive(java.time.Instant.now().toString())))
                val body = buildJsonObject { put("path", "$dir/config.json"); put("content", next.toString()) }.toString()
                return when (val r = backend.runtimeJson("POST", "/api/v1/files/write", body)) {
                    is RuntimeCall.Ok -> null
                    RuntimeCall.NoComputer -> "Connect a computer to turn off skills."
                    is RuntimeCall.Http -> r.message ?: "HTTP ${r.status}"
                    is RuntimeCall.Failed -> r.message
                }
            }
            if (depth < 4) entries.filter { it.str("type") == "directory" && it.str("name")?.startsWith(".") == false && it.str("name") != "node_modules" }
                .forEach { e -> e.str("path")?.let { queue.addLast(it to depth + 1) } }
        }
    }
    return if (anyListed) "That skill is not on this computer." else "Connect a computer to turn off skills."
}

/**
 * What a bot knows and what depends on what (iOS ImpactMapView): the focus card with its counts,
 * Could change and Used by above it, Uses below; tap any box to move focus there.
 */
@Composable
internal fun ImpactMapView(
    botId: String?,
    backend: ChatBackend? = null,
    openAction: (MapItem) -> (() -> Unit)? = { null },
    onChanged: () -> Unit = {},
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var graph by remember { mutableStateOf(ImpactGraph(emptyList(), emptyList())) }
    var truncated by remember { mutableStateOf(false) }
    var loading by remember { mutableStateOf(true) }
    var loaded by remember { mutableStateOf(false) }
    var unavailable by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var focusId by remember { mutableStateOf<String?>(null) }
    val expanded = remember { mutableStateListOf<String>() }
    var confirming by remember { mutableStateOf<MapItem?>(null) }
    var busy by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    suspend fun load() {
        loading = true
        when (val r = LibraryNet.get(if (botId != null) "memory-map?bot=${BotsApi.esc(botId)}" else "memory-map")) {
            is ApiResult.Ok -> {
                val o = r.value.obj
                val items = (o?.get("items") as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.mapNotNull { m ->
                    MapItem(m.str("id") ?: return@mapNotNull null, m.str("kind") ?: "item", m.str("label").orEmpty(), m.str("ref"), m.num("uses")?.toInt(), m.str("last_at") ?: m.str("lastAt"))
                }
                val links = (o?.get("links") as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }.mapNotNull { l -> (l.str("from") ?: return@mapNotNull null) to (l.str("to") ?: return@mapNotNull null) }
                graph = ImpactGraph(items, links)
                truncated = (o?.get("truncated") as? JsonPrimitive)?.contentOrNull == "true"
                if (focusId != null && graph.item(focusId!!) == null) focusId = null
                error = null
                loaded = true
            }
            is ApiResult.HttpError -> if (r.status == 404) unavailable = true else error = r.friendly("load the map")
            else -> error = r.friendly("load the map")
        }
        loading = false
    }
    LaunchedEffect(botId) { load() }
    // A computer on an older runtime has no map route yet: show nothing.
    if (unavailable) return
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).testTag("impactMap"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("Memory map", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f).semantics { heading() })
            if (loaded && graph.items.isNotEmpty()) Text("${graph.items.size} items", color = c.textTertiary, fontSize = 12.sp)
        }
        val card = Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp))
        when {
            loading && !loaded -> Row(card.heightIn(min = 96.dp).padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally)) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                Text("Mapping what your bots know", color = c.textSecondary, fontSize = 13.sp)
            }
            error != null && !loaded -> Column(card.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Couldn't load the map", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                Text(error.orEmpty(), color = c.textSecondary, fontSize = 12.5.sp)
                Pill("Retry", "impactRetry") { scope.launch { load() } }
            }
            graph.items.isEmpty() -> Column(card.padding(18.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                com.allternit.core.design.EmptyArt(com.allternit.core.design.EmptyArtKind.Sheets, size = 72.dp)
                Text("Nothing to map yet", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                Text("As your bots learn things, use skills and run routines, you will see here what depends on what.", color = c.textSecondary, fontSize = 12.5.sp)
            }
            else -> {
                val focus = focusId?.let { graph.item(it) } ?: graph.defaultFocus
                if (focus != null) {
                    val usedBy = graph.usedBy(focus.id)
                    val far = graph.couldChange(focus.id)
                    val uses = graph.uses(focus.id)
                    Column(Modifier.fillMaxWidth().animateContentSize(tween(180)), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        if (far.isNotEmpty()) ImpactGroup("Could change", far, "far", expanded) { focusId = it.id }
                        if (usedBy.isNotEmpty()) ImpactGroup("Used by", usedBy, "by", expanded) { focusId = it.id }
                        FocusCard(focus, usedBy.size, far.size, uses.size, openAction(focus), canAct = focus.kind == "memory" || (focus.kind == "skill" && backend != null), busy = busy) { confirming = focus }
                        if (uses.isNotEmpty()) ImpactGroup("Uses", uses, "uses", expanded) { focusId = it.id }
                        if (usedBy.isEmpty() && uses.isEmpty()) Text("Nothing is linked to this yet.", color = c.textTertiary, fontSize = 12.5.sp)
                        if (truncated) Text("Showing the first 500 items.", color = c.textTertiary, fontSize = 11.5.sp)
                        actionError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
                    }
                }
            }
        }
    }
    confirming?.let { item ->
        val affected = graph.usedBy(item.id) + graph.couldChange(item.id)
        val verb = if (item.kind == "skill") "Turning it off" else "Forgetting it"
        val names = affected.take(4).joinToString(", ") { it.label } + if (affected.size > 4) " and ${affected.size - 4} more" else ""
        ConfirmSheet(
            if (item.kind == "skill") "Turn off ${item.label}?" else "Forget this?",
            if (affected.isEmpty()) "$verb does not change anything else on the map." else "$verb could change: $names.",
            if (item.kind == "skill") "Turn off" else "Forget", "impactConfirm", onDismiss = { confirming = null },
        ) {
            confirming = null
            busy = true
            actionError = null
            scope.launch {
                val problem: String? = if (item.kind == "skill") {
                    backend?.let { turnOffSkill(it, item.label) } ?: "Connect a computer to turn off skills."
                } else {
                    var id = item.ref ?: item.id
                    id = id.removePrefix("memory:")
                    val r = if (id.startsWith("fact:")) BotsApi.call("DELETE", "memory/v2/facts/${BotsApi.esc(id.removePrefix("fact:"))}", null, "forget that") else BotsApi.forgetMemory(id)
                    (r as? BotsResult.Failed)?.message
                }
                if (problem != null) actionError = "Couldn't do that: $problem" else { focusId = null; load(); onChanged() }
                busy = false
            }
        }
    }
}

@Composable
private fun ImpactGroup(title: String, items: List<MapItem>, key: String, expanded: MutableList<String>, onFocus: (MapItem) -> Unit) {
    val c = AllternitTheme.colors
    val open = key in expanded
    val shown = if (open) items else items.take(GROUP_LIMIT)
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("${title.uppercase()} · ${items.size}", color = c.textTertiary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
        shown.forEach { item ->
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp))
                    .clickable { onFocus(item) }.semantics { contentDescription = "${impactTitle(item.kind)}: ${item.label}" }.testTag("impactItem${item.id}").padding(horizontal = 12.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Box(Modifier.width(4.dp).heightIn(min = 22.dp, max = 22.dp).clip(RoundedCornerShape(2.dp)).background(hexColor(impactHex(item.kind))))
                Text(item.label, color = c.textPrimary, fontSize = 14.sp, maxLines = 2, modifier = Modifier.weight(1f))
                Text(impactTitle(item.kind), color = hexColor(impactHex(item.kind)), fontSize = 11.sp)
            }
        }
        if (items.size > GROUP_LIMIT) {
            Text(
                if (open) "Show less" else "+${items.size - GROUP_LIMIT} more", color = c.textSecondary, fontSize = 13.sp,
                modifier = Modifier.fillMaxWidth().heightIn(min = 36.dp).clickable { if (open) expanded.remove(key) else expanded.add(key) }.padding(vertical = 8.dp).testTag("impactMore$key"),
            )
        }
    }
}

@Composable
private fun FocusCard(item: MapItem, usedBy: Int, far: Int, uses: Int, open: (() -> Unit)?, canAct: Boolean, busy: Boolean, onAct: () -> Unit) {
    val c = AllternitTheme.colors
    val tint = hexColor(impactHex(item.kind))
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(tint.copy(alpha = 0.08f)).border(BorderStroke(1.5.dp, tint.copy(alpha = 0.55f)), RoundedCornerShape(16.dp)).testTag("impactFocus").padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(impactTitle(item.kind).uppercase(), color = tint, fontSize = 11.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            item.lastAt?.takeIf { it.isNotEmpty() && botTimeShort(it).isNotEmpty() }?.let { Text("Used ${botTimeShort(it)} ago", color = c.textTertiary, fontSize = 11.sp) }
        }
        Text(item.label, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
        Row(horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            Stat(uses, "uses")
            Stat(usedBy, "used by")
            Stat(far, "could change")
            item.uses?.let { Stat(it, if (it == 1) "time used" else "times used") }
        }
        if (open != null || canAct) Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (open != null) Pill("Open", "impactOpen", enabled = !busy) { open() }
            if (canAct) Pill(if (item.kind == "skill") "Turn off" else "Forget", "impactAct", enabled = !busy, busy = busy) { onAct() }
        }
    }
}

@Composable
private fun Stat(value: Int, label: String) {
    val c = AllternitTheme.colors
    Column(Modifier.semantics { contentDescription = "$value $label" }) {
        Text("$value", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        Text(label, color = c.textSecondary, fontSize = 11.sp)
    }
}
