package com.allternit.feature.bots

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.Motion
import com.allternit.core.design.R as DesignR
import com.allternit.core.design.rememberReducedMotion
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive

// Team map sheet (Island bar + Team map, lane 38): a port of iOS Features/Office/TeamMap (platform #3038).
// Same rows, same words. Flag `team.map` (FeatureFlags.TEAM_MAP), off by default.

private sealed interface TeamMapRoute {
    data class Node(val id: String) : TeamMapRoute
    data class NewRule(val scope: String) : TeamMapRoute
    /** The same editor on an existing rule, prefilled. */
    data class EditRule(val id: String, val scope: String) : TeamMapRoute
}

private object TeamMapPalette {
    val waiting = AllternitStatic.StatusWarning
    val working = AllternitStatic.StatusInfo
    @Composable fun line(): Color = AllternitTheme.colors.borderStrong
    @Composable fun ring(state: String, waiting: Boolean): Color = when {
        waiting || state == "waiting_approval" || state == "needs_input" -> this.waiting
        state == "working" -> working
        else -> line()
    }
}

/**
 * The Team map as a sheet. [focus] opens straight on a node: `p:<user>`, `b:<agent>` or `v:<vendor bot>`
 * (a `b:` id that the map lists as `v:` is matched too, like iOS). [onAddBot] shows "Add a bot" on the empty map.
 */
@Composable
fun TeamMapSheet(focus: String? = null, onAddBot: (() -> Unit)? = null, onDismiss: () -> Unit) {
    val model = remember { TeamMapViewModel() }
    val stack = remember { mutableStateListOf<TeamMapRoute>() }
    var focused by remember { mutableStateOf(false) }
    val reduced = rememberReducedMotion()
    LaunchedEffect(model) {
        model.load()
        val map = model.map
        if (!focused && focus != null && map != null) {
            focused = true
            val id = when {
                map.node(focus) != null -> focus
                focus.startsWith("b:") && map.node("v:" + focus.drop(2)) != null -> "v:" + focus.drop(2)
                else -> null
            }
            id?.let { stack.add(TeamMapRoute.Node(it)) }
        }
    }
    val top = stack.lastOrNull()
    val title = when (top) {
        null -> "Team map"
        is TeamMapRoute.Node -> model.map?.name(top.id) ?: "Team map"
        is TeamMapRoute.NewRule -> "New rule"
        is TeamMapRoute.EditRule -> "Edit rule"
    }
    LibrarySheet(
        title = title,
        testTag = "teamMapSheet",
        onDismiss = onDismiss,
        leading = if (stack.isEmpty()) null else LibrarySheetAction("Back", testTag = "teamMapBack") { stack.removeAt(stack.lastIndex) },
        trailing = LibrarySheetAction("Close", bold = true, testTag = "teamMapClose", onClick = onDismiss),
    ) {
        Box(Modifier.fillMaxWidth().fillMaxHeight(0.92f).imePadding()) {
            AnimatedContent(
                targetState = top,
                transitionSpec = {
                    if (reduced) fadeIn(androidx.compose.animation.core.snap()) togetherWith fadeOut(androidx.compose.animation.core.snap())
                    else fadeIn(Motion.standardSpring()) togetherWith fadeOut(Motion.standardSpring())
                },
                label = "teamMapRoute",
            ) { route ->
                when (route) {
                    null -> TeamMapRootContent(model, onOpen = { stack.add(TeamMapRoute.Node(it)) }, onNewRule = { stack.add(TeamMapRoute.NewRule(it)) },
                        onAddBot = onAddBot?.let { add -> { onDismiss(); add() } })
                    is TeamMapRoute.Node -> TeamNodeDetail(model, route.id, onOpen = { stack.add(TeamMapRoute.Node(it)) }, onNewRule = { stack.add(TeamMapRoute.NewRule(it)) },
                        onEditRule = { id, scope -> stack.add(TeamMapRoute.EditRule(id, scope)) })
                    is TeamMapRoute.NewRule -> TeamRuleEditor(model, route.scope, null, onSaved = { stack.removeAt(stack.lastIndex) })
                    is TeamMapRoute.EditRule -> TeamRuleEditor(model, route.scope, route.id, onSaved = { stack.removeAt(stack.lastIndex) })
                }
            }
        }
    }
}

// MARK: - Map

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TeamMapRootContent(model: TeamMapViewModel, onOpen: (String) -> Unit, onNewRule: (String) -> Unit, onAddBot: (() -> Unit)?) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }
    var ruleError by remember { mutableStateOf<String?>(null) }
    when (val phase = model.phase) {
        TeamMapViewModel.Phase.Loading -> Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
            CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
            Text("Loading the team map…", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(top = 12.dp))
        }
        is TeamMapViewModel.Phase.Failed -> TeamMapFailed(phase.error) { scope.launch { model.load() } }
        TeamMapViewModel.Phase.Loaded -> {
            val map = model.map ?: return
            if (map.botCount == 0) { TeamMapEmpty(map, onAddBot); return }
            PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; model.load(); refreshing = false } }, modifier = Modifier.fillMaxSize()) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp).testTag("teamMap"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("${map.orgName ?: "Just you"} · ${teamCount(map.peopleCount, "person", "people")} · ${teamCount(map.botCount, "bot")}", color = c.textSecondary, fontSize = 13.sp)
                        if (map.asks.isNotEmpty()) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            Icon(painterResource(DesignR.drawable.ph_hourglass), null, tint = TeamMapPalette.waiting, modifier = Modifier.size(14.dp))
                            Text("${map.asks.size} waiting", color = TeamMapPalette.waiting, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                        }
                        map.partialNote?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
                    }
                    Box(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) { TeamTree(map, onOpen) }
                    Text("Circles are people, squares are bots. Amber dashed lines wait on someone.", color = c.textTertiary, fontSize = 12.sp)
                    if (map.asks.isNotEmpty()) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        TeamSectionTitle("Asks (${map.asks.size} waiting)")
                        map.asks.forEach { ask ->
                            TeamAskRow(ask, map, onDecide = { model.decide(ask, it) }, onWhoApproves = ask.approverUserId?.let { u -> { onOpen("p:$u") } })
                        }
                    }
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Box(Modifier.weight(1f)) { TeamSectionTitle(teamCount(map.rules.size, "rule")) }
                            if (map.viewerIsAdmin) TeamTextButton("New rule", DesignR.drawable.ph_plus, "teamMapNewRule") { onNewRule("org") }
                        }
                        if (map.rules.isEmpty()) Text("No rules yet. Each bot's level decides what it may do.", color = c.textSecondary, fontSize = 13.sp)
                        map.rules.forEach { rule ->
                            TeamRuleRow(rule, onDelete = if (map.viewerIsAdmin) ({ ruleError = null; scope.launch { ruleError = model.deleteRule(rule.id) } }) else null)
                        }
                        ruleError?.let { TeamErrorText(it) }
                    }
                    if (map.jobs.isEmpty()) Text("Jobs not available yet.", color = c.textTertiary, fontSize = 12.sp)
                }
            }
        }
    }
}

