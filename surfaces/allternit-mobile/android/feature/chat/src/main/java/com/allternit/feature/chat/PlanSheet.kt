package com.allternit.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme

/** The three answers a plan question takes (web `PlanActionKind`). */
enum class PlanAction { Approve, Revise, Ask }

/**
 * The reply label for a plan action. Approve sends the question's "Yes" (the first option that is not "No");
 * Revise and Ask a question both send "No" (keep planning), and the person's next message is the note or the
 * question (web `answerPlanQuestion`). Null when the question has no usable option.
 */
fun planReplyLabel(options: List<String>, action: PlanAction): String? =
    if (action == PlanAction.Approve) options.firstOrNull { it != "No" } ?: "Yes"
    else options.firstOrNull { it == "No" } ?: "No"

/** The plan card drawn in chat for a plan question: title, step count, "Open plan" (web plan bar; Artifacts redesign 6.7). */
@Composable
fun PlanCard(plan: PlanDraft, onOpen: () -> Unit) {
    val c = AllternitTheme.colors
    CardChrome(Modifier.testTag("planCard")) {
        CardHeader(com.allternit.core.design.R.drawable.ph_list_bullets, "Plan to approve")
        Text(plan.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("${plan.steps.size} ${if (plan.steps.size == 1) "step" else "steps"}", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f))
            CapsuleButton("Open plan", primary = true, modifier = Modifier.testTag("planCardOpen"), onClick = onOpen)
        }
    }
}

/**
 * Read-only plan: the steps, then the body as Markdown, with Approve, Revise and Ask a question at the bottom.
 * [onAction] answers the same pending question the chat card answers; it reports a failure through its callback
 * and the sheet shows it under the buttons.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PlanSheet(plan: PlanDraft, onDismiss: () -> Unit, onAction: (PlanAction, (String) -> Unit) -> Unit) {
    val c = AllternitTheme.colors
    var problem by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    fun act(a: PlanAction) {
        if (busy) return
        busy = true; problem = null
        onAction(a) { problem = it; busy = false }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).testTag("planSheet"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(plan.title, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
                    Text("${plan.steps.size} ${if (plan.steps.size == 1) "step" else "steps"} · read only", color = c.textTertiary, fontSize = 12.sp)
                }
                Text("Done", color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.heightIn(min = 44.dp).clickable(onClick = onDismiss).padding(8.dp))
            }
            Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                plan.steps.forEachIndexed { i, s ->
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        Box(Modifier.size(24.dp).clip(CircleShape).background(c.fillNeutral), contentAlignment = Alignment.Center) {
                            Text("${i + 1}", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                        }
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                            Text(s.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                            s.detail?.let { Text(it, color = c.textSecondary, fontSize = 14.sp) }
                        }
                    }
                }
                if (plan.body.isNotBlank()) MarkdownText(plan.body)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                CapsuleButton("Approve", primary = true, enabled = !busy, modifier = Modifier.testTag("planApprove")) { act(PlanAction.Approve) }
                CapsuleButton("Revise", enabled = !busy, modifier = Modifier.testTag("planRevise")) { act(PlanAction.Revise) }
                CapsuleButton("Ask a question", enabled = !busy, modifier = Modifier.testTag("planAsk")) { act(PlanAction.Ask) }
            }
            Text("Revise and Ask a question keep the plan open, then the message box opens for your note.", color = c.textTertiary, fontSize = 12.sp)
            problem?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 12.sp) }
            Box(Modifier.heightIn(min = 16.dp))
        }
    }
}

/**
 * Plan Revise / Ask a question: asks the chat composer (feature/shell Composer) to take focus and
 * show [String] as its prompt while empty (web ChatComposer PLAN_NOTE).
 */
object ComposerFocusRequests {
    val requests = kotlinx.coroutines.flow.MutableSharedFlow<String>(extraBufferCapacity = 1)
}
