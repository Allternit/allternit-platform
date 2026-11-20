package com.allternit.feature.computer

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import kotlin.math.max
import kotlin.math.roundToInt

/** iOS TeachEvent: one thing that happened during a recording, read for the timeline. */
internal data class TeachEvent(val index: Int, val kind: String, val t: Double, val label: String) {
    val time: String get() { val total = max(0, (t / 1000).roundToInt()); return "${total / 60}:${(total % 60).toString().padStart(2, '0')}" }

    companion object {
        fun list(values: List<JsonObject>): List<TeachEvent> = values.mapIndexed { i, raw ->
            fun text(k: String) = raw.str(k)?.takeIf { it.isNotEmpty() }
            val kind = text("kind") ?: "click"
            val label = text("label").orEmpty()
            val words = when (kind) {
                "navigate" -> text("title") ?: text("url")?.let { u -> runCatching { java.net.URI(u) }.getOrNull()?.let { (it.host ?: "") + (if (it.path == "/" || it.path == null) "" else it.path) } ?: u }.orEmpty()
                "input" -> text("valuePreview")?.let { "$label: $it" } ?: label
                "app" -> text("window")?.let { "${text("app").orEmpty()}: $it" } ?: text("app").orEmpty()
                "narration" -> text("text").orEmpty()
                "snapshot" -> "Screen"
                "key" -> text("label")?.let { "${text("key").orEmpty()} in $it" } ?: text("key").orEmpty()
                "select" -> text("value")?.let { "$label: $it" } ?: label
                "upload" -> "$label: ${text("fileName").orEmpty()}"
                "download" -> text("fileName").orEmpty()
                "copy" -> "Copied “${text("textPreview").orEmpty()}”"
                "paste" -> "Pasted into $label"
                else -> label
            }
            TeachEvent(i, kind, (raw["t"] as? JsonPrimitive)?.content?.toDoubleOrNull() ?: 0.0, words)
        }
    }
}