/** People and bots as a tree; reports-to / owns lines behind it, amber dashed lines for a bot waiting on someone. */
@Composable
private fun TeamTree(map: TeamMap, onOpen: (String) -> Unit) {
    val rects = remember(map) { mutableStateMapOf<String, Rect>() }
    var origin by remember { mutableStateOf(Offset.Zero) }
    val line = TeamMapPalette.line()
    Row(
        Modifier
            .onGloballyPositioned { origin = it.positionInRoot() }
            .drawBehind {
                val tree = Path(); val waiting = Path()
                fun r(id: String) = rects[id]?.translate(-origin)
                for (node in map.nodes) {
                    val child = r(node.id) ?: continue
                    val waitsOn = if (node.isBot) map.waitingOn(node.id) else null
                    val parentId = map.parent(node.id)
                    val parent = parentId?.let { r(it) }
                    if (parent != null) {
                        val start = Offset(parent.center.x, parent.bottom + 4.dp.toPx())
                        val end = Offset(child.center.x, child.top - 4.dp.toPx())
                        val midY = (start.y + end.y) / 2
                        val p = if (waitsOn == parentId) waiting else tree
                        p.moveTo(start.x, start.y); p.lineTo(start.x, midY); p.lineTo(end.x, midY); p.lineTo(end.x, end.y)
                    }
                    if (waitsOn != null && waitsOn != parentId) r(waitsOn)?.let { target ->
                        waiting.moveTo(child.center.x, child.top); waiting.lineTo(target.center.x, target.bottom)
                    }
                }
                drawPath(tree, line, style = Stroke(width = 1.5.dp.toPx()))
                drawPath(waiting, TeamMapPalette.waiting, style = Stroke(width = 2.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(5.dp.toPx(), 4.dp.toPx()))))
            }
            .semantics { contentDescription = "Team map" },
        horizontalArrangement = Arrangement.spacedBy(20.dp),
        verticalAlignment = Alignment.Top,
    ) {
        map.roots.forEach { TeamSubtree(map, it, 0, rects, onOpen) }
    }
}

@Composable
private fun TeamSubtree(map: TeamMap, node: TeamNode, depth: Int, rects: MutableMap<String, Rect>, onOpen: (String) -> Unit) {
    val kids = if (depth < 12) map.children(node.id) else emptyList()
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(28.dp)) {
        TeamNodeButton(map, node, Modifier.onGloballyPositioned { rects[node.id] = it.boundsInRoot() }) { onOpen(node.id) }
        if (kids.isNotEmpty()) Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.Top) {
            kids.forEach { TeamSubtree(map, it, depth + 1, rects, onOpen) }
        }
    }
}

@Composable
private fun TeamNodeButton(map: TeamMap, node: TeamNode, modifier: Modifier, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val waitingOnMe = map.waitingOnMe(node.id).size
    val badge = if (node.isBot) node.asks else maxOf(waitingOnMe, map.asks(node).count { it.actor != node.id })
    val waits = node.isBot && map.waitingOn(node.id) != null
    val state = map.stateText(node)
    val label = listOfNotNull(if (node.id == map.me) "${node.name}, you" else node.name, node.subtitle, state, if (waitingOnMe > 0) "$waitingOnMe waiting on them" else null).joinToString(", ")
    Column(
        modifier.widthIn(min = 92.dp, max = 132.dp).clip(RoundedCornerShape(12.dp))
            .clickable(role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .clearAndSetSemantics { contentDescription = "$label. Shows levels, who they report to, and rules"; onClick { onClick(); true } }
            .testTag("teamNode${node.id}")
            .padding(4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        TeamNodeAvatar(node, TeamMapPalette.ring(node.state, waits || waitingOnMe > 0), badge)
        Text(if (node.id == map.me) "${node.name} (you)" else node.name, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 2, textAlign = TextAlign.Center, overflow = TextOverflow.Ellipsis)
        Text(node.subtitle, color = c.textSecondary, fontSize = 11.sp, maxLines = 2, textAlign = TextAlign.Center, overflow = TextOverflow.Ellipsis)
        if (node.isBot || node.state != "idle") Text(
            state, fontSize = 11.sp, textAlign = TextAlign.Center, maxLines = 2,
            color = if (waits) TeamMapPalette.waiting else if (node.state == "working") TeamMapPalette.working else c.textTertiary,
        )
    }
}

@Composable
private fun TeamNodeAvatar(node: TeamNode, ring: Color = TeamMapPalette.line(), badge: Int = 0) {
    val c = AllternitTheme.colors
    val shape = if (node.isBot) RoundedCornerShape(11.dp) else CircleShape
    Box(Modifier.size(46.dp).clearAndSetSemantics {}) {
        Box(Modifier.size(40.dp).align(Alignment.Center).clip(shape).background(c.fillNeutral).border(BorderStroke(2.dp, ring), shape), contentAlignment = Alignment.Center) {
            if (node.isBot) Icon(painterResource(DesignR.drawable.ph_cpu), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
            else Text(node.initials, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        }
        if (badge > 0) Text(
            "$badge", color = Color.White, fontSize = 10.sp, fontWeight = FontWeight.Bold,
            modifier = Modifier.align(Alignment.TopEnd).offset(x = 2.dp, y = (-2).dp).clip(CircleShape).background(TeamMapPalette.waiting).padding(horizontal = 5.dp, vertical = 1.dp),
        )
    }
}

@Composable
private fun TeamMapFailed(error: TeamMapError, retry: () -> Unit) {
    val c = AllternitTheme.colors
    val title = when (error) {
        TeamMapError.NoRuntime -> "Connect a computer to see your team"
        TeamMapError.SignedOut -> "Sign in to see your team"
        else -> "Couldn't load the team map"
    }
    val icon = when (error) {
        TeamMapError.NoRuntime -> DesignR.drawable.ph_desktop
        TeamMapError.SignedOut -> DesignR.drawable.ph_identification_card
        else -> DesignR.drawable.ph_warning
    }
    Box(Modifier.fillMaxSize().padding(20.dp).testTag("teamMapFailed"), contentAlignment = Alignment.Center) {
        FriendlyState(
            style = if (error == TeamMapError.NoRuntime) FriendlyStyle.Offline else FriendlyStyle.Error,
            title = title, message = error.text,
            actionTitle = if (error == TeamMapError.SignedOut) null else "Try again",
            onAction = if (error == TeamMapError.SignedOut) null else retry,
        ) { Icon(painterResource(icon), null, tint = c.textSecondary) }
    }
}

@Composable
private fun TeamMapEmpty(map: TeamMap, onAddBot: (() -> Unit)?) {
    val c = AllternitTheme.colors
    val me = map.viewer ?: TeamNode(map.me, "person", "You", "owner", null, null, "idle", 0, 0)
    val others = map.peopleCount - (if (map.viewer == null) 0 else 1)
    val line = TeamMapPalette.line()
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp, vertical = 32.dp).testTag("teamMapEmpty"),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp)) {
            TeamNodeAvatar(me)
            Text("You", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text(me.subtitle, color = c.textSecondary, fontSize = 12.sp)
        }
        Box(Modifier.width(1.5.dp).heightIn(min = 24.dp).background(line).clearAndSetSemantics {})
        Box(Modifier.size(width = 120.dp, height = 56.dp).clearAndSetSemantics {}.drawBehind {
            drawRoundRect(line, cornerRadius = androidx.compose.ui.geometry.CornerRadius(12.dp.toPx()),
                style = Stroke(width = 1.5.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx()))))
        })
        Text(if (others > 0) "No bots yet: add one to see the map" else "Just you: add a bot to see the map",
            color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center, modifier = Modifier.semantics { heading() })
        Text("Bots you create or connect appear under you, with the apps they can use and who approves what.", color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
        onAddBot?.let { TeamFilledButton("Add a bot", "teamMapAddBot", enabled = true, onClick = it) }
    }
}

