package com.allternit.feature.bar

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.GlassPill
import com.allternit.core.design.Haptics
import com.allternit.core.design.PolishHaptic
import kotlinx.coroutines.launch

private val StrongOut = CubicBezierEasing(0.23f, 1f, 0.32f, 1f)
private val Mute = Color(0xFF8A8784)

/**
 * The translucent bar (mockups/B-android.jsx, plan D16): scrim .32 over what is behind, the bar card at the top
 * (26dp corners) slides 12dp down and fades in 240 ms; the step card below rises 8dp in 180 ms. Light by default.
 */
@Composable
fun BarScreen(model: BarViewModel, onClose: () -> Unit) {
    val view = LocalView.current
    val context = LocalContext.current
    val reduced = remember(context) {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    val scrim = remember { Animatable(if (reduced) .32f else 0f) }
    val enter = remember { Animatable(if (reduced) 1f else 0f) }
    LaunchedEffect(Unit) {
        model.open()
        if (!reduced) {
            kotlinx.coroutines.coroutineScope {
                launch { scrim.animateTo(.32f, tween(180, easing = CubicBezierEasing(0f, 0f, 0.2f, 1f))) }
                launch { enter.animateTo(1f, tween(240, easing = StrongOut)) }
            }
        }
    }
    LaunchedEffect(model.finishedTick) {
        if (model.finishedTick > 0) Haptics.perform(view, if (model.jobState == "completed") Haptics.Kind.Success else Haptics.Kind.Warning)
    }

    Box(
        Modifier.fillMaxSize().background(Color.Black.copy(alpha = scrim.value))
            .clickable(remember { MutableInteractionSource() }, indication = null) { onClose() },
    ) {
        Column(
            Modifier.fillMaxWidth().statusBarsPadding().imePadding().padding(horizontal = 10.dp, vertical = 8.dp)
                .graphicsLayer { alpha = enter.value; translationY = (enter.value - 1f) * 12.dp.toPx() }
                // Taps on the cards do not close the bar.
                .clickable(remember { MutableInteractionSource() }, indication = null) {},
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            BarPill(model)
            AnimatedContent(
                targetState = model.step,
                transitionSpec = {
                    (fadeIn(tween(180)) + slideInVertically(tween(180, easing = StrongOut)) { 8 }) togetherWith fadeOut(tween(120))
                },
                label = "barStep",
            ) { step ->
                Card {
                    model.errorText?.let { ErrorRow(it, model.noComputer) }
                    when (step) {
                        BarViewModel.Step.Apps -> AppsList(model)
                        BarViewModel.Step.Tools -> ToolsList(model)
                        BarViewModel.Step.Blanks -> BlanksCard(model)
                        BarViewModel.Step.Review -> ReviewCard(model)
                        BarViewModel.Step.Live, BarViewModel.Step.Done -> LiveCard(model, onClose)
                    }
                }
            }
        }
    }
}

/** The bar itself: the typed request before an app is picked, then the tool's sentence with its blanks. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun BarPill(model: BarViewModel) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.fillMaxWidth().shadow(12.dp, RoundedCornerShape(26.dp), ambientColor = Color.Black.copy(.10f), spotColor = Color.Black.copy(.18f))
            .clip(RoundedCornerShape(26.dp)).background(c.bgPrimary).padding(horizontal = 14.dp, vertical = 10.dp)
            .testTag("islandBar").semantics { contentDescription = BarWording.entry },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("A://", fontSize = 13.sp, fontWeight = FontWeight.Bold, color = c.textPrimary, modifier = Modifier.padding(end = 8.dp))
        val intent = model.intent
        if (intent != null && model.step != BarViewModel.Step.Apps && model.step != BarViewModel.Step.Tools) {
            FlowRow(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                intent.parts.forEach { part ->
                    when (part) {
                        is SentencePart.Text -> Text(part.text, fontSize = 14.sp, color = c.textPrimary, lineHeight = 24.sp)
                        is SentencePart.Blank -> {
                            val blank = intent.blanks.firstOrNull { it.key == part.key }
                            val value = model.values[part.key]
                            val focused = model.focusedBlank == part.key
                            Text(
                                value?.let { BarWording.display(it, blank) } ?: (blank?.label ?: part.key),
                                fontSize = 14.sp,
                                fontWeight = if (value != null) FontWeight.SemiBold else FontWeight.Normal,
                                color = if (value != null) c.textPrimary else Mute,
                                modifier = Modifier.padding(horizontal = 2.dp).clip(RoundedCornerShape(8.dp))
                                    .background(if (focused) c.textPrimary.copy(alpha = .10f) else c.textPrimary.copy(alpha = .05f))
                                    .clickable(enabled = model.step == BarViewModel.Step.Blanks) {
                                        PolishHaptic.perform(view, PolishHaptic.Kind.Selection)
                                        model.focusedBlank = part.key
                                    }
                                    .padding(horizontal = 6.dp, vertical = 1.dp)
                                    .testTag("islandBarBlank_${part.key}"),
                            )
                        }
                    }
                }
            }
        } else {
            Box(Modifier.weight(1f)) {
                if (model.query.isEmpty()) Text(BarWording.prompt, fontSize = 14.sp, color = Mute)
                BasicTextField(
                    value = model.query,
                    onValueChange = { model.query = it },
                    singleLine = true,
                    textStyle = TextStyle(fontSize = 14.sp, color = c.textPrimary),
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { model.matchingApps.firstOrNull()?.let(model::pickApp) }),
                    modifier = Modifier.fillMaxWidth().testTag("islandBarQuery"),
                )
            }
        }
    }
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().shadow(10.dp, RoundedCornerShape(22.dp), ambientColor = Color.Black.copy(.08f), spotColor = Color.Black.copy(.14f))
            .clip(RoundedCornerShape(22.dp)).background(AllternitTheme.colors.bgPrimary).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) { content() }
}

@Composable
private fun Header(text: String) = Text(text, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = AllternitTheme.colors.textPrimary)

@Composable
private fun Note(text: String) = Text(text, fontSize = 13.sp, color = AllternitTheme.colors.textSecondary)

@Composable
private fun LoadingRow(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = AllternitTheme.colors.textSecondary)
        Note(text)
    }
}

@Composable
private fun ErrorRow(text: String, noComputer: Boolean) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp), modifier = Modifier.testTag("islandBarError")) {
        Text(text, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = Color(0xFFC4413A))
        if (noComputer) Text(BarWording.noComputerDetail, fontSize = 12.sp, color = AllternitTheme.colors.textSecondary)
    }
}

@Composable
private fun PickRow(title: String, detail: String?, enabled: Boolean = true, selected: Boolean = false, tag: String? = null, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp))
            .background(if (selected) c.textPrimary.copy(alpha = .06f) else Color.Transparent)
            .clickable(enabled = enabled) { PolishHaptic.perform(view, PolishHaptic.Kind.Light); onClick() }
            .padding(horizontal = 10.dp, vertical = 10.dp)
            .let { if (tag != null) it.testTag(tag) else it },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, fontSize = 14.sp, color = if (enabled) c.textPrimary else c.textSecondary, modifier = Modifier.weight(1f))
        detail?.let { Text(it, fontSize = 12.sp, color = c.textSecondary) }
    }
}

@Composable
private fun AppsList(model: BarViewModel) {
    when {
        model.loading && model.apps.isEmpty() -> LoadingRow(BarWording.loadingApps)
        model.apps.isEmpty() && !model.noComputer && model.errorText == null -> Note(BarWording.noApps)
        model.apps.isNotEmpty() -> {
            Header(BarWording.whichApp)
            Column(Modifier.heightIn(max = 280.dp).verticalScroll(rememberScrollState())) {
                model.matchingApps.forEach { app ->
                    PickRow(app.name, BarWording.subtitle(app), enabled = app.isRunnable, tag = "islandBarApp_${app.id}") { model.pickApp(app) }
                }
            }
        }
    }
}

@Composable
private fun ToolsList(model: BarViewModel) {
    Header(BarWording.whatInApp(model.app?.name))
    Column(Modifier.heightIn(max = 280.dp).verticalScroll(rememberScrollState())) {
        model.tools.forEach { tool ->
            PickRow(tool.title, if (tool.readOnly) "Reads only" else null, tag = "islandBarTool_${tool.name}") { model.pickTool(tool) }
        }
    }
    Buttons(back = model::back)
}

@Composable
private fun BlanksCard(model: BarViewModel) {
    val intent = model.intent ?: return
    val blank = model.focusedBlank?.let { k -> intent.blanks.firstOrNull { it.key == k } }
    if (blank != null) {
        Header(BarWording.question(blank))
        BlankPicker(blank, intent.recents[blank.key].orEmpty(), model.values[blank.key], intent.app.name) { model.set(it, blank.key) }
    } else if (intent.blanks.isEmpty()) {
        Note(BarWording.nothingToFill)
    }
    Buttons(back = model::back, primary = "Review", primaryEnabled = model.canReview && !model.loading, loading = model.loading, tag = "islandBarReview") {
        model.goReview()
    }
}

/** One picker per blank kind (iOS IslandBlankPicker): recents, then choices / yes-no / a typed field. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun BlankPicker(blank: BarBlank, recents: List<BarRecent>, value: BarValue?, appName: String, onPick: (BarValue?) -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    if (blank.pickerKind == "mcp_app" || blank.type == "details") {
        Note(BarWording.ownForm(appName))
        return
    }
    if (recents.isNotEmpty() && blank.type != "boolean") {
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp),
            modifier = Modifier.semantics { contentDescription = "Recent values" }) {
            recents.forEach { r ->
                GlassPill(onClick = { onPick(r.value) }, height = 32.dp) { Text(r.label, fontSize = 13.sp, color = c.textPrimary) }
            }
        }
    }
    when (blank.type) {
        "enum" -> Column {
            blank.options.forEach { o ->
                val picked = (value as? BarValue.Text)?.value == o.value
                PickRow(o.label, if (picked) "✓" else null, selected = picked) {
                    PolishHaptic.perform(view, PolishHaptic.Kind.Selection)
                    onPick(BarValue.Text(o.value))
                }
            }
        }
        "boolean" -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            listOf(true, false).forEach { b ->
                GlassPill(onClick = { onPick(BarValue.Bool(b)) }, height = 36.dp) {
                    Text(if (b) "Yes" else "No", fontSize = 14.sp, color = c.textPrimary,
                        fontWeight = if ((value as? BarValue.Bool)?.value == b) FontWeight.SemiBold else FontWeight.Normal)
                }
            }
        }
        else -> {
            var text by remember(blank.key) { mutableStateOf(value?.display.orEmpty()) }
            val commit = { onPick(BarWording.parse(text, blank)) }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Box(Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 10.dp)) {
                    if (text.isEmpty()) Text(blank.unit ?: blank.label, fontSize = 14.sp, color = Mute)
                    BasicTextField(
                        value = text,
                        onValueChange = { text = it },
                        singleLine = true,
                        textStyle = TextStyle(fontSize = 14.sp, color = c.textPrimary),
                        keyboardOptions = KeyboardOptions(
                            keyboardType = when (blank.type) {
                                "number" -> KeyboardType.Decimal
                                "integer" -> KeyboardType.Number
                                "email" -> KeyboardType.Email
                                "uri" -> KeyboardType.Uri
                                else -> KeyboardType.Text
                            },
                            imeAction = ImeAction.Done,
                        ),
                        keyboardActions = KeyboardActions(onDone = { commit() }),
                        modifier = Modifier.fillMaxWidth().testTag("islandBarField_${blank.key}"),
                    )
                }
                GlassPill(onClick = { commit() }, height = 40.dp) { Text("Set", fontSize = 14.sp, color = c.textPrimary) }
            }
        }
    }
}

@Composable
private fun ReviewCard(model: BarViewModel) {
    val review = model.review ?: return
    val c = AllternitTheme.colors
    Header(model.intent?.let { "${it.toolTitle} in ${it.app.name}" } ?: "Review")
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        review.lines.forEach { line -> LineRow(line.label, line.value) }
        if (review.level.isNotEmpty()) LineRow("Level", BarWording.level(review.level))
    }
    val approverLine = BarWording.approverLine(review)
    if (review.ruleText != null || approverLine != null) {
        Column(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(9.dp)).background(c.textPrimary.copy(alpha = .06f)).padding(8.dp)
                .testTag("islandBarRule"),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            review.ruleText?.let { Text(it, fontSize = 12.sp, color = c.textPrimary) }
            approverLine?.let { Text(it, fontSize = 12.sp, color = c.textSecondary) }
        }
    }
    val view = LocalView.current
    Buttons(back = model::back, primary = BarWording.runLabel(review), primaryEnabled = !model.loading, loading = model.loading, tag = "islandBarRun") {
        PolishHaptic.perform(view, PolishHaptic.Kind.Medium)
        model.run()
    }
}

@Composable
private fun LineRow(label: String, value: String) {
    Row(Modifier.fillMaxWidth()) {
        Text(label, fontSize = 13.sp, color = AllternitTheme.colors.textSecondary, modifier = Modifier.weight(1f))
        Text(value, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = AllternitTheme.colors.textPrimary)
    }
}

@Composable
private fun LiveCard(model: BarViewModel, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val state = model.jobState ?: "working"
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            model.stateLine, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.textPrimary,
            modifier = Modifier.testTag(if (model.step == BarViewModel.Step.Done) "islandBarReceipt" else "islandBarState"),
        )
        if (model.step == BarViewModel.Step.Live) {
            model.progressLine?.let { Text(it, fontSize = 12.sp, color = c.textSecondary, maxLines = 2) }
            if (state == "input_required") Text(BarWording.answerInTasks, fontSize = 12.sp, color = c.textSecondary)
        }
    }
    StepLine(state)
    if (model.step == BarViewModel.Step.Live) {
        Buttons(back = null, secondary = "Stop" to model::stop, primary = "Hide", primaryEnabled = true) { onClose() }
    } else {
        Buttons(back = null, secondary = "New" to model::reset, primary = "Done", primaryEnabled = true) { model.reset(); onClose() }
    }
}

/** Asked, Approval, Running, Done (the notification's step line). */
@Composable
private fun StepLine(state: String) {
    val reached = BarWording.stepsReached(state)
    val tint = when (state) {
        "waiting_approval" -> Color(0xFFE08A2E)
        "completed" -> Color(0xFF2E9E5B)
        "failed", "cancelled" -> Color(0xFFC4413A)
        else -> Color(0xFF3B7BE0)
    }
    Row(Modifier.fillMaxWidth().semantics { contentDescription = "Step $reached of 4" }, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        repeat(4) { i ->
            Box(Modifier.weight(1f).padding(vertical = 2.dp).clip(RoundedCornerShape(2.dp)).height(4.dp).background(if (i < reached) tint else tint.copy(alpha = .2f)))
        }
    }
}

@Composable
private fun Buttons(
    back: (() -> Unit)?,
    secondary: Pair<String, () -> Unit>? = null,
    primary: String? = null,
    primaryEnabled: Boolean = true,
    loading: Boolean = false,
    tag: String? = null,
    onPrimary: () -> Unit = {},
) {
    val c = AllternitTheme.colors
    Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        back?.let { GlassPill(onClick = it, height = 36.dp) { Text("Back", fontSize = 14.sp, color = c.textPrimary) } }
        secondary?.let { (label, action) -> GlassPill(onClick = action, height = 36.dp) { Text(label, fontSize = 14.sp, color = c.textPrimary) } }
        Spacer(Modifier.weight(1f))
        if (primary != null) {
            Box(
                Modifier.clip(RoundedCornerShape(18.dp)).background(if (primaryEnabled) c.textPrimary else c.textPrimary.copy(alpha = .3f))
                    .clickable(enabled = primaryEnabled) { onPrimary() }
                    .padding(horizontal = 16.dp, vertical = 9.dp)
                    .let { if (tag != null) it.testTag(tag) else it },
                contentAlignment = Alignment.Center,
            ) {
                if (loading) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.bgPrimary)
                else Text(primary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, color = c.bgPrimary)
            }
        }
    }
}
