package com.allternit.feature.computer

import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** One Teach skill (iOS TeachSkill): the raw JSON is kept so a save writes back every field the editor doesn't know. */
internal data class TeachSkill(val raw: JsonObject) {
    val id: String get() = raw.str("id").orEmpty()
    val title: String get() = raw.str("title")?.takeIf { it.isNotEmpty() } ?: "Untitled skill"
    val intent: String get() = raw.str("intent").orEmpty()
    val domain: String? get() = raw.str("domain")?.takeIf { it.isNotEmpty() }
    val steps: List<JsonObject> get() = raw.list("steps")
    val inputs: List<JsonObject> get() = raw.list("inputs")
    val updatedAt: String get() = raw.str("updated_at").orEmpty()
    val subtitle: String get() = steps.size.let { n -> "$n step${if (n == 1) "" else "s"}" }.let { n -> domain?.let { "$it · $n" } ?: n }
}

/** `/api/har-derived-api/*` on the person's own computer, through the relay (iOS TeachClient). */
internal class TeachClient(private val backend: ChatBackend) {
    sealed interface Out<out T> { data class Ok<T>(val v: T) : Out<T>; data class Failed(val message: String, val gate: Boolean = false) : Out<Nothing> }
    private suspend fun call(path: String, method: String = "GET", body: String? = null): Out<JsonElement> = when (val r = backend.runtimeJson(method, "/api/har-derived-api/$path", body)) {
        is RuntimeCall.Ok -> Out.Ok(r.body)
        RuntimeCall.NoComputer -> Out.Failed("No computer is connected.", gate = true)
        is RuntimeCall.Http -> if (r.status == 404) Out.Failed("This computer doesn’t have Teach yet. Update Allternit Desktop, then try again.") else if (r.status == 401) Out.Failed("Sign in to save skills.") else Out.Failed(r.failureText().orEmpty())
        else -> Out.Failed(r.failureText().orEmpty())
    }
    private fun esc(s: String) = LibraryNet.escape(s)

    suspend fun listSkills(): Out<List<TeachSkill>> = when (val r = call("teach/skills")) {
        is Out.Ok -> Out.Ok(r.v.obj.list("skills").map(::TeachSkill).sortedByDescending { it.updatedAt })
        is Out.Failed -> r
    }
    suspend fun save(skill: TeachSkill): Out<TeachSkill> = when (val r = call("teach/skills/${esc(skill.id)}", "PUT", skill.raw.toString())) {
        is Out.Ok -> Out.Ok((r.v as? JsonObject)?.takeIf { it.containsKey("id") }?.let(::TeachSkill) ?: skill)
        is Out.Failed -> r
    }
    suspend fun delete(id: String): String? = (call("teach/skills/${esc(id)}", "DELETE") as? Out.Failed)?.message

    /** An imported .har → a contract (`POST /ingest`), then the model's draft (`POST /teach/distill`) saved as a new skill. */
    suspend fun analyze(har: String, note: String): Out<TeachSkill> {
        val ing = when (val r = call("ingest", "POST", buildJsonObject { put("har", har); put("source", "upload") }.toString())) { is Out.Ok -> r.v.obj; is Out.Failed -> return r }
        val contract = ing.str("contract_id") ?: return Out.Failed("Couldn’t read that recording.")
        val body = buildJsonObject {
            put("events", JsonArray(emptyList()))
            put("contract_id", contract)
            if (note.isNotBlank()) put("note", note.trim())
        }.toString()
        val draft = when (val r = call("teach/distill", "POST", body)) { is Out.Ok -> r.v.obj ?: return Out.Failed("Couldn’t analyze the recording."); is Out.Failed -> return r }
        val now = java.time.Instant.now().toString()
        val steps = JsonArray(draft.list("steps").map { st -> if (st.str("id").isNullOrEmpty()) JsonObject(st + ("id" to JsonPrimitive(newId("step")))) else st })
        val out = HashMap<String, JsonElement>(draft)
        out["steps"] = steps; out["id"] = JsonPrimitive(newId("teach")); out["contract_id"] = JsonPrimitive(contract)
        ing.str("domain")?.takeIf { it.isNotEmpty() }?.let { out["domain"] = JsonPrimitive(it) }
        if (!out.containsKey("inputs")) out["inputs"] = JsonArray(emptyList())
        out["created_at"] = JsonPrimitive(now); out["updated_at"] = JsonPrimitive(now)
        return Out.Ok(TeachSkill(JsonObject(out)))
    }