// MARK: - Rows

@Composable
private fun TeamSectionTitle(text: String) {
    Text(text, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
}

@Composable
private fun TeamErrorText(text: String) = Text(text, color = AllternitStatic.StatusError, fontSize = 12.sp)

@Composable
private fun TeamTextButton(title: String, icon: Int?, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Row(
        Modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .testTag(tag).padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        icon?.let { Icon(painterResource(it), null, tint = c.accentPrimary, modifier = Modifier.size(16.dp)) }
        Text(title, color = if (enabled) c.accentPrimary else c.textTertiary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun TeamFilledButton(title: String, tag: String, enabled: Boolean, modifier: Modifier = Modifier, prominent: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    Box(
        modifier.heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp))
            .background(if (prominent) c.accentPrimary.copy(alpha = if (enabled) 1f else 0.4f) else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button) { Haptics.perform(view, Haptics.Kind.Tap); onClick() }
            .testTag(tag).padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = if (prominent) Color.White else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
}

/** One rule line: lock + "Always on" for a built-in rule, else On/Off. With [onEdit] the sentence opens the editor; with [onToggle] On/Off is a switch (owner/admin only; locked rules never get either). */
@Composable
private fun TeamRuleRow(rule: TeamRule, onDelete: (() -> Unit)? = null, onEdit: (() -> Unit)? = null, onToggle: ((Boolean) -> Unit)? = null) {
    val c = AllternitTheme.colors
    val status = if (rule.locked) "Always on" else if (rule.enabled) "On" else "Off"
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(horizontal = 12.dp, vertical = 8.dp).testTag("teamRule${rule.id}"),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val canEdit = onEdit != null && !rule.locked
        Row(
            Modifier.weight(1f).heightIn(min = 44.dp)
                .then(if (canEdit) Modifier.clickable(role = Role.Button, onClick = onEdit!!) else Modifier)
                .clearAndSetSemantics {
                    contentDescription = if (canEdit) "${rule.text}. $status. Edit this rule" else "${rule.text}. $status"
                    if (canEdit) onClick(label = "Edit this rule") { onEdit!!(); true }
                }
                .testTag("teamRuleEdit${rule.id}"),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(painterResource(if (rule.locked) DesignR.drawable.ph_lock_fill else DesignR.drawable.ph_shield_check), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            Text(rule.text, color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
            if (onToggle == null || rule.locked) Text(status, color = c.textSecondary, fontSize = 12.sp)
        }
        if (onToggle != null && !rule.locked) Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) {
            Switch(
                checked = rule.enabled, onCheckedChange = onToggle,
                modifier = Modifier.semantics { contentDescription = "Rule $status. Turns this rule on or off"; stateDescription = status }.testTag("teamRuleToggle${rule.id}"),
            )
        }
        if (onDelete != null && !rule.locked) Box(
            Modifier.size(44.dp).clip(CircleShape).clickable(role = Role.Button, onClick = onDelete)
                .semantics { contentDescription = "Delete rule" }.testTag("teamRuleDelete${rule.id}"),
            contentAlignment = Alignment.Center,
        ) { Icon(painterResource(DesignR.drawable.ph_trash), null, tint = AllternitStatic.StatusError, modifier = Modifier.size(18.dp)) }
    }
}

@Composable
private fun TeamAskRow(ask: TeamAsk, map: TeamMap, onDecide: (suspend (Boolean) -> String?)?, onWhoApproves: (() -> Unit)?) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var busy by remember(ask.id) { mutableStateOf(false) }
    var error by remember(ask.id) { mutableStateOf<String?>(null) }
    val headline = map.name(ask.actor)?.let { "${ask.title} · $it" } ?: ask.title
    val requester = ask.actor?.let { actor ->
        if (actor.startsWith("p:")) map.name(actor)
        else { val bot = map.name(actor) ?: "a bot"; map.requesterName(ask)?.let { "$it, through $bot" } ?: bot }
    }
    fun decide(approve: Boolean) {
        val run = onDecide ?: return
        if (busy) return
        busy = true; error = null
        scope.launch { error = run(approve); busy = false }
    }
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(14.dp).testTag("teamAsk${ask.id}"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Box(Modifier.size(8.dp).clip(CircleShape).background(TeamMapPalette.waiting).clearAndSetSemantics {})
            Text(headline, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        }
        ask.summary?.takeIf { it.isNotEmpty() }?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
        map.rule(ask.ruleId)?.let { rule ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(painterResource(DesignR.drawable.ph_shield_check), null, tint = c.textSecondary, modifier = Modifier.size(14.dp).padding(top = 2.dp))
                Text(androidx.compose.ui.text.buildAnnotatedString {
                    append("Rule: ")
                    pushStyle(androidx.compose.ui.text.SpanStyle(fontWeight = FontWeight.Bold)); append(rule.text); pop()
                }, color = c.textSecondary, fontSize = 13.sp)
            }
        }
        TeamDetailLine("Approver", map.approverName(ask) ?: "Not named yet")
        requester?.let { TeamDetailLine("Asked by", it) }
        map.levelLine(ask)?.let { TeamDetailLine("Level", it) }
        onWhoApproves?.let { TeamTextButton("Who approves this", null, "teamAskWho${ask.id}", onClick = it) }
        if (onDecide != null && map.canDecide(ask)) {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                TeamFilledButton("Approve", "teamAskApprove${ask.id}", enabled = !busy, modifier = Modifier.weight(1f)) { decide(true) }
                TeamFilledButton("Deny", "teamAskDeny${ask.id}", enabled = !busy, modifier = Modifier.weight(1f), prominent = false) { decide(false) }
            }
        } else if (ask.resolve == null) Text("Decide this on the platform.", color = c.textTertiary, fontSize = 12.sp)
        error?.let { TeamErrorText(it) }
    }
}

