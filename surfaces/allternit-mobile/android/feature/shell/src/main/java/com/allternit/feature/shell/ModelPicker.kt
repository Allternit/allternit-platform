package com.allternit.feature.shell

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.launch

private sealed interface PickerScope {
    data object All : PickerScope
    data object Favorites : PickerScope
    data class Provider(val id: String) : PickerScope
}

private data class ModelGroup(val key: String, val logo: String, val title: String, val rows: List<PickerModel>)

/**
 * "Pick a brain" (iOS ModelPickerSheet): search, the Reasoning / Local filters, the overview
 * cards (All, Favorites, one per provider, Connect), the current model with its thinking levels,
 * then every model grouped by provider. Allternit Cloud rows split by upstream vendor
 * ("Anthropic · via Allternit Cloud"). A pick persists and the sheet stays open so the level can
 * be set, as on iOS. iOS's "On this iPhone" group has no Android source in this app, so it is
 * left out.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ModelPickerSheet(models: ModelStore, onOpenSection: (RailRow) -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var query by remember { mutableStateOf("") }
    var pick by remember { mutableStateOf<PickerScope>(PickerScope.All) }
    var reasoning by remember { mutableStateOf(false) }
    var local by remember { mutableStateOf(false) }
    var creditsRequest by remember { mutableStateOf<com.allternit.feature.billing.CloudCreditsRequest?>(null) }
    LaunchedEffect(Unit) { models.load(); com.allternit.feature.billing.AccountPlan.load() }
    creditsRequest?.let { req -> com.allternit.feature.billing.CloudCreditsSheet(req) { creditsRequest = null } }

    val offered = models.chatModels.filter { !it.hidden || it.id == models.selectedId }
    val providerIds = offered.map { it.provider }.distinct()
        .sortedWith(compareBy<String> { ModelProviderNames.rank(it) }.thenBy { ModelProviderNames.name(it) })
    val q = query.trim().lowercase()
    val matches = offered.filter { m ->
        when (val p = pick) {
            PickerScope.Favorites -> if (m.id !in models.favorites) return@filter false
            is PickerScope.Provider -> if (m.provider != p.id) return@filter false
            PickerScope.All -> Unit
        }
        if (reasoning && m.thinking == null) return@filter false
        if (local && m.provider !in ModelProviderNames.local) return@filter false
        q.isEmpty() || "${m.name} ${m.id} ${ModelProviderNames.name(m.provider)} ${m.thinking?.term.orEmpty()}".lowercase().contains(q)
    }
    val groups = buildList {
        for (pid in providerIds) {
            val rows = rankOrdered(matches.filter { it.provider == pid })
            if (rows.isEmpty()) continue
            if (pid == "allternit") {
                val vendors = rows.map { ModelProviderNames.inferVendor(it) }.distinct()
                    .sortedBy { if (it == "other") "Allternit" else ModelProviderNames.name(it) }
                for (v in vendors) {
                    val name = if (v == "other") "Allternit" else ModelProviderNames.name(v)
                    add(ModelGroup("allternit:$v", if (v == "other") "allternit" else v, "$name · via Allternit Cloud", rows.filter { ModelProviderNames.inferVendor(it) == v }))
                }
            } else {
                add(ModelGroup(pid, pid, ModelProviderNames.name(pid), rows))
            }
        }
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = state,
        containerColor = c.bgPrimary,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
        dragHandle = null,
    ) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(0.86f).testTag("modelPicker")) {
            Box(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 6.dp), contentAlignment = Alignment.Center) {
                Box(Modifier.width(36.dp).height(5.dp).clip(CircleShape).background(c.borderStrong))
            }
            // Header: search, filters, overview cards.
            Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(
                    modifier = Modifier.fillMaxWidth().clip(CircleShape).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
                    Box(Modifier.weight(1f)) {
                        if (query.isEmpty()) Text("Search every model", color = c.textTertiary, fontSize = 16.sp)
                        BasicTextField(
                            value = query,
                            onValueChange = { query = it },
                            singleLine = true,
                            textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp, fontFamily = ShellSans),
                            cursorBrush = SolidColor(c.textPrimary),
                            modifier = Modifier.fillMaxWidth().testTag("modelPickerSearch").semantics { contentDescription = "Search models" },
                        )
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip("Reasoning", reasoning) { reasoning = !reasoning }
                    FilterChip("Local", local) { local = !local }
                }
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                    OverviewCard("All", "${offered.size} models", pick == PickerScope.All, "modelPickerAll", {
                        Icon(painterResource(R.drawable.shell_grid), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(18.dp))
                    }) { pick = PickerScope.All }
                    OverviewCard("Favorites", "${models.favorites.size}", pick == PickerScope.Favorites, "modelPickerFavorites", {
                        Icon(painterResource(R.drawable.shell_star_fill), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(18.dp))
                    }) { pick = PickerScope.Favorites }
                    providerIds.forEach { pid ->
                        val count = offered.count { it.provider == pid }
                        OverviewCard(ModelProviderNames.name(pid), "$count models", pick == PickerScope.Provider(pid), "modelPickerProvider$pid", {
                            Box(Modifier.size(28.dp).clip(CircleShape).border(2.5.dp, c.borderSubtle, CircleShape), contentAlignment = Alignment.Center) {
                                ProviderMark(pid, 15.dp)
                            }
                            Box(
                                Modifier
                                    .align(Alignment.BottomEnd)
                                    .size(7.dp)
                                    .clip(CircleShape)
                                    .background(AllternitStatic.StatusSuccess)
                                    .border(1.5.dp, c.bgPrimary, CircleShape),
                            )
                        }) { pick = PickerScope.Provider(pid) }
                    }
                    OverviewCard("Connect", "account", false, "modelPickerConnect", {
                        Icon(painterResource(R.drawable.shell_plus), contentDescription = null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
                    }) { onOpenSection(RailRow("models", "Models", R.drawable.shell_brain)) }
                }
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))

            LazyColumn(Modifier.fillMaxWidth().weight(1f).padding(horizontal = 10.dp)) {
                val current = models.selected
                if (q.isEmpty() && pick == PickerScope.All && current != null) {
                    item(key = "current") { CurrentModel(current, models) }
                }
                when {
                    models.isLoading && models.chatModels.isEmpty() -> item(key = "loading") {
                        Box(Modifier.fillMaxWidth().padding(top = 32.dp), contentAlignment = Alignment.Center) {
                            CircularProgressIndicator(Modifier.size(22.dp).semantics { contentDescription = "Loading models" }, strokeWidth = 2.dp, color = c.textSecondary)
                        }
                    }
                    models.chatModels.isEmpty() && models.loadError != null -> item(key = "error") {
                        FriendlyState(
                            style = FriendlyStyle.Offline,
                            title = "Couldn't load models",
                            message = models.loadError.orEmpty(),
                            actionTitle = "Retry",
                            onAction = { scope.launch { models.load(force = true) } },
                            modifier = Modifier.padding(top = 16.dp).testTag("modelPickerError"),
                        ) {
                            Icon(painterResource(R.drawable.shell_wifi_slash), contentDescription = null, tint = c.textSecondary)
                        }
                    }
                    groups.isEmpty() -> item(key = "empty") {
                        Text(
                            if (pick == PickerScope.Favorites) "Star a model to keep it here." else "No models match",
                            color = c.textSecondary,
                            fontSize = 14.sp,
                            modifier = Modifier.fillMaxWidth().padding(vertical = 24.dp),
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                    }
                    else -> groups.forEach { group ->
                        item(key = "h-${group.key}") {
                            Row(
                                modifier = Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 14.dp, bottom = 4.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(6.dp),
                            ) {
                                ProviderMark(group.logo, 15.dp)
                                Text(group.title.uppercase(), color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.8.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                Text("${group.rows.size}", color = c.textTertiary, fontSize = 11.sp)
                            }
                        }
                        item(key = "g-${group.key}") {
                            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral)) {
                                group.rows.forEachIndexed { i, m ->
                                    ModelRow(m, models) { creditsRequest = it }
                                    if (i < group.rows.lastIndex) Box(Modifier.fillMaxWidth().padding(start = 44.dp).height(1.dp).background(c.borderSubtle))
                                }
                            }
                        }
                    }
                }
                item(key = "foot") { Box(Modifier.navigationBarsPadding().height(24.dp)) }
            }
        }
    }
}

/** Ranked models first by ascending rank; unranked keep their order after (iOS rankOrdered). */
private fun rankOrdered(rows: List<PickerModel>): List<PickerModel> =
    rows.withIndex().sortedWith(compareBy<IndexedValue<PickerModel>> { it.value.rank == null }.thenBy { it.value.rank ?: 0L }.thenBy { it.index }).map { it.value }

