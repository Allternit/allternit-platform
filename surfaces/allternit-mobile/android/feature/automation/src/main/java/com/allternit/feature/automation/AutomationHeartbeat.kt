package com.allternit.feature.automation

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.activity.compose.BackHandler
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject

/*
 * Agent Heartbeats (iOS AutomationHeartbeatViews): the agent picker, then one agent's HEARTBEAT.md
 * tasks kept in step with the computer's scheduler (`v1/cron/jobs`). The file is
 * `agents/<id>/.allternit/governance/HEARTBEAT.md`, the same one the web's HeartbeatScheduler writes.
 */

/** One `###` section of an agent's HEARTBEAT.md. */
internal data class HeartbeatFileTask(val name: String, val schedule: String, val description: String)

internal object HeartbeatDocument {
    const val JOB_PREFIX = "Heartbeat: "

    fun path(agentId: String) = "agents/$agentId/.allternit/governance/HEARTBEAT.md"

    /** A section with no `(schedule)`: older files kept hourly/weekly only in the name's first word. */
    private fun scheduleFromName(name: String): String {
        val first = name.trim().split(Regex("\\s+")).firstOrNull()?.lowercase().orEmpty()
        return if (first == "hourly" || first == "weekly") first else "daily"
    }

    fun parse(content: String): List<HeartbeatFileTask> {
        val tasks = ArrayList<HeartbeatFileTask>()
        var name: String? = null
        var schedule = ""
        val description = StringBuilder()
        fun flush() {
            val n = name
            if (n != null && n.isNotEmpty()) tasks.add(HeartbeatFileTask(n, schedule, description.toString().trim('\n')))
            name = null; description.setLength(0)
        }
        for (raw in content.split("\n")) {
            val line = raw.trim()
            if (line.startsWith("###") && !line.startsWith("####")) {
                flush()
                val header = line.trimStart('#').trim()
                val open = header.lastIndexOf('(')
                if (header.endsWith(")") && open >= 0) { name = header.substring(0, open).trim(); schedule = header.substring(open + 1, header.length - 1) }
                else { name = header; schedule = scheduleFromName(header) }
            } else if (line.startsWith("## ") || line.startsWith("# ")) flush()
            else if (line.startsWith("-") && name != null) description.append(line.drop(1).trim()).append('\n')
        }
        flush()
        return tasks
    }

    fun generate(tasks: List<HeartbeatFileTask>): String {
        val lines = arrayListOf("# HEARTBEAT.md — Agent Periodic Tasks", "", "## Scheduled Tasks", "")
        for (t in tasks) {
            val plain = t.schedule == scheduleFromName(t.name) && !t.schedule.contains(":")
            lines.add(if (plain) "### ${t.name}" else "### ${t.name} (${t.schedule})")
            lines.add("")
            for (l in t.description.split("\n")) if (l.isNotEmpty()) lines.add("- $l")
            lines.add("")
        }
        lines += listOf("## Task History", "", "*Tasks will be logged here when executed*", "")
        return lines.joinToString("\n")
    }

    /** taskToJobConfig: common schedules to cron. */
    fun cron(schedule: String): String {
        val v = schedule.trim()
        when (v.lowercase()) { "hourly" -> return "0 * * * *"; "daily" -> return "0 9 * * *"; "weekly" -> return "0 9 * * 1" }
        val t = v.split(":")
        if (t.size == 2 && t[1].length == 2) {
            val h = t[0].toIntOrNull(); val m = t[1].toIntOrNull()
            if (h != null && m != null) return "$m $h * * *"
        }
        return v
    }

    /** The other way, for the file: "09:00" for a daily time, else the cron. */
    fun scheduleFromCron(cron: String): String {
        if (cron == "0 * * * *") return "hourly"
        if (cron == "0 9 * * 1") return "weekly"
        val f = cron.split(" ")
        if (f.size == 5 && f[2] == "*" && f[3] == "*" && f[4] == "*") {
            val m = f[0].toIntOrNull(); val h = f[1].toIntOrNull()
            if (m != null && h != null) return "%02d:%02d".format(h, m)
        }
        return cron
    }

    fun prompt(name: String, description: String) = "Execute heartbeat task: $name\n\n$description"
}

// MARK: - Requests

