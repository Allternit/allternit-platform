package com.allternit.feature.bots

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/**
 * "Edit with chat" under the workspace file editor (iOS WorkspaceFileEditorView assistPanel, web
 * WorkspaceChatEditor.tsx): the current content plus an instruction streams a proposed revision in
 * a short-lived session that rides the bot's own id. Nothing touches the file until Apply puts the
 * proposal in the editor and the person taps Save.
 */
@Composable
internal fun EditWithChatPanel(bot: BotRecord, file: WorkspaceFileInfo, backend: ChatBackend, content: String, onApply: (String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var instruction by remember { mutableStateOf("") }
    var assisting by remember { mutableStateOf(false) }
    var assistError by remember { mutableStateOf<String?>(null) }
    var preview by remember { mutableStateOf<String?>(null) }
    var sessionId by remember { mutableStateOf<String?>(null) }
    var job by remember { mutableStateOf<Job?>(null) }
    var stopped by remember { mutableStateOf(false) }
    DisposableEffect(file.path) {
        onDispose {
            job?.cancel()
            sessionId?.let { sid -> kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.IO).launch { backend.deleteSession(sid) } }
        }
    }
    fun send() {
        val text = instruction.trim()
        if (text.isEmpty() || assisting) return
        assistError = null
        preview = null
        assisting = true
        stopped = false
        job = scope.launch {
            val rid = when (val r = BotTurn.runtime(backend)) {
                is BotsResult.Ok -> r.value
                is BotsResult.Failed -> { assistError = r.message; assisting = false; return@launch }
            }
            val sid = sessionId ?: when (val r = BotTurn.session(backend, "Workspace edit — ${file.path}", bot.id, bot.displayName)) {
                is BotsResult.Ok -> r.value.also { sessionId = it }
                is BotsResult.Failed -> { assistError = r.message; assisting = false; return@launch }
            }
            // Same framing as WorkspaceChatEditor.tsx: the client systemPrompt, appended last by the bridge.
            val system = """
                You are editing one workspace file of the agent "${bot.displayName}" on the Allternit platform.
                File: ${file.path}
                Revise the file according to the user instruction and return ONLY the complete new file content.
                Do not add explanations, do not wrap in markdown code fences, preserve the existing format unless the instruction says otherwise.

                --- CURRENT FILE CONTENT ---
                $content
                --- END CURRENT FILE CONTENT ---
            """.trimIndent()
            when (val r = BotTurn.stream(backend, rid, sid, bot.id, text, system, { stopped })) {
                is BotsResult.Ok -> if (!stopped) {
                    val next = stripCodeFence(r.value)
                    if (next.isEmpty()) assistError = "The model returned an empty response" else preview = next
                }
                is BotsResult.Failed -> assistError = r.message
            }
            instruction = ""
            assisting = false
        }
    }
    fun stop() {
        stopped = true
        job?.cancel()
        assisting = false
        sessionId?.let { sid -> scope.launch { backend.abort(sid) } }
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 10.dp).testTag("editWithChat"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Hairline()
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("✦", color = c.accentPrimary, fontSize = 13.sp)
            Text("Edit with chat", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            if (assisting) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
        }
        assistError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
        preview?.let { proposal ->
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).border(BorderStroke(1.dp, c.accentPrimary.copy(alpha = 0.35f)), RoundedCornerShape(12.dp)).testTag("editWithChatPreview")) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("Proposed content — review, then Apply", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.weight(1f))
                    Pill("Apply", "editWithChatApply", primary = true) { onApply(proposal); preview = null }
                    Icon(painterResource(DesignR.drawable.ph_x), "Discard", tint = c.textSecondary, modifier = Modifier.size(32.dp).clip(CircleShape).clickable { preview = null }.padding(8.dp).testTag("editWithChatDiscard"))
                }
                Hairline()
                Column(Modifier.fillMaxWidth().heightIn(max = 220.dp).verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(vertical = 4.dp)) {
                    lineDiff(content, proposal).forEach { (kind, line) ->
                        val bg = when (kind) { '+' -> AllternitStatic.StatusSuccess.copy(alpha = 0.12f); '-' -> AllternitStatic.StatusError.copy(alpha = 0.12f); else -> Color.Transparent }
                        val ink = when (kind) { '+' -> AllternitStatic.StatusSuccess; '-' -> AllternitStatic.StatusError; else -> c.textSecondary }
                        Text("$kind $line", color = ink, fontSize = 12.sp, fontFamily = FontFamily.Monospace, softWrap = false, modifier = Modifier.background(bg).padding(horizontal = 10.dp))
                    }
                }
            }
        }
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(12.dp)).padding(horizontal = 12.dp, vertical = 10.dp)) {
                if (instruction.isEmpty()) Text("Tell ${bot.displayName} how to change this file…", color = c.textTertiary, fontSize = 14.sp)
                BasicTextField(instruction, { instruction = it }, enabled = !assisting, textStyle = TextStyle(color = c.textPrimary, fontSize = 14.sp), cursorBrush = SolidColor(c.textPrimary), maxLines = 4, modifier = Modifier.fillMaxWidth().testTag("editWithChatInput"))
            }
            val canSend = instruction.isNotBlank()
            Icon(
                painterResource(if (assisting) com.allternit.core.design.R.drawable.ph_stop_fill else DesignR.drawable.ph_arrow_up),
                if (assisting) "Stop" else "Send",
                tint = Color.White,
                modifier = Modifier.size(36.dp).clip(CircleShape).background(if (assisting || canSend) c.accentPrimary else c.textTertiary)
                    .clickable(enabled = assisting || canSend) { if (assisting) stop() else send() }.padding(9.dp).testTag("editWithChatSend"),
            )
        }
        Spacer(Modifier.size(1.dp))
    }
}

/** Strip one wrapping markdown code fence, if the model added one (WorkspaceChatEditor stripCodeFence). */
internal fun stripCodeFence(text: String): String {
    val t = text.trim()
    if (!t.startsWith("```") || !t.endsWith("```")) return t
    val lines = t.split("\n")
    return if (lines.size >= 2 && lines.first().startsWith("```") && lines.last() == "```") lines.subList(1, lines.size - 1).joinToString("\n") else t
}

/** A line diff by longest common subsequence (iOS DiffLine.diffLines): ' ', '+' or '-' per line. */
internal fun lineDiff(before: String, after: String): List<Pair<Char, String>> {
    val a = before.split("\n")
    val b = after.split("\n")
    if (a.size * b.size > 4_000_000) return a.map { '-' to it } + b.map { '+' to it }
    val dp = Array(a.size + 1) { IntArray(b.size + 1) }
    for (i in a.indices.reversed()) for (j in b.indices.reversed()) {
        dp[i][j] = if (a[i] == b[j]) dp[i + 1][j + 1] + 1 else maxOf(dp[i + 1][j], dp[i][j + 1])
    }
    val out = mutableListOf<Pair<Char, String>>()
    var i = 0
    var j = 0
    while (i < a.size && j < b.size) {
        when {
            a[i] == b[j] -> { out.add(' ' to a[i]); i++; j++ }
            dp[i + 1][j] >= dp[i][j + 1] -> { out.add('-' to a[i]); i++ }
            else -> { out.add('+' to b[j]); j++ }
        }
    }
    while (i < a.size) out.add('-' to a[i++])
    while (j < b.size) out.add('+' to b[j++])
    return out
}