@Composable
private fun FilterChip(label: String, on: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        label,
        color = if (on) c.textPrimary else c.textSecondary,
        fontSize = 13.sp,
        modifier = Modifier
            .clip(CircleShape)
            .background(if (on) c.fillNeutral else Color.Transparent)
            .border(1.dp, if (on) c.accentPrimary.copy(alpha = 0.5f) else c.borderSubtle, CircleShape)
            .clickable(role = Role.Checkbox, onClick = onClick)
            .semantics { selected = on }
            .padding(horizontal = 12.dp, vertical = 5.dp),
    )
}

@Composable
private fun OverviewCard(title: String, detail: String, active: Boolean, testTag: String, icon: @Composable androidx.compose.foundation.layout.BoxScope.() -> Unit, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        modifier = Modifier
            .clip(RoundedCornerShape(14.dp))
            .background(c.fillNeutral)
            .border(BorderStroke(1.dp, if (active) c.accentPrimary else c.borderSubtle), RoundedCornerShape(14.dp))
            .testTag(testTag)
            .clickable(role = Role.Tab, onClick = onClick)
            .semantics(mergeDescendants = true) { selected = active }
            .padding(start = 8.dp, end = 12.dp, top = 7.dp, bottom = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(Modifier.size(28.dp), contentAlignment = Alignment.Center, content = icon)
        Column {
            Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(detail, color = c.textTertiary, fontSize = 11.5.sp, maxLines = 1)
        }
    }
}

