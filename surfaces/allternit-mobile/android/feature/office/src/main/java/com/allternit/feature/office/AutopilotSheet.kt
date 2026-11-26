package com.allternit.feature.office

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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
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
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.InlineEmpty
import com.allternit.feature.bots.BotRecord
import kotlinx.coroutines.launch

/** "Draft only", "Ask first", "Send and tell me", "Act within limits" (the shared level copy, every host). */
internal fun autopilotLevelLabel(level: String) = when (level) {
    "draft" -> "Draft only"
    "tell" -> "Send and tell me"
    "limits" -> "Act within limits"
    else -> "Ask first"
}

internal fun autopilotNeedLabel(need: String) = when (need) {
    "suggestion" -> "Suggestions"
    "permission" -> "Permissions"
    else -> "Questions"
}

private fun levelDetail(level: String, main: String) = when (level) {
    "draft" -> "$main writes a proposed answer. Nothing is sent."
    "tell" -> "$main answers now. The digest and the thread record it."
    "limits" -> "$main answers on its own inside the limits below. Past a limit it asks first."
    else -> "Waits for you."
}

/**
 * Autopilot's state for the chief dock and its sheet (step 27): `GET|PUT /floor/autopilot` ([FloorClient]). The dock's
 * switch and the sheet's Save both go through [save], which always sends the whole rule set back.
 */
class AutopilotModel {
    var state by mutableStateOf<AutopilotState?>(null)
        private set
    var loading by mutableStateOf(false)
        private set
    /** Why the last load failed (signed out, offline or a message), or null. */
    var problem by mutableStateOf<String?>(null)
        private set
    var offline by mutableStateOf(false)
        private set
    var saving by mutableStateOf(false)
        private set

    suspend fun load() {
        loading = true
        when (val r = FloorClient.autopilot()) {
            is FloorCall.Ok -> { state = r.value; problem = null; offline = false }
            FloorCall.SignedOut -> { problem = "Sign in to use Autopilot."; offline = false }
            FloorCall.Offline -> { problem = "You're offline. Autopilot will load when you're back."; offline = true }
            is FloorCall.Failed -> { problem = r.message; offline = false }
        }
        loading = false
    }

    /** Saves [next]; null when it worked, else the message to show. */
    suspend fun save(next: AutopilotState): String? {
        saving = true
        val r = FloorClient.saveAutopilot(next)
        saving = false
        return when (r) {
            is FloorCall.Ok -> { state = r.value; problem = null; offline = false; null }
            FloorCall.SignedOut -> "Sign in again to save Autopilot."
            FloorCall.Offline -> "You're offline. Check your connection and try again."
            is FloorCall.Failed -> r.message
        }
    }

    suspend fun setEnabled(on: Boolean): String? = state?.let { save(it.copy(enabled = on)) } ?: problem
}

/** A rule while it is edited: the allowed actions stay as typed text until Save. */
private data class RuleDraft(val key: Long, val rule: AutopilotRule, val actionsText: String)

/** Checks the draft before it is sent, with the server's own copy (`autopilotErrorMessage`). */
private fun localProblem(rules: List<AutopilotRule>): String? {
    if (rules.size > 50) return autopilotErrorMessage("too_many_rules", null)
    val seen = HashSet<Triple<String, String?, String>>()
    rules.forEach { r ->
        if (r.scopeType != "all" && r.scopeId.isNullOrBlank()) return autopilotErrorMessage("scope_id_required", null)
        if (!seen.add(Triple(r.scopeType, if (r.scopeType == "all") null else r.scopeId, r.need))) return autopilotErrorMessage("duplicate_rule", null)
        if (r.level == "limits" && r.need != "question") {
            r.maxPerDay?.let { if (it !in 1..200) return autopilotErrorMessage("max_per_day_out_of_range", null) }
            if (r.allowedActions.size > 20) return autopilotErrorMessage("too_many_actions", null)
            if (r.allowedActions.any { it.length > 40 }) return autopilotErrorMessage("bad_action", null)
        }
        if ((r.note?.length ?: 0) > 200) return autopilotErrorMessage("note_too_long", null)
    }
    return null
}