/** iOS TeachTimeline: the recording as wrapping chips with their time. In Review a chip can be struck out so it isn't sent. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
internal fun TeachTimeline(events: List<TeachEvent>, struck: Set<Int> = emptySet(), onToggleStrike: ((Int) -> Unit)? = null) {
    val c = AllternitTheme.colors
    if (events.isEmpty()) { Text("No actions recorded, only network traffic.", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.testTag("aciTeachTimelineEmpty")); return }
    Column(Modifier.heightIn(max = 240.dp).verticalScroll(rememberScrollState()).testTag("aciTeachTimeline")) {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            events.forEach { e ->
                val isStruck = e.index in struck
                Row(
                    Modifier.alpha(if (isStruck) 0.45f else 1f).heightIn(min = 28.dp).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(8.dp)).padding(start = 7.dp, end = if (onToggleStrike == null) 7.dp else 0.dp),
                    verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp),
                ) {
                    Text(e.label, color = c.textSecondary, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, textDecoration = if (isStruck) TextDecoration.LineThrough else null, modifier = Modifier.widthIn(max = 160.dp))
                    Text(e.time, color = c.textTertiary, fontSize = 10.sp, fontFamily = FontFamily.Monospace)
                    if (onToggleStrike != null && e.kind != "snapshot") Text(if (isStruck) "↩" else "✕", color = c.textTertiary, fontSize = 12.sp,
                        modifier = Modifier.heightIn(min = 44.dp).widthIn(min = 36.dp).clickable { onToggleStrike(e.index) }.padding(horizontal = 10.dp, vertical = 12.dp).testTag("aciTeachStrike-${e.index}"))
                }
            }
        }
    }
}

// MARK: - Business profile (TeachProfileSheet)

private class ProfileRow(key: String, label: String, value: String, type: String) {
    var key by mutableStateOf(key); var label by mutableStateOf(label); var value by mutableStateOf(value); var type by mutableStateOf(type)
}
private val profileTypes = listOf("text" to "Text", "url" to "Web address", "email" to "Email", "folder" to "Folder", "number" to "Number")

/** GET /teach/profile: `{ values: { key: { value, label?, type? } } }`; no profile (or an older runtime) starts empty. */
internal suspend fun TeachClient.loadProfile(): TeachClient.Out<JsonObject> = when (val r = rawCall("teach/profile")) {
    is TeachClient.Out.Ok -> TeachClient.Out.Ok(r.v.obj?.get("values") as? JsonObject ?: JsonObject(emptyMap()))
    is TeachClient.Out.Failed -> if (r.message.contains("doesn’t have Teach")) TeachClient.Out.Ok(JsonObject(emptyMap())) else r
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TeachProfileSheet(client: TeachClient, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val rows = remember { mutableStateListOf<ProfileRow>() }
    var loading by remember { mutableStateOf(true) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        when (val r = client.loadProfile()) {
            is TeachClient.Out.Ok -> r.v.keys.sorted().forEach { k -> (r.v[k] as? JsonObject)?.let { e -> rows += ProfileRow(k, e.str("label").orEmpty(), e.str("value").orEmpty(), e.str("type")?.takeIf { it.isNotEmpty() } ?: "text") } }
            is TeachClient.Out.Failed -> error = "Couldn’t load the profile. ${r.message}"
        }
        loading = false
    }
    // profileRowProblems: each row's problem, by index.
    val problems = HashMap<Int, String>().also { out ->
        val seen = HashSet<String>()
        rows.forEachIndexed { i, r ->
            val k = r.key.trim()
            if (k.isEmpty()) out[i] = "Give it a key, like erp_url."
            else if (!Regex("^[a-zA-Z0-9_-]+$").matches(k)) out[i] = "Use letters, numbers, _ or - in the key."
            else if (k in seen) out[i] = "Two rows use the key $k."
            seen += k
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).verticalScroll(rememberScrollState()).testTag("aciTeachProfileSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Business profile", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            Text("Set things like your ERP address or reports folder once. A skill uses them as {{profile.key}}, so it doesn’t need an input for them each run.", color = c.textSecondary, fontSize = 13.sp)
            if (loading) Text("Loading…", color = c.textTertiary, fontSize = 13.sp)
            else if (rows.isEmpty()) Text("Nothing here yet. Add the first value.", color = c.textTertiary, fontSize = 13.sp)
            rows.forEachIndexed { i, row ->
                AciCard(Modifier.testTag("aciTeachProfileRow-$i")) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        AciSearchField(row.key, "erp_url", { row.key = it.replace(Regex("[^a-zA-Z0-9_-]"), "_") }, "aciTeachProfileKey-$i")
                        AciChipRow { profileTypes.forEach { (t, l) -> AciChip(l, row.type == t, "aciTeachProfileType-$i-$t") { row.type = t } } }
                        AciSearchField(row.label, "ERP address", { row.label = it }, "aciTeachProfileLabel-$i")
                        AciSearchField(row.value, "Value", { row.value = it }, "aciTeachProfileValue-$i")
                        val key = row.key.trim()
                        val problem = problems[i]
                        if (problem != null) Text(problem, color = AllternitStatic.StatusError, fontSize = 12.sp)
                        else if (key.isNotEmpty()) Text("Use as {{profile.$key}}", color = c.textTertiary, fontSize = 12.sp)
                        AciPillButton("Remove", "aciTeachProfileRemove-$i", "Remove ${key.ifEmpty { "row" }}") { rows.removeAt(i) }
                    }
                }
            }
            AciPillButton("Add value", "aciTeachProfileAdd") { rows += ProfileRow("", "", "", "text") }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            AciPrimaryButton(if (saving) "Saving…" else "Save", "aciTeachProfileSave") {
                if (saving || loading || problems.isNotEmpty()) return@AciPrimaryButton
                saving = true; error = null
                scope.launch {
                    val values = buildJsonObject {
                        rows.forEach { r -> putJsonObject(r.key.trim()) { put("value", r.value); if (r.label.isNotBlank()) put("label", r.label.trim()); if (r.type != "text") put("type", r.type) } }
                    }
                    val body = buildJsonObject { put("values", values); put("updated_at", java.time.Instant.now().toString()) }
                    when (val r = client.rawCall("teach/profile", "PUT", body.toString())) {
                        is TeachClient.Out.Ok -> onDismiss()
                        is TeachClient.Out.Failed -> error = "Couldn’t save the profile. ${r.message}"
                    }
                    saving = false
                }
            }
        }
    }
}

// MARK: - Run on a schedule (TeachScheduleSheet)

private val schedulePresets = listOf("hourly" to "Hourly", "daily" to "Daily 9:00", "weekdays" to "Weekdays 9:00", "custom" to "Custom")

