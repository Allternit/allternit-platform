package com.allternit.feature.office

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.bots.BotModeStore
import com.allternit.feature.bots.BotsResult
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.QuestionCard
import kotlinx.coroutines.launch

/**
 * The approval card docked at the top of the thread sheet (step 26, iOS OfficeApprovalCard): what the bot is waiting
 * on, with Approve / Not now (Answer for a question). Every reply goes through the app's existing calls, the ones the
 * chat's ask cards and the bot chat use: permissions `POST permissions/:id/reply` (once / reject), questions
 * `POST questions/:id/reply|reject` ([QuestionCard]), suggestions `POST threads/:id/approve|decline`
 * ([BotModeStore.act]). The frame never approves. [onDone] gets the need and, after an approval, the done bubble's text.
 */
@Composable
fun OfficeApprovalCard(
    need: OfficeNeed,
    botName: String,
    backend: ChatBackend,
    store: BotModeStore,
    more: Int,
    onDone: (need: OfficeNeed, bubble: String?) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    var busy by remember(need.id) { mutableStateOf<String?>(null) }
    var error by remember(need.id) { mutableStateOf<String?>(null) }
    var answering by remember(need.id) { mutableStateOf(false) }

    fun act(which: String, bubble: String?, call: suspend () -> String?) {
        busy = which
        error = null
        scope.launch {
            val problem = call()
            busy = null
            if (problem == null) {
                Haptics.perform(view, if (bubble != null) Haptics.Kind.Success else Haptics.Kind.Tap)
                onDone(need, bubble)
            } else {
                error = problem
            }
        }
    }
    fun ApiResult<*>.apiProblem(doing: String): String? = if (this is ApiResult.Ok) null else "Couldn't $doing. ${friendly(doing)}"
    fun BotsResult<*>.botsProblem(doing: String): String? = (this as? BotsResult.Failed)?.let { "Couldn't $doing. ${it.message}" }

    val (icon, header, title, detail) = when (need) {
        is OfficeNeed.Approval -> Quad(DesignR.drawable.ph_hand, "$botName needs your approval", need.ask.title, need.ask.patterns.firstOrNull { it.isNotEmpty() && it != "*" })
        is OfficeNeed.Question -> Quad(DesignR.drawable.ph_question, "$botName is asking", need.ask.questions.first().question, need.ask.questions.first().header)
        is OfficeNeed.Suggestion -> Quad(DesignR.drawable.ph_sparkle, "$botName suggests", need.thread.displayTitle.ifEmpty { "A new thread" }, need.thread.summary ?: need.thread.statusLine)
    }

    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel)
            .border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp))
            .padding(14.dp).testTag("officeApprovalCard")
            .semantics { liveRegion = LiveRegionMode.Polite },
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(painterResource(icon), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(14.dp))
            Text(header, color = c.textSecondary, fontSize = 12.5.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).semantics { heading() })
            if (more > 0) Text("+$more more", color = c.textTertiary, fontSize = 12.sp)
        }
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 3, overflow = TextOverflow.Ellipsis)
        detail?.takeIf { it.isNotBlank() }?.let {
            Text(it, color = c.textSecondary, fontSize = 13.sp, maxLines = 3, overflow = TextOverflow.Ellipsis,
                fontFamily = if (need is OfficeNeed.Approval) FontFamily.Monospace else null)
        }
        if (need is OfficeNeed.Question && answering) {
            // The chat's own question card (options, Send, Skip), answered through the same calls.
            QuestionCard(
                need.ask,
                onAnswer = { answers, onError ->
                    scope.launch {
                        when (val r = backend.replyQuestion(need.ask.id, answers)) {
                            is ApiResult.Ok -> { Haptics.perform(view, Haptics.Kind.Success); onDone(need, "Answered") }
                            else -> onError("Couldn't send your answer. ${r.friendly("answer")}")
                        }
                    }
                },
                onSkip = { onError ->
                    scope.launch {
                        when (val r = backend.rejectQuestion(need.ask.id)) {
                            is ApiResult.Ok -> onDone(need, null)
                            else -> onError("Couldn't skip it. ${r.friendly("skip")}")
                        }
                    }
                },
            )
        } else Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
            when (need) {
                is OfficeNeed.Approval -> {
                    CardButton("Not now", "officeCardNotNow", label = "Not now: ${need.ask.title}", enabled = busy == null, busy = busy == "no") {
                        act("no", null) { backend.replyPermission(need.ask.id, "reject").apiProblem("send your answer") }
                    }
                    CardButton("Approve", "officeCardApprove", primary = true, label = "Approve: ${need.ask.title}", enabled = busy == null, busy = busy == "yes") {
                        act("yes", "Approved: ${need.ask.title}".take(120)) { backend.replyPermission(need.ask.id, "once").apiProblem("approve it") }
                    }
                }
                is OfficeNeed.Question -> {
                    CardButton("Not now", "officeCardNotNow", label = "Not now: skip the question", enabled = busy == null, busy = busy == "no") {
                        act("no", null) { backend.rejectQuestion(need.ask.id).apiProblem("skip it") }
                    }
                    CardButton("Answer", "officeCardAnswer", primary = true, label = "Answer ${botName}'s question", enabled = busy == null) { answering = true }
                }
                is OfficeNeed.Suggestion -> {
                    CardButton("Not now", "officeCardNotNow", label = "Not now: ${need.thread.displayTitle}", enabled = busy == null, busy = busy == "no") {
                        act("no", null) { store.act(need.thread, "decline").botsProblem("close the suggestion") }
                    }
                    CardButton("Approve", "officeCardApprove", primary = true, label = "Approve: ${need.thread.displayTitle}", enabled = busy == null, busy = busy == "yes") {
                        act("yes", "Started: ${need.thread.displayTitle}".take(120)) { store.act(need.thread, "approve").botsProblem("start it") }
                    }
                }
            }
        }
        error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.5.sp, modifier = Modifier.testTag("officeCardError")) }
    }
}

private data class Quad(val icon: Int, val header: String, val title: String, val detail: String?)

/** The card's capsule button (the Bots Pill look: neutral fill, or ink for the primary). */
@Composable
private fun CardButton(title: String, testTag: String, primary: Boolean = false, label: String = title, enabled: Boolean = true, busy: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.height(40.dp).clip(CircleShape).background(if (primary) c.textPrimary else c.fillNeutral).testTag(testTag)
            .clickable(enabled = enabled && !busy, onClickLabel = label, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { this.contentDescription = label }
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        val ink = if (primary) c.bgPrimary else c.textPrimary
        if (busy) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = ink)
        Text(title, color = if (enabled) ink else c.textTertiary, fontSize = 14.5.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}