/**
 * The Autopilot sheet (step 27, iOS AutopilotSheet), opened from the chief dock's info control: the switch, then one
 * row per rule (which bots, which need, the level; maxPerDay and allowed actions at "Act within limits"), Add a rule,
 * Save. Questions are fixed to "Draft only" until the server can answer them. Outbound sending stays in the bot's
 * own settings ([onOpenBotSettings]). A 400 shows as a plain message.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AutopilotSheet(
    model: AutopilotModel,
    mainBot: BotRecord?,
    bots: List<BotRecord>,
    pods: List<FloorPod>,
    onOpenBotSettings: ((String) -> Unit)?,
    onDismiss: () -> Unit,
) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val main = mainBot?.displayName ?: "Your main bot"
    LaunchedEffect(model) { if (model.state == null && !model.loading) model.load() }

    // Plain counter: rule keys are never read as state.
    val keys = remember { java.util.concurrent.atomic.AtomicLong(1L) }
    var enabled by remember(model.state) { mutableStateOf(model.state?.enabled ?: false) }
    var drafts by remember(model.state) {
        mutableStateOf(model.state?.rules.orEmpty().map { r -> RuleDraft(keys.getAndIncrement(), r, r.allowedActions.joinToString(", ")) })
    }
    var open by remember(model.state) { mutableStateOf<Long?>(null) }
    var error by remember { mutableStateOf<String?>(null) }

    fun update(key: Long, f: (RuleDraft) -> RuleDraft) { drafts = drafts.map { if (it.key == key) f(it) else it }; error = null }

    fun save() {
        val base = model.state ?: return
        val rules = drafts.map { d ->
            d.rule.copy(allowedActions = d.actionsText.split(',').map { it.trim() }.filter { it.isNotEmpty() }.distinct())
        }
        localProblem(rules)?.let { error = it; Haptics.perform(view, Haptics.Kind.Warning); return }
        scope.launch {
            val problem = model.save(base.copy(enabled = enabled, rules = rules))
            if (problem == null) { Haptics.perform(view, Haptics.Kind.Success); onDismiss() } else { error = problem; Haptics.perform(view, Haptics.Kind.Warning) }
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = c.bgPrimary, shape = RoundedCornerShape(topStart = 24.dp, topEnd = 24.dp)) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().imePadding().testTag("autopilotSheet")) {
            Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Autopilot", color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f).semantics { heading() })
                Box(
                    Modifier.size(44.dp).clip(CircleShape).clickable(onClickLabel = "Close", role = Role.Button, onClick = onDismiss)
                        .semantics { contentDescription = "Close" }.testTag("autopilotClose"),
                    contentAlignment = Alignment.Center,
                ) { Icon(painterResource(DesignR.drawable.ph_x), null, tint = c.textSecondary, modifier = Modifier.size(18.dp)) }
            }
            val st = model.state
            when {
                st == null && model.loading -> Box(Modifier.fillMaxWidth().height(160.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = c.textSecondary)
                }
                st == null -> Column(Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text(model.problem ?: "Autopilot couldn't load.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.testTag("autopilotLoadError"))
                    InkButton("Try again", "autopilotRetry") { scope.launch { model.load() } }
                }
                else -> {
                    Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text(
                            "When Autopilot is on, $main handles what these rules allow. Payments, signing, deletes and flagged threads always wait for you.",
                            color = c.textSecondary, fontSize = 13.5.sp, modifier = Modifier.padding(horizontal = 4.dp),
                        )
                        // The switch.
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.fillNeutral.copy(alpha = 0.6f))
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text("Autopilot", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                                Text(if (enabled) "On" else "Off. Every need asks you first.", color = c.textSecondary, fontSize = 12.5.sp)
                            }
                            Switch(
                                checked = enabled, onCheckedChange = { enabled = it; error = null },
                                colors = officeSwitchColors(),
                                modifier = Modifier.semantics { contentDescription = "Autopilot" }.testTag("autopilotSheetSwitch"),
                            )
                        }
                        Text("Rules", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 4.dp, top = 6.dp).semantics { heading() })
                        if (drafts.isEmpty()) {
                            InlineEmpty("No rules yet. Every need asks you first.", modifier = Modifier.padding(horizontal = 4.dp), art = EmptyArtKind.Sheets)
                        }
                        drafts.forEach { d ->
                            RuleRow(
                                d, main, bots, pods, expanded = open == d.key,
                                onToggle = { open = if (open == d.key) null else d.key },
                                onChange = { f -> update(d.key, f) },
                                onRemove = { drafts = drafts.filter { it.key != d.key }; error = null },
                            )
                        }
                        Row(
                            Modifier.clip(CircleShape).background(c.fillNeutral).clickable(role = Role.Button) {
                                // A new rule for all bots, on the first need no all-bots rule covers yet.
                                val taken = drafts.filter { it.rule.scopeType == "all" }.map { it.rule.need }.toSet()
                                val need = AUTOPILOT_NEEDS.firstOrNull { it !in taken } ?: "suggestion"
                                val scopeType = if (need in taken) "bot" else "all"
                                val d = RuleDraft(keys.getAndIncrement(), AutopilotRule(null, scopeType, if (scopeType == "bot") bots.firstOrNull()?.id else null, need, defaultAutopilotLevel(need)), "")
                                drafts = drafts + d; open = d.key; error = null
                            }.padding(horizontal = 14.dp, vertical = 9.dp).testTag("autopilotAddRule"),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
                        ) {
                            Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textPrimary, modifier = Modifier.size(14.dp))
                            Text("Add a rule", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                        }
                        // Sending rules live with the bot (D10); this app's place for them is the bot's settings.
                        if (mainBot != null && onOpenBotSettings != null) {
                            Row(
                                Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp))
                                    .clickable(onClickLabel = "Open ${mainBot.displayName}'s settings", role = Role.Button) { onDismiss(); onOpenBotSettings(mainBot.id) }
                                    .padding(14.dp).testTag("autopilotBotSettings"),
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Icon(painterResource(DesignR.drawable.ph_gear), null, tint = c.textSecondary, modifier = Modifier.size(18.dp))
                                Column(Modifier.weight(1f)) {
                                    Text("${mainBot.displayName}'s settings", color = c.textPrimary, fontSize = 14.5.sp, fontWeight = FontWeight.Medium)
                                    Text("Trust tier and how it works on its own", color = c.textSecondary, fontSize = 12.5.sp)
                                }
                                Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
                            }
                        }
                    }
                    error?.let {
                        Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp,
                            modifier = Modifier.padding(horizontal = 20.dp, vertical = 6.dp).testTag("autopilotError").semantics { liveRegion = LiveRegionMode.Polite })
                    }
                    Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp)) {
                        InkButton(if (model.saving) "Saving…" else "Save", "autopilotSave", busy = model.saving, modifier = Modifier.fillMaxWidth()) { save() }
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RuleRow(
    d: RuleDraft,
    main: String,
    bots: List<BotRecord>,
    pods: List<FloorPod>,
    expanded: Boolean,
    onToggle: () -> Unit,
    onChange: ((RuleDraft) -> RuleDraft) -> Unit,
    onRemove: () -> Unit,
) {
    val c = AllternitTheme.colors
    val r = d.rule
    val scopeName = when (r.scopeType) {
        "bot" -> r.scopeId?.let { id -> bots.firstOrNull { it.id == id }?.displayName } ?: "A bot"
        "pod" -> r.scopeId?.let { id -> pods.firstOrNull { it.id == id }?.name } ?: "A pod"
        else -> "All bots"
    }
    val level = if (r.need == "question") "draft" else r.level
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(16.dp)).testTag("autopilotRule"),
    ) {
        Row(
            Modifier.fillMaxWidth().clickable(onClickLabel = if (expanded) "Close the rule" else "Edit the rule", role = Role.Button, onClick = onToggle)
                .semantics { stateDescription = if (expanded) "Open" else "Closed" }
                .padding(horizontal = 14.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Column(Modifier.weight(1f)) {
                Text("$scopeName · ${autopilotNeedLabel(r.need)}", color = c.textPrimary, fontSize = 14.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(autopilotLevelLabel(level), color = c.textSecondary, fontSize = 12.5.sp)
            }
            Icon(painterResource(if (expanded) DesignR.drawable.ph_caret_up else DesignR.drawable.ph_caret_down), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
        }
        if (!expanded) return@Column
        Column(Modifier.fillMaxWidth().padding(start = 14.dp, end = 14.dp, bottom = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Caption("Which bots")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Chip("All bots", r.scopeType == "all", "autopilotScopeAll") { onChange { it.copy(rule = it.rule.copy(scopeType = "all", scopeId = null)) } }
                Chip("A bot", r.scopeType == "bot", "autopilotScopeBot") { onChange { it.copy(rule = it.rule.copy(scopeType = "bot", scopeId = bots.firstOrNull()?.id)) } }
                if (pods.isNotEmpty()) Chip("A pod", r.scopeType == "pod", "autopilotScopePod") { onChange { it.copy(rule = it.rule.copy(scopeType = "pod", scopeId = pods.firstOrNull()?.id)) } }
            }
            if (r.scopeType == "bot") FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                if (bots.isEmpty()) InlineEmpty("No bots yet.", art = EmptyArtKind.Gizzi)
                bots.forEach { b -> Chip(b.displayName, r.scopeId == b.id, "autopilotBot") { onChange { it.copy(rule = it.rule.copy(scopeId = b.id)) } } }
            }
            if (r.scopeType == "pod") FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                pods.forEach { p -> Chip(p.name.ifEmpty { "Pod" }, r.scopeId == p.id, "autopilotPod") { onChange { it.copy(rule = it.rule.copy(scopeId = p.id)) } } }
            }
            Caption("Which need")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                AUTOPILOT_NEEDS.forEach { n ->
                    Chip(autopilotNeedLabel(n), r.need == n, "autopilotNeed_$n") {
                        onChange {
                            // Questions are Draft only; leaving questions goes back to the default, Ask first.
                            val level = when { n == "question" -> "draft"; it.rule.need == "question" -> "ask"; else -> it.rule.level }
                            it.copy(rule = it.rule.copy(need = n, level = level))
                        }
                    }
                }
            }
            Caption("What $main does")
            if (r.need == "question") {
                Chip("Draft only", true, "autopilotLevel_draft") {}
                Text("Questions are draft only for now: $main writes an answer for you to send.", color = c.textTertiary, fontSize = 12.5.sp)
            } else {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    AUTOPILOT_LEVELS.forEach { l -> Chip(autopilotLevelLabel(l), r.level == l, "autopilotLevel_$l") { onChange { it.copy(rule = it.rule.copy(level = l)) } } }
                }
                Text(levelDetail(r.level, main), color = c.textTertiary, fontSize = 12.5.sp)
            }
            if (r.level == "limits" && r.need != "question") {
                OfficeField(
                    value = r.maxPerDay?.toString().orEmpty(), label = "Answers per day (1 to 200)", placeholder = "20", number = true, testTag = "autopilotMaxPerDay",
                ) { t -> onChange { it.copy(rule = it.rule.copy(maxPerDay = t.filter(Char::isDigit).take(3).toIntOrNull())) } }
                OfficeField(
                    value = d.actionsText, label = "Allowed actions, separated by commas", placeholder = "read, web_search", testTag = "autopilotActions",
                ) { t -> onChange { it.copy(actionsText = t.take(900)) } }
            }
            Box(
                Modifier.clip(CircleShape).clickable(onClickLabel = "Remove the rule", role = Role.Button, onClick = onRemove)
                    .padding(horizontal = 4.dp, vertical = 6.dp).testTag("autopilotRemoveRule"),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Icon(painterResource(DesignR.drawable.ph_trash), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(14.dp))
                    Text("Remove rule", color = AllternitStatic.StatusError, fontSize = 13.5.sp)
                }
            }
        }
    }
}

@Composable
private fun Caption(t: String) = Text(t, color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 4.dp))

@Composable
private fun Chip(label: String, on: Boolean, testTag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        Modifier.heightIn(min = 34.dp).clip(CircleShape).background(if (on) c.textPrimary else c.fillNeutral)
            .clickable(role = Role.RadioButton) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .semantics { selected = on }.padding(horizontal = 12.dp, vertical = 8.dp).testTag(testTag),
        contentAlignment = Alignment.Center,
    ) { Text(label, color = if (on) c.bgPrimary else c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

@Composable
private fun OfficeField(value: String, label: String, placeholder: String, testTag: String, number: Boolean = false, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    OutlinedTextField(
        value = value, onValueChange = onChange, singleLine = true,
        label = { Text(label, fontSize = 12.5.sp) }, placeholder = { Text(placeholder, color = c.textTertiary) },
        keyboardOptions = KeyboardOptions(keyboardType = if (number) KeyboardType.Number else KeyboardType.Text),
        shape = RoundedCornerShape(12.dp),
        colors = OutlinedTextFieldDefaults.colors(
            focusedTextColor = c.textPrimary, unfocusedTextColor = c.textPrimary, cursorColor = c.textPrimary,
            focusedBorderColor = c.textPrimary, unfocusedBorderColor = c.borderSubtle,
            focusedLabelColor = c.textSecondary, unfocusedLabelColor = c.textTertiary,
            focusedContainerColor = c.bgPrimary, unfocusedContainerColor = c.bgPrimary,
        ),
        modifier = Modifier.fillMaxWidth().testTag(testTag),
    )
}

/** The ink capsule button the office sheets use (the Bots Pill look). */
@Composable
internal fun InkButton(title: String, testTag: String, busy: Boolean = false, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        modifier.height(46.dp).clip(CircleShape).background(c.textPrimary)
            .clickable(enabled = !busy, onClickLabel = title, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .padding(horizontal = 18.dp).testTag(testTag),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
    ) {
        if (busy) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 1.5.dp, color = c.bgPrimary)
        Text(title, color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
    }
}

/** Switch colours on the white / graphite surfaces: ink when on, neutral when off (no accent tint). */
@Composable
internal fun officeSwitchColors() = AllternitTheme.colors.let { c ->
    SwitchDefaults.colors(
        checkedThumbColor = c.bgPrimary, checkedTrackColor = c.textPrimary, checkedBorderColor = c.textPrimary,
        uncheckedThumbColor = c.textTertiary, uncheckedTrackColor = c.fillNeutral, uncheckedBorderColor = c.borderStrong,
    )
}