@Composable
private fun TeamDetailLine(key: String, value: String) {
    val c = AllternitTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(key, color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.widthIn(min = 64.dp))
        Text(value, color = c.textPrimary, fontSize = 12.sp)
    }
}

// MARK: - Node detail

@Composable
private fun TeamCard(header: String?, footer: String? = null, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        header?.let { TeamSectionTitle(it) }
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }
        footer?.let { Text(it, color = c.textTertiary, fontSize = 12.sp) }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TeamNodeDetail(model: TeamMapViewModel, nodeId: String, onOpen: (String) -> Unit, onNewRule: (String) -> Unit, onEditRule: (id: String, scope: String) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var refreshing by remember { mutableStateOf(false) }
    var reportsError by remember { mutableStateOf<String?>(null) }
    var savingReports by remember { mutableStateOf(false) }
    var ruleError by remember { mutableStateOf<String?>(null) }
    var whoApp by remember { mutableStateOf<TeamAppRef?>(null) }
    val map = model.map
    val node = map?.node(nodeId)
    if (map == null || node == null) {
        Box(Modifier.fillMaxSize().padding(20.dp), contentAlignment = Alignment.Center) {
            FriendlyState(FriendlyStyle.Empty, "Not on the team map", "This person or bot isn't on the map anymore.") {
                Icon(painterResource(DesignR.drawable.ph_users_three), null, tint = c.textSecondary)
            }
        }
        return
    }
    val you = node.id == map.me
    PullToRefreshBox(isRefreshing = refreshing, onRefresh = { scope.launch { refreshing = true; model.load(); refreshing = false } }, modifier = Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp).testTag("teamNodeDetail"), verticalArrangement = Arrangement.spacedBy(18.dp)) {
            TeamCard(null) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TeamNodeAvatar(node, TeamMapPalette.ring(node.state, map.waitingOn(node.id) != null))
                    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(if (you) "${node.name} (you)" else node.name, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text(node.subtitle, color = c.textSecondary, fontSize = 13.sp)
                        Text(map.stateText(node), color = c.textTertiary, fontSize = 12.sp)
                    }
                }
                if (node.isBot) map.owner(node.id)?.let { owner -> map.node(owner)?.let { ownerNode ->
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(role = Role.Button) { onOpen(owner) }
                            .semantics { contentDescription = "Owner, ${ownerNode.name}. Opens ${ownerNode.name} on the team map" }.testTag("teamNodeOwner"),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text("Owner", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Text(ownerNode.name, color = c.textSecondary, fontSize = 14.sp)
                        Icon(painterResource(DesignR.drawable.ph_caret_right), null, tint = c.textTertiary, modifier = Modifier.padding(start = 6.dp).size(12.dp))
                    }
                } }
                val waiting = map.waitingOnMe(node.id)
                if (waiting.isNotEmpty()) Text(
                    "${waiting.joinToString(", ") { it.name }} ${if (waiting.size == 1) "is" else "are"} waiting on ${if (you) "you" else node.name}.",
                    color = TeamMapPalette.waiting, fontSize = 13.sp,
                )
            }
            if (node.isBot) {
                val grants = map.grants(node.id)
                TeamCard("Apps and levels", "Browser opens the app and reads. App calls its tools; changes follow the rules. Full runs long tasks, and each call still asks to confirm.") {
                    if (grants.isEmpty()) Text("No apps yet. Give this bot an app from its settings.", color = c.textSecondary, fontSize = 13.sp)
                    grants.forEachIndexed { i, g ->
                        if (i > 0) HorizontalDivider(color = c.borderSubtle)
                        TeamLevelPicker(model, g, map.canEditLevels(node.id), onWhoCanUse = { whoApp = g.app })
                    }
                }
            } else {
                val current = map.parent(node.id)
                val canEdit = map.hasOrg && map.viewerIsAdmin
                TeamCard(
                    "Reporting line",
                    when {
                        !map.hasOrg -> "Make a team to set who reports to whom."
                        !canEdit -> "Only the owner or an admin can change this."
                        else -> "\"The manager\" in a rule means this person's manager."
                    },
                ) {
                    if (canEdit) {
                        Text("Reports to", color = c.textSecondary, fontSize = 13.sp)
                        val choices = listOf<TeamNode?>(null) + map.people.filter { it.id != node.id }
                        choices.forEach { person ->
                            val on = (person?.id) == current
                            Row(
                                Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp))
                                    .clickable(enabled = !savingReports && !on, role = Role.RadioButton) {
                                        val uid = node.userId ?: return@clickable
                                        savingReports = true; reportsError = null
                                        scope.launch { reportsError = model.setReportsTo(uid, person?.userId); savingReports = false }
                                    }
                                    .semantics { selected = on }.testTag("teamReportsTo${person?.id ?: "none"}"),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text(person?.name ?: "No one (top of the team)", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                                if (on) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.accentPrimary, modifier = Modifier.size(16.dp))
                            }
                        }
                    } else Row {
                        Text("Reports to", color = c.textPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Text(map.name(current) ?: "No one", color = c.textSecondary, fontSize = 14.sp)
                    }
                    reportsError?.let { TeamErrorText(it) }
                }
            }
            val asks = map.asks(node)
            if (asks.isNotEmpty()) Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                TeamSectionTitle("Asks (${asks.size} waiting)")
                asks.forEach { ask ->
                    val who = ask.approverUserId?.let { "p:$it" }?.takeIf { it != nodeId && map.node(it) != null }
                    TeamAskRow(ask, map, onDecide = { model.decide(ask, it) }, onWhoApproves = who?.let { { onOpen(it) } })
                }
            }
            val rules = map.rules(node)
            TeamCard(
                "${node.name} · ${teamCount(map.rulesInScope(node.id).size, "rule")} today",
                if (map.viewerIsAdmin) "Rules win over the level." else "Only the owner or an admin can change rules.",
            ) {
                if (rules.isEmpty()) Text("No rules about ${if (you) "you" else node.name} yet.", color = c.textSecondary, fontSize = 13.sp)
                rules.forEach { rule ->
                    TeamRuleRow(
                        rule,
                        onDelete = if (map.viewerIsAdmin) ({ ruleError = null; scope.launch { ruleError = model.deleteRule(rule.id) } }) else null,
                        onEdit = if (map.viewerIsAdmin) ({ onEditRule(rule.id, rule.scopeKey) }) else null,
                        onToggle = if (map.viewerIsAdmin) ({ on -> ruleError = null; scope.launch { ruleError = model.setRuleEnabled(rule.id, on) } }) else null,
                    )
                }
                ruleError?.let { TeamErrorText(it) }
                if (map.viewerIsAdmin) TeamTextButton("New rule", DesignR.drawable.ph_plus, "teamNodeNewRule") { onNewRule(node.id) }
            }
        }
    }
    whoApp?.let { app -> TeamWhoCanUseSheet(app, model, onDismiss = { whoApp = null }) }
}