private suspend fun AutomationApi.readHeartbeatFile(agentId: String): AResult<String> =
    when (val r = backend.relay("GET", "api/v1/files/read?path=${esc(HeartbeatDocument.path(agentId))}", doing = "read HEARTBEAT.md")) {
        is AResult.Ok -> AResult.Ok((r.value as? JsonObject).str("content").orEmpty())
        AResult.NoComputer -> AResult.NoComputer
        is AResult.Failed -> if (r.status == 404) AResult.Ok("") else r
    }

private suspend fun AutomationApi.writeHeartbeatFile(agentId: String, content: String) =
    backend.relay("POST", "api/v1/files/write", buildJsonObject { put("path", HeartbeatDocument.path(agentId)); put("content", content) }, "save HEARTBEAT.md")

private suspend fun AutomationApi.createAgentJob(name: String, prompt: String, cron: String, description: String, agentId: String, paused: Boolean) =
    backend.relay("POST", "v1/cron/jobs", buildJsonObject {
        put("name", HeartbeatDocument.JOB_PREFIX + name); put("type", "agent"); put("schedule", cron); put("description", description)
        if (paused) put("status", "paused")
        putJsonObject("config") { put("prompt", prompt); put("agentId", agentId) }
        put("maxRetries", 3); put("timeoutSeconds", 300)
    }, "add $name")

private suspend fun AutomationApi.updateAgentJob(id: String, name: String?, description: String?, cron: String?, prompt: String?) =
    backend.relay("PUT", "v1/cron/jobs/${esc(id)}", buildJsonObject {
        name?.let { put("name", HeartbeatDocument.JOB_PREFIX + it) }; description?.let { put("description", it) }; cron?.let { put("schedule", it) }
        prompt?.let { p -> putJsonObject("config") { put("prompt", p) } }
    }, "save the task")

private fun AResult<*>.problem(): String? = when (this) {
    is AResult.Ok -> null
    AResult.NoComputer -> "Connect a computer or Allternit Cloud to manage heartbeats."
    is AResult.Failed -> message
}

// MARK: - Screens

/** The Agent Heartbeats tab: the picker, or one agent's scheduler. */
@Composable
internal fun HeartbeatsTab(store: AutomationStore, agentId: String?, onPick: (String?) -> Unit) {
    val agent = agentId?.let { id -> store.bots.firstOrNull { it.id == id } }
    if (agent != null) HeartbeatScheduler(store, agent) { onPick(null) } else HeartbeatAgentPicker(store, onPick = { onPick(it.id) })
}

@Composable
private fun HeartbeatAgentPicker(store: AutomationStore, onPick: (BotLite) -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp).testTag("heartbeatPicker"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Agent Heartbeat Tasks", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
        Text(
            "Configure periodic tasks that agents execute automatically. Heartbeat tasks are stored in each agent's workspace and synced with the scheduler.",
            color = c.textSecondary, fontSize = 13.sp,
        )
        if (store.bots.isEmpty()) Text("No agents yet.", color = c.textTertiary, fontSize = 13.sp)
        store.bots.forEach { b ->
            val n = store.tasks.count { it.type == TaskType.Heartbeat && it.agentId == b.id }
            Row(
                Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).testTag("heartbeatAgent-${b.id}")
                    .clickable(role = Role.Button) { onPick(b) }.padding(horizontal = 14.dp),
                verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(b.name.ifEmpty { b.id }, color = c.textPrimary, fontSize = 15.sp)
                Text(if (n == 0) "No tasks" else "$n task" + if (n == 1) "" else "s", color = c.textSecondary, fontSize = 12.sp)
            }
        }
    }
}

private data class HbRow(val id: String, val name: String, val schedule: String, val description: String, val job: AutoTask?) {
    val file get() = HeartbeatFileTask(name, schedule, description)
}

