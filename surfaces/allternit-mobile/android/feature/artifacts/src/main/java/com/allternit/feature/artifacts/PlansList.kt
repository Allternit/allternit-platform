package com.allternit.feature.artifacts

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

private const val NO_PROJECT = "No project"

/** Plans grouped by `origin.project_title` (else "No project"), plans newest first, "No project" last (web `groupPlans`). */
fun groupPlans(items: List<ArtifactSummary>): List<Pair<String, List<ArtifactSummary>>> {
    val sorted = items.sortedByDescending { parseTime(it.updatedAt) }
    return sorted.groupBy { it.origin?.projectTitle?.trim()?.takeIf { t -> t.isNotEmpty() } ?: NO_PROJECT }
        .toList().sortedBy { if (it.first == NO_PROJECT) 1 else 0 }
}

/**
 * Plans chip content (Artifacts redesign 6.6; web `PlansList.tsx`, iOS PlansListView). Reads
 * `GET api/v2/artifacts?kind=plan&scope=all`: the server leaves plans out of every other list, so this
 * is the only place they show. States: loading, error with Retry, empty, list.
 */
@Composable
fun PlansList(onOpen: (String) -> Unit, reloadKey: Any? = null, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    var plans by remember { mutableStateOf<List<ArtifactSummary>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(attempt, reloadKey) {
        when (val r = ArtifactsV2Client.list("all", "plan", null, null, false)) {
            is ApiResult.Ok -> { plans = r.value.items.filter { it.kind == "plan" }; error = null }
            else -> error = r.friendly("load your plans")
        }
    }
    Column(modifier.fillMaxWidth().testTag("plansList"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(painterResource(DesignR.drawable.ph_info), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
            Text("Plans stay here. They don't show in Everything or Recent.", color = c.textTertiary, fontSize = 12.sp)
        }
        val list = plans
        when {
            list == null && error == null -> Text("Loading plans…", color = c.textTertiary, fontSize = 13.sp,
                modifier = Modifier.semantics { contentDescription = "Loading plans" })
            list == null -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(error ?: "Couldn't load your plans.", color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.weight(1f, fill = false))
                Text("Retry", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                    modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).background(c.fillNeutral).testTag("plansRetry")
                        .clickable { error = null; attempt++ }.padding(horizontal = 14.dp, vertical = 12.dp))
            }
            list.isEmpty() -> Text("No plans yet. Plans the agent writes show up here.", color = c.textTertiary, fontSize = 13.sp)
            else -> groupPlans(list).forEach { (project, group) ->
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.Bottom) {
                        Text(project, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                        Text("${group.size} ${if (group.size == 1) "plan" else "plans"}", color = c.textTertiary, fontSize = 12.sp)
                    }
                    val shape = RoundedCornerShape(14.dp)
                    Column(Modifier.fillMaxWidth().clip(shape).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), shape)) {
                        group.forEachIndexed { i, p -> PlanRow(p, i > 0, onOpen) }
                    }
                }
            }
        }
    }
}

@Composable
private fun PlanRow(p: ArtifactSummary, divider: Boolean, onOpen: (String) -> Unit) {
    val c = AllternitTheme.colors
    val (label, tint) = when (p.planStatus) {
        "approved" -> "Approved" to AllternitStatic.StatusSuccess
        "done" -> "Done" to c.textSecondary
        else -> "Open" to AllternitStatic.StatusInfo
    }
    val chat = p.origin?.sessionTitle?.trim()?.takeIf { it.isNotEmpty() }
    val updated = relativeTime(parseTime(p.updatedAt)).ifEmpty { "recently" }
    Column {
        if (divider) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
        Row(
            Modifier.fillMaxWidth().heightIn(min = 56.dp).testTag("plan${p.id}")
                .clickable(onClickLabel = "Open ${p.title}") { onOpen(p.id) }.padding(horizontal = 12.dp, vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Box(Modifier.size(30.dp).clip(RoundedCornerShape(8.dp)).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(8.dp)), contentAlignment = Alignment.Center) {
                Icon(painterResource(DesignR.drawable.ph_list_bullets), null, tint = c.textSecondary, modifier = Modifier.size(16.dp))
            }
            Column(Modifier.weight(1f)) {
                Text(p.title, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (chat != null) Text("Chat: $chat", color = c.textTertiary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text("v${p.currentVersion} · Updated $updated", color = c.textTertiary, fontSize = 12.sp, maxLines = 1)
            }
            Text(label, color = tint, fontSize = 11.5.sp, fontWeight = FontWeight.Medium,
                modifier = Modifier.clip(CircleShape).background(tint.copy(alpha = 0.16f)).padding(horizontal = 8.dp, vertical = 2.dp))
            Text("Open", color = c.textPrimary, fontSize = 12.5.sp,
                modifier = Modifier.clip(CircleShape).background(c.fillNeutral).padding(horizontal = 12.dp, vertical = 6.dp))
        }
    }
}