@Composable
private fun TeamLevelPicker(model: TeamMapViewModel, grant: TeamGrant, canEdit: Boolean, onWhoCanUse: (() -> Unit)? = null) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var saving by remember(grant.id) { mutableStateOf<String?>(null) }
    var error by remember(grant.id) { mutableStateOf<String?>(null) }
    var confirmRemove by remember(grant.id) { mutableStateOf(false) }
    val levels = listOf(
        "browser" to "Open the app and read. No changes.",
        "app" to "Call its tools. Changes follow the rules.",
        "full" to "Long tasks. Each call still asks to confirm.",
    )
    fun allowed(level: String) = grant.ceiling?.let { TeamLevel.rank(level) <= TeamLevel.rank(it) } ?: false
    val app = grant.app.displayName
    val ceilingReason = when (val ceiling = grant.ceiling) {
        null -> "$app is not installed for this bot's owner, so its level cannot change."
        "full" -> null
        else -> {
            val top = TeamLevel.label(ceiling)
            if (grant.app.kind == "vendor") "Ceiling: $top. $app is a vendor app, and vendor apps stop at $top."
            else "Ceiling: $top. The owner's plan allows $app up to $top."
        }
    }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.testTag("teamGrant${grant.id}")) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(app, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            Text(TeamLevel.label(grant.level), color = c.textSecondary, fontSize = 13.sp)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            levels.forEach { (level, desc) ->
                val on = grant.level == level
                val ok = allowed(level)
                val enabled = canEdit && ok && !on && saving == null
                val pick = {
                    saving = level; error = null
                    scope.launch { error = model.setLevel(grant, level); saving = null }
                    Unit
                }
                Row(
                    Modifier.weight(1f).heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp))
                        .background(if (on) c.accentPrimary else c.fillNeutral)
                        .clickable(enabled = enabled, role = Role.RadioButton, onClick = pick)
                        .clearAndSetSemantics { contentDescription = "${TeamLevel.label(level)}. $desc${if (ok) "" else " Locked."}"; selected = on; if (enabled) onClick { pick(); true } }
                        .testTag("teamLevel${grant.id}$level"),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.Center,
                ) {
                    if (saving == level) CircularProgressIndicator(Modifier.size(12.dp).padding(end = 2.dp), strokeWidth = 1.5.dp, color = if (on) Color.White else c.textSecondary)
                    Text(TeamLevel.label(level), fontSize = 13.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                        color = if (on) Color.White else if (ok) c.textPrimary else c.textTertiary)
                    if (!ok) Icon(painterResource(DesignR.drawable.ph_lock_fill), null, tint = c.textTertiary, modifier = Modifier.padding(start = 4.dp).size(11.dp))
                }
            }
        }
        ceilingReason?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
        if (!canEdit) Text("Only the bot's owner or an admin can change its level.", color = c.textTertiary, fontSize = 12.sp)
        error?.let { TeamErrorText(it) }
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            onWhoCanUse?.let { TeamTextButton("Who can use this", null, "teamGrantWho${grant.id}", onClick = it) }
            if (canEdit) TeamTextButton("Remove access", null, "teamGrantRemove${grant.id}", enabled = saving == null) { confirmRemove = true }
        }
    }
    if (confirmRemove) AlertDialog(
        onDismissRequest = { confirmRemove = false },
        title = { Text("Remove $app from this bot?", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold) },
        text = { Text("The bot can't use $app until someone gives it back.", color = c.textSecondary, fontSize = 14.sp) },
        containerColor = c.bgPrimary,
        confirmButton = {
            TextButton(
                onClick = {
                    confirmRemove = false
                    saving = "remove"; error = null
                    scope.launch { error = model.revokeGrant(grant); saving = null }
                },
                modifier = Modifier.heightIn(min = 44.dp).testTag("teamGrantRemoveConfirm${grant.id}"),
            ) { Text("Remove access", color = AllternitStatic.StatusError, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = {
            TextButton(onClick = { confirmRemove = false }, modifier = Modifier.heightIn(min = 44.dp).testTag("teamGrantRemoveCancel${grant.id}")) {
                Text("Cancel", color = c.textPrimary, fontSize = 15.sp)
            }
        },
    )
}

// MARK: - Rule editor

private data class RuleDraft(
    val scope: String, val app: String = "*", val tool: String = "", val field: String = "", val op: String = "gt",
    val value: String = "", val unit: String = "", val approver: String = "owner",
)

private val RULE_OPS = listOf("always" to "always", "gt" to "over", "gte" to "at least", "eq" to "equals", "in" to "is one of")

private fun ruleNumber(text: String): Double? = text.trim().takeIf { it.isNotEmpty() }?.toDoubleOrNull()?.takeIf { it.isFinite() }
private fun firstName(name: String): String = name.split(" ").firstOrNull { it.isNotEmpty() } ?: name

/** Wrapped so "always" (no value) differs from a value that does not parse (null). */
private class ParsedValue(val value: JsonElement?)

private fun RuleDraft.parsedValue(): ParsedValue? {
    val t = value.trim()
    return when (op) {
        "always" -> ParsedValue(null)
        "gt", "gte" -> ruleNumber(t)?.let { ParsedValue(JsonPrimitive(it)) }
        "eq" -> if (t.isEmpty()) null else ParsedValue(ruleNumber(t)?.let { JsonPrimitive(it) } ?: JsonPrimitive(t))
        else -> {
            val items = t.split(",").map { it.trim() }.filter { it.isNotEmpty() }
            if (items.isEmpty()) null else {
                val numbers = items.mapNotNull(::ruleNumber)
                ParsedValue(JsonArray(if (numbers.size == items.size) numbers.map { JsonPrimitive(it) } else items.map { JsonPrimitive(it) }))
            }
        }
    }
}

private fun RuleDraft.problem(): String? {
    if (op != "always" && field.isBlank()) return "Name the field to check, for example amount."
    if (parsedValue() == null) return if (op == "gt" || op == "gte") "Enter a number." else "Enter a value."
    return null
}

private fun valueText(v: JsonElement?): String = when (v) {
    is JsonPrimitive -> v.content.let { c -> c.toDoubleOrNull()?.takeIf { !v.isString && it == Math.rint(it) && Math.abs(it) < 1e15 }?.toLong()?.toString() ?: c }
    is JsonArray -> v.joinToString(", ") { valueText(it) }
    else -> ""
}

/** iOS TeamRuleEditorView.draft(from:). */
private fun TeamFullRule.toDraft(): RuleDraft = RuleDraft(
    scope = when (scopeKind) { "person" -> scopeRef?.let { "p:$it" } ?: "org"; "bot" -> scopeRef ?: "org"; else -> "org" },
    app = app?.key ?: "*", tool = if (tool == "*") "" else tool, field = field.orEmpty(), op = op,
    value = valueText(value), unit = unitLabel.orEmpty(),
    approver = if (approverKind == "user") "u:${approverUserId.orEmpty()}" else approverKind,
)

private fun RuleDraft.approverShort(map: TeamMap): String = when (approver) {
    "owner" -> "the owner"
    "manager" -> "the manager"
    else -> firstName(map.name("p:" + approver.drop(2)) ?: "someone")
}

private fun RuleDraft.tryLine(map: TeamMap): String? {
    if (problem() != null) return null
    val who = approverShort(map)
    if (op == "always") return "Try it: every request asks $who first."
    if (op != "gt" && op != "gte") return null
    val n = ruleNumber(value)?.takeIf { it > 0 } ?: return null
    val u = unit.trim().ifEmpty { map.currencyMark }
    fun money(x: Double): String {
        val amount = java.text.NumberFormat.getIntegerInstance().format(x.toLong())
        return if (u.isEmpty()) amount else if (u.length <= 2) "$u$amount" else "$amount $u"
    }
    return "Try it: ${money(Math.round(n * 1.3).toDouble())} asks $who · ${money(maxOf(0.0, Math.floor(n / 3)))} runs without asking."
}

private fun RuleDraft.input(): TeamRuleInput? {
    val v = parsedValue() ?: return null
    val scopeKind = if (scope == "org") "org" else if (scope.startsWith("p:")) "person" else "bot"
    val parts = app.split(":", limit = 2)
    val anyApp = app == "*" || parts.size < 2
    val approverKind = if (approver.startsWith("u:")) "user" else approver
    val needsField = op != "always"
    val u = unit.trim()
    return TeamRuleInput(
        scopeKind = scopeKind,
        scopeRef = when (scopeKind) { "org" -> null; "person" -> scope.drop(2); else -> scope },
        appKind = if (anyApp) null else parts[0], appRef = if (anyApp) null else parts[1],
        tool = tool.trim().ifEmpty { "*" },
        field = if (needsField) field.trim() else null,
        op = op, value = v.value,
        unitLabel = if (needsField && u.isNotEmpty()) u.take(24) else null,
        approverKind = approverKind, approverUserId = if (approverKind == "user") approver.drop(2) else null,
    )
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TeamRuleEditor(model: TeamMapViewModel, scopeKey: String, ruleId: String?, onSaved: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    var draft by remember(scopeKey) { mutableStateOf(RuleDraft(scopeKey)) }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var loading by remember(ruleId) { mutableStateOf(ruleId != null) }
    var loadError by remember(ruleId) { mutableStateOf<String?>(null) }
    var loadTick by remember(ruleId) { mutableStateOf(0) }
    /** The edited rule's app, kept as a choice even when no bot has a grant for it. */
    var ruleApp by remember(ruleId) { mutableStateOf<TeamAppRef?>(null) }
    // The map lists a rule's sentence only; the editor needs op, tool and value.
    LaunchedEffect(ruleId, loadTick) {
        if (ruleId == null) return@LaunchedEffect
        loading = true; loadError = null
        when (val r = model.fullRule(ruleId)) {
            is TeamResult.Ok -> { ruleApp = r.value.app; draft = r.value.toDraft(); loading = false }
            is TeamResult.Failed -> { loadError = r.error.text; loading = false }
        }
    }
    val map = model.map ?: return
    if (loadError != null || loading) {
        Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically), horizontalAlignment = Alignment.CenterHorizontally) {
            if (loadError != null) {
                Text(loadError.orEmpty(), color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
                TeamTextButton("Try again", null, "teamRuleLoadRetry") { loadTick++ }
            } else {
                CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
                Text("Loading the rule…", color = c.textSecondary, fontSize = 14.sp)
            }
        }
        return
    }
    val appChoices = map.apps + listOfNotNull(ruleApp?.takeIf { r -> map.apps.none { it.key == r.key } })
    fun edit(next: RuleDraft) { draft = next; error = null }
    val scopeName = if (draft.scope == "org") "Anyone" else map.name(draft.scope) ?: "Someone"
    val appName = if (draft.app == "*") "any app" else appChoices.firstOrNull { it.key == draft.app }?.displayName ?: draft.app
    val opLabel = RULE_OPS.firstOrNull { it.first == draft.op }?.second ?: "over"
    val current = map.rulesInScope(draft.scope)
    val problem = draft.problem()
    val opOptions = RULE_OPS.map { (op, label) -> RulePick(label) { edit(draft.copy(op = op)) } }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp).testTag("teamRuleEditor"), verticalArrangement = Arrangement.spacedBy(18.dp)) {
        TeamCard("$scopeName · ${teamCount(current.size, "rule")} today") {
            if (current.isEmpty()) Text("No rules yet.", color = c.textSecondary, fontSize = 13.sp)
            current.forEach { rule ->
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.weight(1f)) { TeamRuleRow(rule) }
                    if (rule.id == ruleId) Text("Editing", color = c.textSecondary, fontSize = 12.sp)
                }
            }
        }
        TeamCard(if (ruleId == null) "New rule" else "This rule", "Rules win over the level.") {
            FlowRow(
                Modifier.semantics { contentDescription = "Rule sentence" },
                horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                RuleWord("When")
                RulePickBlank("Who", scopeName, "teamRuleWho",
                    listOf(RulePick("Anyone") { edit(draft.copy(scope = "org")) }) +
                        map.people.map { p -> RulePick(p.name, "People") { edit(draft.copy(scope = p.id)) } } +
                        map.bots.map { b -> RulePick(b.name, "Bots") { edit(draft.copy(scope = b.id)) } })
                RuleWord("uses")
                RulePickBlank("App", appName, "teamRuleApp",
                    listOf(RulePick("Any app") { edit(draft.copy(app = "*")) }) + appChoices.map { a -> RulePick(a.displayName) { edit(draft.copy(app = a.key)) } })
                RuleWord("·")
                RuleField("Tool", draft.tool, "any tool", 80, "teamRuleTool") { edit(draft.copy(tool = it)) }
                if (draft.op == "always") {
                    RuleWord(",")
                    RulePickBlank("Condition", "always", "teamRuleOp", opOptions)
                } else {
                    RuleWord("and")
                    RuleField("Field", draft.field, "pick a field", 96, "teamRuleField") { edit(draft.copy(field = it)) }
                    RuleWord("is")
                    RulePickBlank("Condition", opLabel, "teamRuleOp", opOptions)
                    RuleField("Unit (optional)", draft.unit, map.currencyMark.ifEmpty { "unit" }, 40, "teamRuleUnit") { edit(draft.copy(unit = it)) }
                    RuleField("Value", draft.value, if (draft.op == "in") "a, b, c" else "250", 60, "teamRuleValue",
                        if (draft.op == "gt" || draft.op == "gte") KeyboardType.Decimal else KeyboardType.Text) { edit(draft.copy(value = it)) }
                    RuleWord(",")
                }
                RuleWord("ask")
                RulePickBlank("Approver", draft.approverShort(map), "teamRuleApprover",
                    listOf(RulePick("The owner") { edit(draft.copy(approver = "owner")) }, RulePick("The manager") { edit(draft.copy(approver = "manager")) }) +
                        map.people.map { p -> RulePick(firstName(p.name), "A person") { edit(draft.copy(approver = "u:${p.userId.orEmpty()}")) } })
                RuleWord("first.")
            }
            draft.tryLine(map)?.let { line ->
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag("teamRuleTry")) {
                    Icon(painterResource(DesignR.drawable.ph_check), null, tint = AllternitStatic.StatusSuccess, modifier = Modifier.size(14.dp).padding(top = 2.dp))
                    Text(line, color = c.textPrimary, fontSize = 13.sp)
                }
            }
            problem?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
            error?.let { TeamErrorText(it) }
        }
        TeamFilledButton(if (saving) "Saving…" else if (ruleId == null) "Save rule" else "Save changes", "teamRuleSave", enabled = problem == null && !saving, modifier = Modifier.fillMaxWidth()) {
            val input = draft.input() ?: return@TeamFilledButton
            saving = true; error = null
            scope.launch {
                val failure = if (ruleId != null) model.updateRule(ruleId, input) else model.createRule(input)
                saving = false
                if (failure == null) { Haptics.perform(view, Haptics.Kind.Success); onSaved() } else error = failure
            }
        }
    }
}