    companion object {
        fun newId(prefix: String) = prefix + "-" + java.util.UUID.randomUUID().toString().replace("-", "").take(12)
        private val FILL = Regex("""\{\{\s*([a-zA-Z0-9_.-]+)\s*\}\}""")
        fun fill(text: String, values: Map<String, String>) = FILL.replace(text) { m -> values[m.groupValues[1]]?.takeIf { it.isNotEmpty() } ?: m.value }

        /** `Run the skill "<title>" with inputs: … Steps: …` (renderTeachPrompt). */
        fun prompt(skill: TeachSkill, values: Map<String, String>): String {
            val inputs = if (skill.inputs.isEmpty()) "none" else skill.inputs.joinToString(", ") { "${it.str("name")} = ${values[it.str("name")] ?: it.str("example").orEmpty()}" }
            val lines = mutableListOf("Run the skill \"${skill.title}\" with inputs: $inputs.")
            if (skill.intent.isNotBlank()) lines += "Goal: ${fill(skill.intent.trim(), values)}"
            skill.domain?.let { lines += "Site: $it" }
            lines += "Steps:"
            lines += skill.steps.mapIndexed { i, s ->
                val target = if (s.str("kind") == "api" && s.str("target") != null) " (${fill(s.str("target").orEmpty(), values)})" else ""
                "${i + 1}. ${fill(s.str("text").orEmpty().trim(), values)}$target"
            }.joinToString("\n")
            if (skill.steps.any { it.str("kind") == "api" }) lines += "For API steps, call the API with the user's signed-in session instead of clicking through the page."
            lines += "Ask for approval before any step that submits, pays, sends or deletes."
            return lines.joinToString("\n")
        }

        /** The page a replay starts on: the first URL in the steps, else the skill's site (teachStartUrl). */
        fun startUrl(skill: TeachSkill): String? {
            val url = Regex("""https?://[^\s)"']+""")
            for (s in skill.steps) for (t in listOfNotNull(s.str("target"), s.str("text"))) url.find(t)?.let { return it.value }
            return skill.domain?.let { "https://$it" }
        }
    }
}

private val STEP_KINDS = listOf("api" to "API", "ui" to "Page", "shell" to "Shell", "note" to "Note")

private class DraftStep(val raw: JsonObject, id: String, kind: String, text: String, target: String) {
    val id = id
    var kind by mutableStateOf(kind); var text by mutableStateOf(text); var target by mutableStateOf(target)
}
private class DraftInput(val raw: JsonObject, name: String, label: String, example: String, required: Boolean) {
    var name by mutableStateOf(name); var label by mutableStateOf(label); var example by mutableStateOf(example); var required by mutableStateOf(required)
}