/** Nothing is scheduled until "Schedule" is tapped: it creates one Automation routine (POST automation/routines), like iOS. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun TeachScheduleSheet(skill: TeachSkill, onDismiss: () -> Unit, onScheduled: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var bots by remember { mutableStateOf<List<Pair<String, String>>?>(null) }
    var botId by remember { mutableStateOf("") }
    var preset by remember { mutableStateOf("weekdays") }
    var custom by remember { mutableStateOf("0 9 * * 1-5") }
    val values = remember(skill.id) { mutableStateMapOf<String, String>().also { m -> skill.inputs.forEach { m[it.str("name").orEmpty()] = if (it.str("type") == "secret") "" else it.str("example").orEmpty() } } }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        val r = LibraryNet.get("agents")
        val list = (r as? com.allternit.core.api.ApiResult.Ok)?.value.listUnder("agents", "items", "data").map { it.str("id").orEmpty() to (it.str("name") ?: "Agent") }.filter { it.first.isNotEmpty() }
        bots = list
        if (botId.isEmpty()) botId = list.firstOrNull()?.first.orEmpty()
    }
    // teachScheduleCron: preset → cron; custom must be 5 fields.
    val cron: String? = when (preset) {
        "hourly" -> "0 * * * *"; "daily" -> "0 9 * * *"; "weekdays" -> "0 9 * * 1-5"
        else -> custom.trim().split(Regex("\\s+")).takeIf { it.size == 5 }?.joinToString(" ")
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).verticalScroll(rememberScrollState()).testTag("aciTeachScheduleSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Run on a schedule", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            Text("Runs “${skill.title}” as a turn in a bot’s chat, on this computer. You’ll find it in Automation Tasks.", color = c.textSecondary, fontSize = 13.sp)
            Text("Runs as", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            val list = bots
            if (list == null) Text("Loading bots…", color = c.textTertiary, fontSize = 13.sp)
            else if (list.isEmpty()) Text("You don’t have a bot yet. Make one in Bots first.", color = c.textSecondary, fontSize = 13.sp)
            else AciChipRow { list.forEach { (id, name) -> AciChip(name, botId == id, "aciTeachScheduleBot-$id") { botId = id } } }
            Text("How often", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
            AciChipRow { schedulePresets.forEach { (id, label) -> AciChip(label, preset == id, "aciTeachSchedulePreset-$id") { preset = id } } }
            if (preset == "custom") AciSearchField(custom, "0 9 * * 1-5", { custom = it }, "aciTeachScheduleCron")
            if (skill.inputs.isNotEmpty()) {
                Text("Input values", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                skill.inputs.forEach { i ->
                    val name = i.str("name").orEmpty()
                    val label = i.str("label")?.takeIf { it.isNotEmpty() } ?: name
                    Text(label, color = c.textSecondary, fontSize = 13.sp)
                    AciSearchField(values[name].orEmpty(), label, { values[name] = it }, "aciTeachScheduleInput-$name")
                }
            }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            AciPrimaryButton(if (saving) "Saving…" else "Schedule", "aciTeachScheduleGo") {
                val expr = cron
                if (saving || botId.isEmpty() || expr == null) return@AciPrimaryButton
                saving = true; error = null
                scope.launch {
                    val body = buildJsonObject {
                        put("name", skill.title)
                        put("description", (skill.intent.ifEmpty { "Teach skill: ${skill.title}" }).take(200))
                        put("agent_id", botId); put("schedule_type", "cron"); put("schedule_expression", expr)
                        put("timezone", java.util.TimeZone.getDefault().id); put("execution_domain", "local")
                        putJsonObject("config") { put("jobType", "agent"); put("prompt", TeachClient.prompt(skill, values.toMap())); put("teach_skill_id", skill.id) }
                        putJsonObject("metadata") { put("origin", "teach"); put("teach_skill_id", skill.id) }
                    }
                    val r = LibraryNet.send("POST", "automation/routines", body)
                    if (r is com.allternit.core.api.ApiResult.Ok) onScheduled(r.value.obj?.get("id")?.let { (it as? JsonPrimitive)?.content }.orEmpty())
                    else error = "Couldn’t create the schedule. ${r.friendly("create the schedule")}"
                    saving = false
                }
            }
        }
    }
}