private class RulePick(val label: String, val section: String? = null, val onPick: () -> Unit)

@Composable
private fun RuleWord(text: String) =
    Text(text, color = AllternitTheme.colors.textPrimary, fontSize = 15.sp, modifier = Modifier.heightIn(min = 36.dp).padding(top = 8.dp).clearAndSetSemantics {})

@Composable
private fun RulePickBlank(label: String, text: String, tag: String, options: List<RulePick>) {
    val c = AllternitTheme.colors
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier.heightIn(min = 36.dp).clip(RoundedCornerShape(8.dp)).background(c.fillNeutral)
                .clickable(role = Role.Button) { open = true }
                .semantics { contentDescription = "$label: $text" }.testTag(tag).padding(horizontal = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            Text(text, color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 180.dp))
            Icon(painterResource(DesignR.drawable.ph_caret_down), null, tint = c.accentPrimary, modifier = Modifier.size(10.dp))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = c.bgPrimary) {
            var lastSection: String? = null
            options.forEach { o ->
                if (o.section != null && o.section != lastSection) {
                    Text(o.section, color = c.textTertiary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp).semantics { heading() })
                }
                lastSection = o.section
                DropdownMenuItem(text = { Text(o.label, color = c.textPrimary, fontSize = 15.sp) }, onClick = { open = false; o.onPick() })
            }
        }
    }
}

