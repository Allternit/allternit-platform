package com.allternit.feature.chat

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme

/** The five quick-check reasons (CONTRACT section 9), shown for Went wrong only. */
val TEST_CASE_REASONS = listOf("Wrong answer", "Broke a file", "Ignored instructions", "Too slow or costly", "Other")

/**
 * Save as test case (eval loop 1.20, mockups panel H): Went well | Went wrong, the reasons, a note,
 * Cancel and Save. A plan chat has no computer, so Save is off and the sheet says "Needs a computer".
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun SaveTestCaseSheet(chat: ChatViewModel, noComputer: Boolean, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var good by remember { mutableStateOf(false) }
    var reason by remember { mutableIntStateOf(0) }
    var note by remember { mutableStateOf("") }
    val state = chat.testCase
    DisposableEffect(Unit) { chat.resetTestCase(); onDispose { chat.resetTestCase() } }

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 24.dp).navigationBarsPadding().testTag("saveTestCaseSheet"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("Save as test case", color = c.textPrimary, fontSize = 22.sp, modifier = Modifier.semantics { heading() })
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(50)).border(1.dp, c.borderStrong, RoundedCornerShape(50)).testTag("testCaseLabel")) {
                listOf("Went well" to true, "Went wrong" to false).forEach { (text, isGood) ->
                    val on = good == isGood
                    Box(
                        Modifier.weight(1f).background(if (on) c.fillNeutral else c.bgPrimary)
                            .clickable(role = Role.RadioButton) { good = isGood }
                            .heightIn(min = 44.dp).testTag(if (isGood) "testCaseWell" else "testCaseWrong"),
                        contentAlignment = Alignment.Center,
                    ) { Text((if (on) "✓ " else "") + text, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium) }
                }
            }
            if (!good) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("What went wrong?", color = c.textSecondary, fontSize = 12.sp)
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        TEST_CASE_REASONS.forEachIndexed { i, text ->
                            val on = i == reason
                            Box(
                                Modifier.clip(RoundedCornerShape(8.dp))
                                    .background(if (on) c.fillNeutral else c.bgPrimary)
                                    .border(BorderStroke(1.dp, if (on) c.fillNeutral else c.borderStrong), RoundedCornerShape(8.dp))
                                    .clickable(role = Role.RadioButton) { reason = i }
                                    .heightIn(min = 36.dp).padding(horizontal = 12.dp).testTag("testCaseReason_$i"),
                                contentAlignment = Alignment.Center,
                            ) { Text((if (on) "✓ " else "") + text, color = c.textPrimary, fontSize = 14.sp) }
                        }
                    }
                }
            }
            OutlinedTextField(
                value = note, onValueChange = { note = it }, modifier = Modifier.fillMaxWidth().testTag("testCaseNote"),
                placeholder = { Text("What should have happened? (optional)", color = c.textTertiary) }, minLines = 2, maxLines = 4,
                enabled = state !is TestCaseState.Saving && state !is TestCaseState.Saved,
            )
            when {
                noComputer -> Text("Needs a computer", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.testTag("testCaseStatus"))
                state is TestCaseState.Saving -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp), modifier = Modifier.testTag("testCaseStatus")) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Saving…", color = c.textSecondary, fontSize = 14.sp)
                }
                state is TestCaseState.Saved -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("testCaseStatus")) {
                    Text("Saved", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                    if (state.needsCheck) Text(
                        "Needs a check", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.Medium,
                        modifier = Modifier.border(1.dp, c.borderStrong, RoundedCornerShape(50)).padding(horizontal = 8.dp, vertical = 2.dp),
                    )
                }
                state is TestCaseState.Failed -> Text(state.text, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.testTag("testCaseStatus"))
            }
            Row(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End), verticalAlignment = Alignment.CenterVertically) {
                val saved = state is TestCaseState.Saved
                CapsuleButton(if (saved) "Done" else "Cancel", modifier = Modifier.testTag("testCaseCancel"), onClick = onDismiss)
                if (!saved) CapsuleButton(
                    "Save", primary = true, enabled = !noComputer && state !is TestCaseState.Saving, modifier = Modifier.testTag("testCaseSave"),
                ) { chat.saveTestCase(good, if (good) null else TEST_CASE_REASONS[reason], note) }
            }
        }
    }
}