/**
 * Teach (iOS TeachView): the skills the agent can replay. List, a skill's editor (goal, steps, inputs, save, delete),
 * Run (fill the inputs, the agent replays it in ACI's browser) and Import a .har recording to analyze into a skill.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AciTeachScreen(host: AciHost, onRun: (String) -> Unit, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val client = remember(host.backend) { TeachClient(host.backend) }
    var skills by remember { mutableStateOf<List<TeachSkill>?>(null) }
    var failure by remember { mutableStateOf<TeachClient.Out.Failed?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf<TeachSkill?>(null) }
    var review by remember { mutableStateOf<Pair<String, String>?>(null) } // file name, har
    var note by remember { mutableStateOf("") }
    var analyzing by remember { mutableStateOf(false) }
    var reviewError by remember { mutableStateOf<String?>(null) }
    var runFor by remember { mutableStateOf<TeachSkill?>(null) }
    suspend fun load() {
        when (val r = client.listSkills()) {
            is TeachClient.Out.Ok -> { skills = r.v; failure = null }
            is TeachClient.Out.Failed -> failure = r
        }
    }
    LaunchedEffect(host.signedIn) { if (host.signedIn) load() }
    BackHandler { if (selected != null) selected = null else if (review != null) review = null else onBack() }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val name = uri.lastPathSegment?.substringAfterLast('/') ?: "recording.har"
        val text = runCatching { context.contentResolver.openInputStream(uri)?.bufferedReader()?.use { it.readText() } }.getOrNull()
        if (text == null) notice = "Couldn’t read that file."
        else if (!name.lowercase().let { it.endsWith(".har") || it.endsWith(".json") }) notice = "Pick a .har file."
        else { review = name to text; note = ""; reviewError = null }
    }

    AciFullPage {
        Column(Modifier.fillMaxSize().testTag("aciTeach")) {
            AciTopBar("Teach", "ACI", { if (selected != null) selected = null else onBack() }) {
                if (selected == null && review == null) Text("Import", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.heightIn(min = 44.dp).clickable { picker.launch(arrayOf("*/*")) }.padding(8.dp).testTag("aciTeachImport"))
            }
            Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp)) {
                notice?.let { Text(it, color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.padding(bottom = 8.dp).clickable { notice = null }.testTag("aciTeachNotice")) }
                val sel = selected
                val rev = review
                val list = skills
                when {
                    !host.signedIn -> AciStateCard("Sign in to use Teach", "Skills are saved to your Allternit account.", "Sign in", host.openSignIn)
                    rev != null -> Column(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.testTag("aciTeachReview")) {
                        Text("Review before Analyze", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text("Check what will be sent, then Analyze turns it into a skill.", color = c.textSecondary, fontSize = 13.sp)
                        Text("What will be sent", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                        Text("Network traffic from ${rev.first}. No actions or narration: those come from Allternit Desktop.", color = c.textSecondary, fontSize = 13.sp)
                        Text("Your note", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                        AciSearchField(note, "What should the agent do with this?", { note = it }, "aciTeachNote")
                        reviewError?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
                        AciPrimaryButton(if (analyzing) "Analyzing…" else "Analyze", "aciTeachAnalyze") {
                            if (!analyzing) scope.launch {
                                analyzing = true; reviewError = null
                                when (val r = client.analyze(rev.second, note)) {
                                    is TeachClient.Out.Ok -> {
                                        val saved = (client.save(r.v) as? TeachClient.Out.Ok)?.v ?: r.v
                                        skills = listOf(saved) + skills.orEmpty(); selected = saved; review = null
                                    }
                                    is TeachClient.Out.Failed -> reviewError = "Couldn’t analyze the recording. ${r.message}"
                                }
                                analyzing = false
                            }
                        }
                        AciPillButton("Cancel", "aciTeachReviewCancel") { review = null }
                    }
                    sel != null -> TeachEditor(sel, client,
                        onSaved = { s -> skills = skills.orEmpty().map { if (it.id == s.id) s else it }; selected = s; notice = "Saved." },
                        onError = { notice = it },
                        onRun = { runFor = sel },
                        onDeleted = { skills = skills.orEmpty().filterNot { it.id == sel.id }; selected = null })
                    failure != null && list == null -> {
                        val f = failure!!
                        if (f.gate) AciStateCard("No computer is connected", "Teach skills and recordings are saved on your computer. Open Allternit Desktop or start your cloud computer, then try again.", "Try again", { scope.launch { load() } })
                        else AciStateCard("Couldn’t load skills", f.message, "Retry", { scope.launch { load() } })
                    }
                    list == null -> AciStateCard("Loading skills…", "Reading your skills.")
                    list.isEmpty() -> Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("aciTeachEmpty")) {
                        Text("Show the agent this site", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text("Click through the flow once in Allternit Desktop or the Allternit browser extension. We record the path and save it as a skill the agent can replay.", color = c.textSecondary, fontSize = 13.sp)
                        AciPillButton("Import a recording", "aciTeachImportEmpty") { picker.launch(arrayOf("*/*")) }
                    }
                    else -> list.forEach { s ->
                        AciListRow(s.title, listOfNotNull(s.subtitle, if (s.raw.str("skill_path") != null) "Skill saved" else null, if (s.raw.str("routine_id") != null) "Scheduled" else null).joinToString(" · "), null, "aciTeachSkill-${s.id}") { selected = s }
                    }
                }
            }
        }
    }
    runFor?.let { skill ->
        TeachRunSheet(skill, onDismiss = { runFor = null }) { values ->
            runFor = null
            val start = TeachClient.startUrl(skill)?.let { "Start at $it." } ?: "Open the right page first."
            onRun(TeachClient.prompt(skill, values) + "\n\n" + start + " Do every step in the browser, so the person watching sees each page.")
        }
    }
}