/** The matrix mark for Allternit Cloud, else the provider's initial on a neutral tile (iOS ModelProviderMark). */
@Composable
private fun ProviderMark(pid: String, size: Dp) {
    val c = AllternitTheme.colors
    Box(
        Modifier.size(size).clip(RoundedCornerShape(size * 0.28f)).background(c.fillNeutral),
        contentAlignment = Alignment.Center,
    ) {
        if (pid == "allternit") {
            MatrixLogo(size = size * 0.84f)
        } else {
            Text(ModelProviderNames.mark(pid), color = c.textPrimary, fontSize = (size.value * 0.55f).sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun RowBadge(text: String) {
    val c = AllternitTheme.colors
    Text(text, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.clip(RoundedCornerShape(4.dp)).background(c.fillNeutral).padding(horizontal = 5.dp, vertical = 1.dp))
}

@Composable
private fun ModelRow(m: PickerModel, models: ModelStore, onLocked: (com.allternit.feature.billing.CloudCreditsRequest) -> Unit = {}) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val selectedRow = models.selectedId == m.id
    val favorite = m.id in models.favorites
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("modelRow${m.id}")
            .clickable(role = Role.Button) {
                Haptics.perform(view, Haptics.Kind.Tap)
                // iOS ModelPickerSheet.choose: a locked Cloud row opens Cloud credits and keeps the old selection.
                val locked = if (m.cloudProvider != null || m.id.startsWith(ModelStore.CLOUD_PREFIX)) {
                    com.allternit.feature.billing.CloudCreditsGate.lockedRequest(m.name, m.cloudProvider, m.inputPer1mUsd, m.outputPer1mUsd)
                } else null
                if (locked != null) onLocked(locked) else models.select(m.id)
            }
            .semantics(mergeDescendants = true) {
                selected = selectedRow
                contentDescription = listOfNotNull(m.name, "default".takeIf { m.isDefault }).joinToString(", ")
            }
            .padding(horizontal = 10.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        ProviderMark(m.provider, 26.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(m.name, color = c.textPrimary, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                if (m.isDefault) RowBadge("Default")
            }
            Text(rowSubtitle(m), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            m.thinking?.let { t -> RowBadge("${t.termOrDefault} · ${t.levels.size}") }
        }
        if (selectedRow) Icon(painterResource(DesignR.drawable.ph_check), contentDescription = null, tint = c.accentPrimary, modifier = Modifier.size(15.dp))
        Box(
            Modifier
                .size(36.dp)
                .clip(CircleShape)
                .clickable(role = Role.Button, onClickLabel = if (favorite) "Unfavorite ${m.name}" else "Favorite ${m.name}") { models.toggleFavorite(m.id) }
                .semantics { contentDescription = if (favorite) "Unfavorite ${m.name}" else "Favorite ${m.name}" },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                painterResource(if (favorite) R.drawable.shell_star_fill else R.drawable.shell_star),
                contentDescription = null,
                tint = if (favorite) c.accentPrimary else c.textTertiary,
                modifier = Modifier.size(18.dp),
            )
        }
    }
}

private fun rowSubtitle(m: PickerModel): String {
    m.description?.takeIf { it.isNotEmpty() && it != "Allternit Cloud" }?.let { return it }
    return if (m.provider == "allternit") "via Allternit Cloud" else ModelProviderNames.name(m.provider)
}

/** The current model and its thinking levels (iOS currentModel + ThinkingSliderView, as chips). */
@Composable
private fun CurrentModel(m: PickerModel, models: ModelStore) {
    val c = AllternitTheme.colors
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 10.dp, bottom = 6.dp)
            .clip(RoundedCornerShape(18.dp))
            .background(c.fillNeutral)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(18.dp))
            .padding(12.dp)
            .testTag("modelPickerCurrent"),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(52.dp).clip(CircleShape).border(2.5.dp, c.borderSubtle, CircleShape), contentAlignment = Alignment.Center) { ProviderMark(m.provider, 22.dp) }
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(m.name, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text(
                    if (m.provider == "allternit") "Allternit Cloud" else "${ModelProviderNames.name(m.provider)} · limits not reported",
                    color = c.textTertiary,
                    fontSize = 12.sp,
                )
            }
        }
        val thinking = m.thinking
        if (thinking == null) {
            Text(
                "This model has no thinking setting.",
                color = c.textTertiary,
                fontSize = 12.sp,
                modifier = Modifier.fillMaxWidth().border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp)).padding(12.dp),
            )
        } else {
            // The block-grid slider (iOS ThinkingSliderView): its level is the effort the chat sends.
            ThinkingSlider(thinking = thinking, value = models.level(m) ?: thinking.default) { models.chooseEffort(it) }
        }
    }
}