@Composable
private fun RuleField(label: String, value: String, placeholder: String, minWidth: Int, tag: String, keyboard: KeyboardType = KeyboardType.Text, onChange: (String) -> Unit) {
    val c = AllternitTheme.colors
    val empty = value.isBlank()
    val border = c.borderStrong
    BasicTextField(
        value = value, onValueChange = onChange, singleLine = true,
        textStyle = TextStyle(color = c.textPrimary, fontSize = 15.sp),
        cursorBrush = SolidColor(c.accentPrimary),
        keyboardOptions = KeyboardOptions(keyboardType = keyboard, autoCorrectEnabled = false),
        modifier = Modifier.widthIn(min = minWidth.dp).semantics { contentDescription = label }.testTag(tag),
        decorationBox = { inner ->
            Box(
                Modifier.heightIn(min = 36.dp).drawBehind {
                    drawRoundRect(border, cornerRadius = androidx.compose.ui.geometry.CornerRadius(8.dp.toPx()),
                        style = Stroke(width = 1.dp.toPx(), pathEffect = if (empty) PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 3.dp.toPx())) else null))
                }.padding(horizontal = 10.dp, vertical = 8.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (empty) Text(placeholder, color = c.textTertiary, fontSize = 15.sp, maxLines = 1)
                inner()
            }
        },
    )
}

// MARK: - Who can use this

/**
 * "Who can use this": for one connection or app, the people and bots that can use it, each with a level chip
 * (Browser / App / Full). iOS TeamWhoCanUseSheet (#3038). Opened from an app row on a bot's page and from a
 * connected connector. [ready] holds the load until the map is in (the host resolves the app against it first).
 */