@Composable
private fun HeartbeatScheduler(store: AutomationStore, agent: BotLite, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val fileTasks = remember(agent.id) { mutableStateListOf<HeartbeatFileTask>() }
    var fileLoaded by remember(agent.id) { mutableStateOf(false) }
    var loading by remember(agent.id) { mutableStateOf(true) }
    var error by remember(agent.id) { mutableStateOf<String?>(null) }
    var adding by remember(agent.id) { mutableStateOf(false) }
    var expanded by remember(agent.id) { mutableStateOf<String?>(null) }
    var busy by remember(agent.id) { mutableStateOf<String?>(null) }
    var pendingDelete by remember(agent.id) { mutableStateOf<HbRow?>(null) }
    BackHandler(onBack = onClose)

    fun rows(): List<HbRow> {
        val synced = store.tasks.filter { it.type == TaskType.Heartbeat && it.agentId == agent.id }.map { j ->
            val n = j.name.removePrefix(HeartbeatDocument.JOB_PREFIX)
            HbRow(j.id, n, HeartbeatDocument.scheduleFromCron(j.scheduleExpression), j.description, j)
        }
        val names = synced.map { it.name }.toSet()
        return synced + fileTasks.filter { it.name !in names }.map { HbRow("file-${it.name}", it.name, it.schedule, it.description, null) }
    }

    suspend fun writeFile() {
        if (!fileLoaded) { error = "Saved to the scheduler, but HEARTBEAT.md wasn't updated because it couldn't be read. Refresh and try again."; return }
        val tasks = rows().map { it.file }
        val p = store.api.writeHeartbeatFile(agent.id, HeartbeatDocument.generate(tasks)).problem()
        if (p != null) error = "Saved to the scheduler, but HEARTBEAT.md wasn't updated. $p"
        else { fileTasks.clear(); fileTasks.addAll(tasks) }
    }

    suspend fun load() {
        loading = true; error = null
        store.load()
        when (val r = store.api.readHeartbeatFile(agent.id)) {
            is AResult.Ok -> { fileTasks.clear(); fileTasks.addAll(HeartbeatDocument.parse(r.value)); fileLoaded = true }
            else -> { fileLoaded = false; error = "Couldn't read HEARTBEAT.md. ${r.problem().orEmpty()}" }
        }
        loading = false
    }
    LaunchedEffect(agent.id) { load() }

    /** Runs one change against the scheduler, then refreshes the list and the file. */
    fun act(id: String, failure: String, afterRefresh: Boolean = true, block: suspend () -> AResult<*>) {
        scope.launch {
            busy = id; error = null
            val p = block().problem()
            if (p != null) error = "$failure $p" else { store.load(); if (afterRefresh) writeFile() }
            busy = null
        }
    }

    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp).testTag("heartbeatScheduler"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            Text("‹ ${agent.name.ifEmpty { agent.id }}", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.testTag("heartbeatBack").heightIn(min = 44.dp).clickable(role = Role.Button, onClick = onClose).padding(end = 12.dp))
            Pill("Add task", "heartbeatAdd", filled = true) { adding = !adding }
        }
        Text("Tasks in HEARTBEAT.md, kept in step with the scheduler.", color = c.textSecondary, fontSize = 12.sp)
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp, modifier = Modifier.testTag("heartbeatError")) }
        if (adding) HbForm("New heartbeat task", null, "Add", onCancel = { adding = false }) { name, time, desc ->
            adding = false
            act("add", "Couldn't add $name.") { store.api.createAgentJob(name, HeartbeatDocument.prompt(name, desc), HeartbeatDocument.cron(time), desc, agent.id, paused = false) }
        }
        val list = rows()
        when {
            loading && list.isEmpty() -> Text("Loading…", color = c.textTertiary, fontSize = 13.sp)
            list.isEmpty() -> Text("No heartbeat tasks yet. Add one and this agent runs it on schedule.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.testTag("heartbeatEmpty"))
        }
        list.forEach { row ->
            val on = row.job?.isActive == true
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).testTag("heartbeatRow-${row.id}")) {
                Row(Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(role = Role.Button) { expanded = if (expanded == row.id) null else row.id }.padding(horizontal = 14.dp, vertical = 8.dp), horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(row.name, color = c.textPrimary, fontSize = 15.sp, maxLines = 1)
                        Text(CronWords.describe("cron", HeartbeatDocument.cron(row.schedule)), color = c.textSecondary, fontSize = 12.sp, maxLines = 1)
                    }
                    Text(if (row.job == null) "Not synced" else if (on) "Active" else "Paused", color = c.textTertiary, fontSize = 12.sp)
                }
                if (expanded == row.id) Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (row.description.isNotBlank()) Text(row.description, color = c.textSecondary, fontSize = 13.sp)
                    HbForm("Edit task", row, "Save", onCancel = { expanded = null }) { name, time, desc ->
                        act(row.id, "Couldn't save ${row.name}.") {
                            val job = row.job
                            if (job != null) {
                                val newCron = if (time != row.schedule) HeartbeatDocument.cron(time) else null
                                val changed = name != row.name || desc != row.description
                                store.api.updateAgentJob(job.id, name.takeIf { it != row.name }, desc.takeIf { it != row.description }, newCron, if (changed) HeartbeatDocument.prompt(name, desc) else null)
                            } else {
                                fileTasks.removeAll { it.name == row.name }; fileTasks.add(HeartbeatFileTask(name, time, desc)); AResult.Ok(Unit)
                            }
                        }
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        val job = row.job
                        if (job != null) {
                            Pill(if (on) "Pause" else "Resume", "heartbeatToggle-${row.id}", enabled = busy == null) { act(row.id, "Couldn't update ${row.name}.", afterRefresh = false) { store.api.setEnabled(job, !on) } }
                            Pill("Run now", "heartbeatRun-${row.id}", enabled = busy == null) { act(row.id, "Couldn't run ${row.name}.", afterRefresh = false) { store.api.runNow(job) } }
                        } else {
                            Pill("Sync", "heartbeatSync-${row.id}", enabled = busy == null) {
                                act(row.id, "Couldn't sync ${row.name}.") { store.api.createAgentJob(row.name, HeartbeatDocument.prompt(row.name, row.description), HeartbeatDocument.cron(row.schedule), row.description, agent.id, paused = false) }
                            }
                        }
                        Pill("Remove", "heartbeatRemove-${row.id}", enabled = busy == null) { pendingDelete = row }
                    }
                }
            }
        }
    }
    pendingDelete?.let { row ->
        AlertDialog(
            onDismissRequest = { pendingDelete = null },
            title = { Text("Remove ${row.name}?") },
            text = { Text("The agent stops running it and it leaves HEARTBEAT.md.") },
            confirmButton = {
                TextButton(onClick = {
                    pendingDelete = null
                    fileTasks.removeAll { it.name == row.name }
                    act(row.id, "Couldn't remove ${row.name}.") { row.job?.let { store.api.delete(it) } ?: AResult.Ok(Unit) }
                }, modifier = Modifier.testTag("heartbeatRemoveConfirm")) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { pendingDelete = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Pill(label: String, tag: String, filled: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(22.dp)).testTag(tag)
            .background(if (filled) c.accentCode.copy(alpha = if (enabled) 1f else 0.4f) else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick).padding(horizontal = 16.dp),
        contentAlignment = androidx.compose.ui.Alignment.Center,
    ) { Text(label, color = if (filled) c.onAccent else if (enabled) c.textPrimary else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
}

/** "New Heartbeat Task" / the edit form: name, when ("09:00", hourly, daily, weekly or a cron) and what to do. */
@Composable
private fun HbForm(title: String, row: HbRow?, action: String, onCancel: () -> Unit, onSubmit: (name: String, time: String, description: String) -> Unit) {
    val c = AllternitTheme.colors
    var name by remember(row?.id) { mutableStateOf(row?.name.orEmpty()) }
    var time by remember(row?.id) { mutableStateOf(row?.schedule ?: "09:00") }
    var desc by remember(row?.id) { mutableStateOf(row?.description.orEmpty()) }
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        HbField(name, "Task name", "heartbeatName") { name = it }
        HbField(time, "When: 09:00, hourly, daily, weekly or a cron", "heartbeatTime") { time = it }
        HbField(desc, "What the agent should do", "heartbeatDesc", singleLine = false) { desc = it }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Pill(action, "heartbeatSubmit", filled = true, enabled = name.isNotBlank() && time.isNotBlank()) { onSubmit(name.trim(), time.trim(), desc.trim()) }
            Pill("Cancel", "heartbeatCancel", onClick = onCancel)
        }
    }
}

@Composable
private fun HbField(value: String, hint: String, tag: String, singleLine: Boolean = true, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    BasicTextField(
        value = value, onValueChange = onChange, singleLine = singleLine,
        textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp), cursorBrush = SolidColor(c.textPrimary),
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(c.fillNeutral).padding(12.dp).testTag(tag),
        decorationBox = { inner -> Box { if (value.isEmpty()) Text(hint, color = c.textTertiary, fontSize = 15.sp); inner() } },
    )
}
