package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch

/**
 * "Pick your main bot" from the Office's chief dock (step 27, iOS MainBotSetupSheet): the first-run flow's own Main step
 * ([MainStep]: A://, its looks, "Bots you already use", vendor / new / import) in a sheet, applied with the same
 * [MeetMainBot.applyChoice] the first-run finish uses. Reaches the Office through `BotsHost.mainBotPicker`.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun MainBotPickerSheet(onDismiss: () -> Unit, onDone: () -> Unit) {
    val c = setupColors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val model = remember { MeetYourBotsModel(context, scope) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(model) { model.loadRoster() }
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)

    ModalBottomSheet(onDismissRequest = { if (!saving) onDismiss() }, sheetState = sheet, containerColor = c.bg) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().testTag("mainBotPicker")) {
            when (val st = model.rosterState) {
                MeetYourBotsModel.RosterState.Loading -> Box(Modifier.fillMaxWidth().height(200.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = c.sub)
                }
                is MeetYourBotsModel.RosterState.Failed -> Column(Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("Couldn't load your bots. ${st.why}", color = c.sub, fontSize = 14.sp)
                    PickerButton("Try again", "mainBotPickerRetry", busy = false) { scope.launch { model.loadRoster() } }
                }
                MeetYourBotsModel.RosterState.Loaded -> {
                    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 4.dp)) {
                        MainStep(model)
                    }
                    error?.let {
                        Text(it, color = c.errFg, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp).semantics { liveRegion = LiveRegionMode.Polite })
                    }
                    Box(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp)) {
                        PickerButton(if (saving) "Saving…" else "Make ${model.leader} my main bot", "mainBotPickerSave", busy = saving) {
                            saving = true
                            error = null
                            scope.launch {
                                try {
                                    model.applyMainChoice()
                                    saving = false
                                    onDone()
                                } catch (e: kotlinx.coroutines.CancellationException) {
                                    throw e
                                } catch (e: Exception) {
                                    saving = false
                                    error = (e as? SetupError)?.message?.takeIf { it.isNotBlank() } ?: "Couldn't set your main bot. Try again."
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PickerButton(title: String, testTag: String, busy: Boolean, onClick: () -> Unit) {
    val c = setupColors
    Row(
        Modifier.fillMaxWidth().height(48.dp).clip(CircleShape).background(c.ink)
            .clickable(enabled = !busy, onClickLabel = title, role = Role.Button, onClick = onClick).testTag(testTag),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
    ) {
        if (busy) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = c.bg)
        Text(title, color = c.bg, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}