@Composable
private fun TeachEditor(skill: TeachSkill, client: TeachClient, onSaved: (TeachSkill) -> Unit, onError: (String) -> Unit, onRun: () -> Unit, onDeleted: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var title by remember(skill.id) { mutableStateOf(skill.raw.str("title").orEmpty()) }
    var intent by remember(skill.id) { mutableStateOf(skill.intent) }
    val steps = remember(skill.id) { mutableStateListOf<DraftStep>().also { l -> skill.steps.forEach { l += DraftStep(it, it.str("id") ?: TeachClient.newId("step"), it.str("kind") ?: "ui", it.str("text").orEmpty(), it.str("target").orEmpty()) } } }
    val inputs = remember(skill.id) { mutableStateListOf<DraftInput>().also { l -> skill.inputs.forEach { l += DraftInput(it, it.str("name").orEmpty(), it.str("label").orEmpty(), it.str("example").orEmpty(), it.bool("required") != false) } } }
    var confirmDelete by remember(skill.id) { mutableStateOf(false) }
    var saving by remember(skill.id) { mutableStateOf(false) }

    fun draft(): TeachSkill {
        val stepValues = steps.map { s ->
            val m = HashMap<String, JsonElement>(s.raw)
            m["id"] = JsonPrimitive(s.id); m["kind"] = JsonPrimitive(s.kind); m["text"] = JsonPrimitive(s.text)
            if (s.kind == "api" && s.target.isNotBlank()) m["target"] = JsonPrimitive(s.target.trim()) else m.remove("target")
            JsonObject(m)
        }
        val inputValues = inputs.map { i ->
            val m = HashMap<String, JsonElement>(i.raw)
            m["name"] = JsonPrimitive(i.name.trim()); m["label"] = JsonPrimitive(i.label)
            m["example"] = JsonPrimitive(if (i.raw.str("type") == "secret") "" else i.example)
            if (i.required) m.remove("required") else m["required"] = kotlinx.serialization.json.JsonPrimitive(false)
            JsonObject(m)
        }
        val m = HashMap<String, JsonElement>(skill.raw)
        m["title"] = JsonPrimitive(title); m["intent"] = JsonPrimitive(intent)
        m["steps"] = JsonArray(stepValues); m["inputs"] = JsonArray(inputValues)
        m["updated_at"] = JsonPrimitive(java.time.Instant.now().toString())
        return TeachSkill(JsonObject(m))
    }
    val unknown = steps.flatMap { s -> Regex("""\{\{\s*([a-zA-Z0-9_.-]+)\s*\}\}""").findAll(s.text).map { it.groupValues[1] }.toList() }.filter { n -> !n.startsWith("profile.") && inputs.none { it.name == n } }.distinct()

    Column(verticalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.testTag("aciTeachEditor")) {
        AciPrimaryButton("Run", "aciTeachRun") { onRun() }
        AciSearchField(title, "Skill name", { title = it }, "aciTeachTitle")
        Text(skill.subtitle + if (skill.raw.str("skill_path") != null) " · Skill saved" else "", color = c.textSecondary, fontSize = 12.sp)
        Text("Goal", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        AciSearchField(intent, "What this skill does", { intent = it }, "aciTeachIntent")
        Text("Steps", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        if (steps.isEmpty()) Text("No steps yet. Add the first one.", color = c.textSecondary, fontSize = 13.sp)
        steps.forEachIndexed { idx, s ->
            AciCard(Modifier.testTag("aciTeachStep-$idx")) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("${idx + 1}", color = c.textTertiary, fontSize = 12.sp)
                    AciChipRow { STEP_KINDS.forEach { (k, label) -> AciChip(label, s.kind == k, "aciTeachKind-$idx-$k") { s.kind = k } } }
                    AciSearchField(s.text, "What happens in this step", { s.text = it }, "aciTeachStepText-$idx")
                    if (s.kind == "api") AciSearchField(s.target, "API address", { s.target = it }, "aciTeachStepTarget-$idx")
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AciPillButton("Up", "aciTeachUp-$idx", "Move step ${idx + 1} up") { if (idx > 0) { val t = steps[idx]; steps[idx] = steps[idx - 1]; steps[idx - 1] = t } }
                        AciPillButton("Down", "aciTeachDown-$idx", "Move step ${idx + 1} down") { if (idx < steps.lastIndex) { val t = steps[idx]; steps[idx] = steps[idx + 1]; steps[idx + 1] = t } }
                        AciPillButton("Remove", "aciTeachRemoveStep-$idx", "Remove step ${idx + 1}") { steps.removeAt(idx) }
                    }
                }
            }
        }
        AciPillButton("Add step", "aciTeachAddStep") { steps += DraftStep(JsonObject(emptyMap()), TeachClient.newId("step"), "ui", "", "") }
        Text("Inputs", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        if (inputs.isEmpty()) Text("No inputs. Add one for a value that changes each run, then use {{name}} in a step.", color = c.textSecondary, fontSize = 13.sp)
        unknown.forEach { n -> Text("{{$n}} has no input. Add an input named “$n” or fix the name.", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.clickable { inputs += DraftInput(JsonObject(emptyMap()), n, n.replace(Regex("[_.-]+"), " ").replaceFirstChar { it.uppercase() }, "", true) }) }
        inputs.forEachIndexed { idx, i ->
            AciCard(Modifier.testTag("aciTeachInput-$idx")) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    AciSearchField(i.name, "Name", { i.name = it }, "aciTeachInputName-$idx")
                    AciSearchField(i.label, "Label", { i.label = it }, "aciTeachInputLabel-$idx")
                    if (i.raw.str("type") == "secret") Text("No example for secrets", color = c.textTertiary, fontSize = 12.sp)
                    else AciSearchField(i.example, "Example", { i.example = it }, "aciTeachInputExample-$idx")
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        AciChip("Required", i.required, "aciTeachInputRequired-$idx") { i.required = !i.required }
                        AciPillButton("Remove", "aciTeachRemoveInput-$idx", "Remove input ${i.name}") { inputs.removeAt(idx) }
                    }
                }
            }
        }
        AciPillButton("Add input", "aciTeachAddInput") {
            var n = inputs.size + 1
            while (inputs.any { it.name == "input_$n" }) n++
            inputs += DraftInput(JsonObject(emptyMap()), "input_$n", "", "", true)
        }
        AciPrimaryButton(if (saving) "Saving…" else "Save changes", "aciTeachSave") {
            if (!saving) scope.launch {
                saving = true
                when (val r = client.save(draft())) {
                    is TeachClient.Out.Ok -> onSaved(r.v)
                    is TeachClient.Out.Failed -> onError("Couldn’t save the skill. ${r.message}")
                }
                saving = false
            }
        }
        if (!confirmDelete) AciPillButton("Delete skill", "aciTeachDelete") { confirmDelete = true }
        else Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("The agent won't be able to replay it any more.", color = c.textSecondary, fontSize = 13.sp)
            AciPillButton("Yes, delete", "aciTeachDeleteConfirm") { scope.launch { val e = client.delete(skill.id); if (e == null) onDeleted() else onError("Couldn’t delete the skill. $e") } }
            AciPillButton("Cancel", "aciTeachDeleteCancel") { confirmDelete = false }
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TeachRunSheet(skill: TeachSkill, onDismiss: () -> Unit, onRun: (Map<String, String>) -> Unit) {
    val c = AllternitTheme.colors
    val values = remember(skill.id) { mutableStateListOf<Pair<String, String>>().also { l -> skill.inputs.forEach { l += (it.str("name").orEmpty() to it.str("example").orEmpty()) } } }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).verticalScroll(rememberScrollState()).testTag("aciTeachRunSheet"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Run “${skill.title}”", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
            Text("Fill in the values for this run. The agent asks before any step that submits, pays, sends or deletes.", color = c.textSecondary, fontSize = 13.sp)
            values.forEachIndexed { idx, (name, v) ->
                val label = skill.inputs.getOrNull(idx)?.str("label")?.takeIf { it.isNotEmpty() } ?: name
                Text(label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                AciSearchField(v, label, { values[idx] = name to it }, "aciTeachRunInput-$name")
            }
            AciPrimaryButton("Run", "aciTeachRunGo") { onRun(values.toMap()) }
        }
    }
}