@Composable
internal fun TeamWhoCanUseSheet(app: TeamAppRef, model: TeamMapViewModel, ready: Boolean = true, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var access by remember(app.key) { mutableStateOf<TeamAppAccess?>(null) }
    var failure by remember(app.key) { mutableStateOf<TeamMapError?>(null) }
    var tick by remember(app.key) { mutableStateOf(0) }
    LaunchedEffect(app.key, ready, tick) {
        if (!ready) return@LaunchedEffect
        if (model.map == null) model.load()
        failure = null
        when (val r = model.appAccess(app.kind, app.ref)) {
            is TeamResult.Ok -> access = r.value
            is TeamResult.Failed -> failure = r.error
        }
    }
    fun name(id: String): String = model.map?.name(id) ?: id.drop(2)
    fun grant(bot: String): TeamGrant? = model.map?.grants?.firstOrNull { it.bot == bot && it.app.kind == app.kind && it.app.ref == app.ref }
    fun botDetail(bot: TeamAppAccess.Bot): String {
        val owner = bot.owner ?: model.map?.owner(bot.bot)
        var text = owner?.let { "${name(it)}'s" } ?: "No owner"
        grant(bot.bot)?.ceiling?.takeIf { it != "full" }?.let { text += " · stops at ${TeamLevel.label(it)}" }
        return text
    }
    val ceilings = (model.map?.grants ?: emptyList()).filter { it.app.kind == app.kind && it.app.ref == app.ref }.map { it.ceiling }
    val appCeiling = ceilings.firstOrNull()?.takeIf { first -> ceilings.all { it == first } }
    @Composable fun chip(level: String) = Text(
        TeamLevel.label(level), color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
        modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 10.dp, vertical = 4.dp),
    )
    LibrarySheet(
        title = "Who can use ${app.displayName}", testTag = "teamWhoSheet", onDismiss = onDismiss,
        trailing = LibrarySheetAction("Done", bold = true, testTag = "teamWhoDone", onClick = onDismiss),
    ) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.7f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp).testTag("teamWhoContent"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            val a = access
            val f = failure
            when {
                f != null -> Column(Modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (f === TeamMapError.NoRuntime) Text("Connect a computer to see who can use this.", color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
                    else {
                        Text(f.text, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
                        TeamTextButton("Try again", null, "teamWhoRetry") { tick++ }
                    }
                }
                a == null -> Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp, color = c.textSecondary)
                }
                else -> {
                    TeamSectionTitle("People")
                    if (a.people.isEmpty()) Text("No one yet.", color = c.textSecondary, fontSize = 13.sp)
                    a.people.forEach { p ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 44.dp).clearAndSetSemantics { contentDescription = "${name(p.user)}, ${TeamLevel.label(p.level)}" },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Icon(painterResource(DesignR.drawable.ph_user_circle), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
                            Text(name(p.user), color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                            chip(p.level)
                        }
                    }
                    TeamSectionTitle("Bots")
                    if (a.bots.isEmpty()) Text("No bots yet.", color = c.textSecondary, fontSize = 13.sp)
                    a.bots.forEach { b ->
                        Row(
                            Modifier.fillMaxWidth().heightIn(min = 44.dp).clearAndSetSemantics { contentDescription = "${name(b.bot)}, ${botDetail(b)}, ${TeamLevel.label(b.level)}" },
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            Icon(painterResource(DesignR.drawable.ph_cpu), null, tint = c.textSecondary, modifier = Modifier.size(20.dp))
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(name(b.bot), color = c.textPrimary, fontSize = 15.sp)
                                Text(botDetail(b), color = c.textSecondary, fontSize = 12.sp)
                            }
                            chip(b.level)
                        }
                    }
                    Text(
                        if (app.kind == "vendor" || appCeiling == "app") "Vendor app: levels stop at App." else "Change a bot's level from its page on the team map.",
                        color = c.textTertiary, fontSize = 12.sp,
                    )
                }
            }
        }
    }
}

/**
 * The map's own ref for [kind]/[ref]/[name]: same ref, `mcp_<ref>`, or the same name (case-insensitive) among the
 * map's grants of that kind. Unchanged when the map is not loaded or nothing matches (the sheet then shows nobody).
 * iOS TeamWhoCanUseHost.resolve (a5ee2e0b6e).
 */
internal fun resolveTeamApp(app: TeamAppRef, map: TeamMap?): TeamAppRef {
    if (map == null) return app
    val known = map.grants.map { it.app }.filter { it.kind == app.kind }
    known.firstOrNull { it.ref == app.ref || it.ref == "mcp_${app.ref}" }?.let { return it }
    val name = app.name?.lowercase() ?: return app
    return known.firstOrNull { it.name?.lowercase() == name } ?: app
}

/**
 * "Who can use this" for a screen with no team map of its own (the Connectors list): owns a map model, resolves the
 * connector's catalog id to the map's ref, and shows the sheet.
 */
@Composable
fun TeamWhoCanUseHost(kind: String, ref: String, name: String?, onDismiss: () -> Unit) {
    val model = remember { TeamMapViewModel() }
    LaunchedEffect(model) { model.load() }
    val app = resolveTeamApp(TeamAppRef(kind, ref, name), model.map)
    TeamWhoCanUseSheet(app, model, ready = model.phase !is TeamMapViewModel.Phase.Loading, onDismiss = onDismiss)
}

// MARK: - Team approvals (Inbox, Cowork Fabric approvals)

/**
 * The asks the viewer can decide, above an approvals list: Rule, Approver, Asked by, Level, Approve/Deny. Asks with no
 * `resolve` are read-only. Shows nothing when flag `team.map` is off or there are no asks. iOS FabricApprovalsView (#3038).
 */
@Composable
fun TeamAsksSection(reloadKey: Any? = null, horizontalPadding: androidx.compose.ui.unit.Dp = 20.dp) {
    val context = androidx.compose.ui.platform.LocalContext.current
    if (!com.allternit.core.api.FeatureFlags.isOn(context, com.allternit.core.api.FeatureFlags.TEAM_MAP)) return
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val model = remember { TeamMapViewModel() }
    var sheetOpen by remember { mutableStateOf(false) }
    var focus by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(model, reloadKey) { model.load() }
    val map = model.map
    if (map != null && map.asks.isNotEmpty()) Column(Modifier.fillMaxWidth().padding(horizontal = horizontalPadding, vertical = 12.dp).testTag("teamAsksSection"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        TeamSectionTitle("Team approvals")
        map.asks.forEach { ask ->
            TeamAskRow(ask, map, onDecide = { model.decide(ask, it) }, onWhoApproves = { focus = ask.approverUserId?.let { "p:$it" }; sheetOpen = true })
        }
        map.partialNote?.let { Text(it, color = c.textSecondary, fontSize = 12.sp) }
    }
    if (sheetOpen) TeamMapSheet(focus = focus, onDismiss = { sheetOpen = false; scope.launch { model.load() } })
}
