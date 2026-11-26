package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import kotlinx.coroutines.launch

/**
 * Account-wide tool policies for the connections list (`GET` / `PUT /api/v1/tool-policies`). The Settings >
 * Connections screen holds one; each connected app row reads its summary, and the sheet below writes through it.
 */
internal class ToolPolicyStore {
    /** owner id to (tool id to policy); tools missing from the inner map are Ask. */
    var policies by mutableStateOf<Map<String, Map<String, ToolPolicy>>>(emptyMap())
        private set

    /** True once the server answered 404: this server can't save tool permissions yet. */
    var unsupported by mutableStateOf(false)
        private set

    suspend fun load() {
        runCatching { ToolPoliciesClient.load() }
            .onSuccess { policies = it; unsupported = false }
            .onFailure { if (ToolPoliciesClient.unsupported(it)) unsupported = true }
    }

    fun summary(connectorId: String) = toolPolicySummary(policies[ToolPoliciesClient.owner(connectorId)])

    fun policy(owner: String, tool: String) = policies[owner]?.get(tool) ?: ToolPolicy.Ask

    /** Shows [changes] at once, then saves them. On failure the previous values come back and the error is returned. */
    suspend fun apply(owner: String, changes: Map<String, ToolPolicy>): Throwable? {
        val before = policies
        policies = before + (owner to ((before[owner] ?: emptyMap()) + changes).filterValues { it != ToolPolicy.Ask })
        val failure = runCatching { ToolPoliciesClient.save(owner, changes) }.exceptionOrNull() ?: return null
        policies = before
        if (ToolPoliciesClient.unsupported(failure)) unsupported = true
        return failure
    }
}

private class PendingSave(val changes: Map<String, ToolPolicy>, val label: String)

/**
 * The "Tool permissions" bottom sheet of one connected app: tools in read-only and write/delete groups, a default
 * per group and an Ask / Allow / Block control per tool (iOS ToolPermissionsSheet shows the same).
 */
@Composable
internal fun ToolPermissionsSheet(connectorId: String, name: String, store: ToolPolicyStore, onDismiss: () -> Unit) {
    val c = setupColors
    val owner = ToolPoliciesClient.owner(connectorId)
    val scope = rememberCoroutineScope()
    var tools by remember(connectorId) { mutableStateOf<List<ConnTool>?>(null) }
    var loadFailed by remember(connectorId) { mutableStateOf(false) }
    var failed by remember(connectorId) { mutableStateOf<PendingSave?>(null) }
    var attempt by remember(connectorId) { mutableStateOf(0) }

    LaunchedEffect(connectorId, attempt) {
        loadFailed = false
        runCatching { ToolPoliciesClient.tools(connectorId) }
            .onSuccess { tools = it }
            .onFailure { loadFailed = true }
        store.load()
    }

    fun save(changes: Map<String, ToolPolicy>, label: String) {
        failed = null
        scope.launch { if (store.apply(owner, changes) != null) failed = PendingSave(changes, label) }
    }

    LibrarySheet("Tool permissions", "toolPermissionsSheet", onDismiss, trailing = LibrarySheetAction("Done", bold = true) { onDismiss() }) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            Text("$name · Choose when agents may use these tools.", color = c.sub, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 6.dp))
            val list = tools
            when {
                store.unsupported -> Note("This server can't save tool permissions yet.", "toolPermissionsUnsupported")
                loadFailed -> Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Could not load the tools.", color = c.errFg, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    RetryButton { attempt++ }
                }
                list == null -> Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { CircularProgressIndicator(strokeWidth = 2.dp, color = c.sub) }
                list.isEmpty() -> Note("This connection has no tools to set.", "toolPermissionsEmpty")
                else -> {
                    val enabled = !store.unsupported
                    for ((title, rows) in listOf("Read-only tools" to list.filter { it.readOnly }, "Write/delete tools" to list.filter { !it.readOnly })) {
                        if (rows.isEmpty()) continue
                        Text(title, color = c.sub, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp))
                        val values = rows.map { store.policy(owner, it.id) }.toSet()
                        PolicyRow("Default for group", values.singleOrNull(), enabled, "toolPolicyGroup_$title") { p ->
                            save(rows.associate { it.id to p }, "$title default")
                        }
                        for (t in rows) {
                            PolicyRow(t.id, store.policy(owner, t.id), enabled, "toolPolicy_${t.id}") { p -> save(mapOf(t.id to p), t.id) }
                            failed?.takeIf { it.label == t.id }?.let { f ->
                                SaveError("Could not save. Back to ${store.policy(owner, t.id).label}.", f) { save(it.changes, it.label) }
                            }
                        }
                        failed?.takeIf { it.label == "$title default" }?.let { f ->
                            SaveError("Could not save. The group is unchanged.", f) { save(it.changes, it.label) }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Note(text: String, tag: String) {
    val c = setupColors
    Text(
        text, color = c.sub, fontSize = 14.sp,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp).fillMaxWidth().background(c.fill, RoundedCornerShape(16.dp)).padding(16.dp).testTag(tag),
    )
}

@Composable
private fun RetryButton(onClick: () -> Unit) {
    val c = setupColors
    Box(Modifier.heightIn(min = 48.dp).clickable(role = Role.Button, onClick = onClick).padding(horizontal = 10.dp).testTag("toolPolicyRetry"), contentAlignment = Alignment.Center) {
        Text("Retry", color = c.errFg, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
private fun SaveError(message: String, pending: PendingSave, retry: (PendingSave) -> Unit) {
    val c = setupColors
    Row(
        Modifier.padding(horizontal = 16.dp, vertical = 2.dp).fillMaxWidth().background(c.errBg, RoundedCornerShape(12.dp)).padding(start = 12.dp)
            .semantics { contentDescription = message },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(message, color = c.errFg, fontSize = 13.sp, modifier = Modifier.weight(1f))
        RetryButton { retry(pending) }
    }
}

/** One row: label and a three-way Ask / Allow / Block control with 48 dp targets. [value] null = mixed (nothing selected). */
@Composable
private fun PolicyRow(label: String, value: ToolPolicy?, enabled: Boolean, tag: String, onPick: (ToolPolicy) -> Unit) {
    val c = setupColors
    Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(label, color = AllternitTheme.colors.textPrimary, fontSize = 15.sp, maxLines = 2, modifier = Modifier.weight(1f))
        Row(Modifier.width(204.dp).background(c.fill2, RoundedCornerShape(24.dp)).padding(2.dp).testTag(tag)) {
            for (p in listOf(ToolPolicy.Ask, ToolPolicy.Allow, ToolPolicy.Block)) {
                val on = p == value
                Box(
                    Modifier.weight(1f).heightIn(min = 48.dp)
                        .background(if (on) c.card else androidx.compose.ui.graphics.Color.Transparent, RoundedCornerShape(22.dp))
                        .selectable(selected = on, enabled = enabled, role = Role.RadioButton) { onPick(p) }
                        .semantics { contentDescription = "${p.label} for $label" },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        p.label, fontSize = 14.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                        color = when {
                            !enabled -> c.mut
                            on && p == ToolPolicy.Block -> c.errFg
                            on -> c.ink
                            else -> c.sub
                        },
                    )
                }
            }
        }
    }
}
