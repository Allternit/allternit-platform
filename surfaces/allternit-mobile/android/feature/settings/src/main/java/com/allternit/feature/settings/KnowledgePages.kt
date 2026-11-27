package com.allternit.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.Haptics
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/* Knowledge review (iOS KnowledgeReviewView): Settings > Memory > Knowledge. Routes: `/api/v1/knowledge/...` (knowledge_routes.rs). */

/** One citation on a proposal: a flag or a past-chat observation. */
internal data class KnowledgeEvidence(val flagId: String?, val observationId: String?, val text: String?)

/** A change the curator or an agent wants to make to the knowledge files. */
internal data class KnowledgeProposal(
    val id: String,
    val targetKind: String,
    val targetRef: String,
    val title: String,
    val body: String,
    val base: String?,
    val evidence: List<KnowledgeEvidence>,
)

/** An agent's note that knowledge was missing, wrong or stale. */
internal data class KnowledgeFlag(val id: String, val kind: String, val targetKind: String, val targetRef: String?, val evidence: String, val suggestedFix: String?) {
    val kindLabel: String get() = when (kind) { "missing" -> "Missing"; "wrong" -> "Wrong"; "stale" -> "Out of date"; else -> "Flagged" }
}

internal object KnowledgeReview {
    const val EMPTY_LINE = "Nothing to review. Agents flag missing knowledge while they work; the curator turns flags into proposals overnight."
    const val SKILL_NOTE = "Approving a skill turns it on the next time Gizzi runs on your computer. Gizzi asks first."
    const val STALE_LINE = "That item changed since you loaded this screen. It has been refreshed."

    fun targetLabel(raw: String): String = when (raw) { "skill" -> "Skill"; "domain" -> "Business knowledge"; "data_access" -> "Data access"; else -> "Knowledge" }

    fun isStale(r: ApiResult<*>): Boolean = r is ApiResult.HttpError && r.status == 409

    fun proposal(o: JsonObject): KnowledgeProposal? {
        val id = o.str("id") ?: return null
        val evidence = ((o["evidence"] as? JsonArray).orEmpty()).mapNotNull { it as? JsonObject }.map {
            KnowledgeEvidence(it.str("flag_id"), it.str("observation_id"), it.str("text") ?: it.str("excerpt") ?: it.str("evidence"))
        }
        return KnowledgeProposal(id, o.str("target_kind").orEmpty(), o.str("target_ref").orEmpty(), o.str("title") ?: "Untitled", o.str("body").orEmpty(), o.str("base"), evidence)
    }

    fun flag(o: JsonObject): KnowledgeFlag? {
        val id = o.str("id") ?: return null
        return KnowledgeFlag(id, o.str("kind").orEmpty(), o.str("target_kind").orEmpty(), o.str("target_ref"), o.str("evidence").orEmpty(), o.str("suggested_fix"))
    }

    enum class Op { Same, Added, Removed }

    data class DiffLine(val op: Op, val text: String)

    /** A line diff of [base] against [body] (longest common subsequence over lines; fine for guide-sized text). */
    fun lineDiff(base: String, body: String): List<DiffLine> {
        val a = base.split("\n")
        val b = body.split("\n")
        val lcs = Array(a.size + 1) { IntArray(b.size + 1) }
        for (i in a.size - 1 downTo 0) for (j in b.size - 1 downTo 0) {
            lcs[i][j] = if (a[i] == b[j]) lcs[i + 1][j + 1] + 1 else maxOf(lcs[i + 1][j], lcs[i][j + 1])
        }
        val out = ArrayList<DiffLine>()
        var i = 0
        var j = 0
        while (i < a.size && j < b.size) {
            if (a[i] == b[j]) { out += DiffLine(Op.Same, a[i]); i++; j++ }
            else if (lcs[i + 1][j] >= lcs[i][j + 1]) { out += DiffLine(Op.Removed, a[i]); i++ }
            else { out += DiffLine(Op.Added, b[j]); j++ }
        }
        while (i < a.size) { out += DiffLine(Op.Removed, a[i]); i++ }
        while (j < b.size) { out += DiffLine(Op.Added, b[j]); j++ }
        return out
    }
}

/** The proposal being reviewed, a line for the list, and a counter that reloads the list (pages push by name only). */
private object KnowledgeSelection {
    var proposal by mutableStateOf<KnowledgeProposal?>(null)
    var notice by mutableStateOf<String?>(null)
    var version by mutableIntStateOf(0)
}

private suspend fun loadKnowledge(): Load<Pair<List<KnowledgeProposal>, List<KnowledgeFlag>>> {
    val p = Net.get("knowledge/proposals?status=proposed")
    val f = Net.get("knowledge/flags?status=open")
    val pl = p.toLoad("load knowledge")
    if (pl !is Load.Ready) return pl as Load<Nothing>
    val fl = f.toLoad("load knowledge")
    if (fl !is Load.Ready) return fl as Load<Nothing>
    return Load.Ready(pl.value.listUnder("proposals").mapNotNull(KnowledgeReview::proposal) to fl.value.listUnder("flags").mapNotNull(KnowledgeReview::flag))
}

/** The "Knowledge" row in Settings > Memory: open flags plus proposals waiting for review. A failed count shows no badge. */
@Composable
internal fun MemoryKnowledgeRow(push: (String) -> Unit) {
    val c = AllternitTheme.colors
    val (count, _) = rememberLoad("knowledge-count-${KnowledgeSelection.version}") {
        val p = Net.get("knowledge/proposals?status=proposed")
        val f = Net.get("knowledge/flags?status=open")
        val n = (if (p is ApiResult.Ok) p.value.listUnder("proposals").size else 0) + (if (f is ApiResult.Ok) f.value.listUnder("flags").size else 0)
        Load.Ready(if (p !is ApiResult.Ok && f !is ApiResult.Ok) null else n)
    }
    val n = (count as? Load.Ready)?.value ?: 0
    SCard {
        SRow("Knowledge", "memory_knowledge_row", icon = DesignR.drawable.ph_books, detail = "Review what agents want to learn", onClick = { push("knowledge") },
            trailing = {
                if (n > 0) Text(
                    "$n", color = Color.White, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(RoundedCornerShape(50)).background(c.accentPrimary).padding(horizontal = 8.dp, vertical = 2.dp).testTag("memory_knowledge_badge"),
                )
                Icon(painterResource(DesignR.drawable.ph_caret_right), contentDescription = null, tint = c.textTertiary, modifier = Modifier.padding(start = 8.dp).size(14.dp))
            })
    }
}

/** Settings > Memory > Knowledge: proposals waiting for review and the flags agents raised. */
@Composable
internal fun KnowledgeReviewPage(back: () -> Unit, push: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val (load, reload) = rememberLoad("knowledge-${KnowledgeSelection.version}") { loadKnowledge() }
    var busy by remember { mutableStateOf<String?>(null) }
    var notice by remember { mutableStateOf(KnowledgeSelection.notice) }
    SPage("Knowledge", tag = "settingsKnowledge", onBack = { KnowledgeSelection.notice = null; back() }) {
        notice?.let { SNote(it) }
        LoadView(load, reload, "load knowledge") { (proposals, flags) ->
            if (proposals.isEmpty() && flags.isEmpty()) SNote(KnowledgeReview.EMPTY_LINE, art = com.allternit.core.design.EmptyArtKind.Sheets)
            if (proposals.isNotEmpty()) {
                SLabel("Waiting for review")
                SCard {
                    proposals.forEachIndexed { i, p ->
                        if (i > 0) SDivider()
                        SRow(
                            p.title, "knowledge_proposal_${p.id}",
                            detail = "${KnowledgeReview.targetLabel(p.targetKind)} · ${p.targetRef}\n" + if (p.evidence.isEmpty()) "No evidence listed" else "Based on ${p.evidence.size} ${if (p.evidence.size == 1) "item" else "items"}",
                            onClick = { KnowledgeSelection.proposal = p; KnowledgeSelection.notice = null; push("knowledge-proposal") },
                        )
                    }
                }
            }
            if (flags.isNotEmpty()) {
                SLabel("Flagged by agents")
                SCard {
                    flags.forEachIndexed { i, f ->
                        if (i > 0) SDivider()
                        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text("${f.kindLabel} · ${KnowledgeReview.targetLabel(f.targetKind)}${f.targetRef?.let { " · $it" } ?: ""}", color = AllternitTheme.colors.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                            Text(f.evidence, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, lineHeight = 18.sp)
                            if (!f.suggestedFix.isNullOrBlank()) Text("Suggested fix: ${f.suggestedFix}", color = AllternitTheme.colors.textPrimary, fontSize = 13.sp, lineHeight = 18.sp)
                            Row {
                                SButton(if (busy == f.id) "Dismissing..." else "Dismiss", "knowledge_dismiss_${f.id}", enabled = busy == null) {
                                    busy = f.id
                                    scope.launch {
                                        val r = Net.send("POST", "knowledge/flags/${LibraryNet.escape(f.id)}/dismiss")
                                        busy = null
                                        if (r is ApiResult.Ok) { Haptics.perform(view, Haptics.Kind.Success); notice = null }
                                        else { Haptics.perform(view, Haptics.Kind.Warning); notice = if (KnowledgeReview.isStale(r)) KnowledgeReview.STALE_LINE else r.friendly("dismiss that flag") }
                                        reload()
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** One proposal: body, a line diff against `base`, evidence, and Approve (with a confirm) and Reject. */
@Composable
internal fun KnowledgeProposalPage(back: () -> Unit) {
    val proposal = KnowledgeSelection.proposal
    if (proposal == null) { androidx.compose.runtime.LaunchedEffect(Unit) { back() }; return }
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val c = AllternitTheme.colors
    var confirm by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    fun decide(approve: Boolean) {
        busy = true
        error = null
        scope.launch {
            val r = Net.send("POST", "knowledge/proposals/${LibraryNet.escape(proposal.id)}/${if (approve) "approve" else "reject"}")
            busy = false
            when {
                r is ApiResult.Ok -> { Haptics.perform(view, Haptics.Kind.Success); KnowledgeSelection.notice = null; KnowledgeSelection.version++; back() }
                KnowledgeReview.isStale(r) -> { Haptics.perform(view, Haptics.Kind.Warning); KnowledgeSelection.notice = KnowledgeReview.STALE_LINE; KnowledgeSelection.version++; back() }
                else -> { Haptics.perform(view, Haptics.Kind.Warning); error = r.friendly(if (approve) "approve that change" else "reject that change") }
            }
        }
    }
    SPage("Review", tag = "settingsKnowledgeProposal", onBack = back) {
        Column(Modifier.padding(horizontal = 6.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(proposal.title, color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
            Text("${KnowledgeReview.targetLabel(proposal.targetKind)} · ${proposal.targetRef}", color = c.textSecondary, fontSize = 13.sp)
        }
        if (!proposal.base.isNullOrEmpty()) {
            SLabel("What changes")
            SCard {
                KnowledgeReview.lineDiff(proposal.base, proposal.body).forEach { line ->
                    val (prefix, tint) = when (line.op) {
                        KnowledgeReview.Op.Added -> "+ " to Color(0xFF2E9E5B).copy(alpha = 0.14f)
                        KnowledgeReview.Op.Removed -> "- " to Color(0xFFD64545).copy(alpha = 0.14f)
                        KnowledgeReview.Op.Same -> "  " to Color.Transparent
                    }
                    Text(prefix + line.text, color = c.textPrimary, fontSize = 12.sp, lineHeight = 17.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.fillMaxWidth().background(tint).padding(horizontal = 12.dp, vertical = 2.dp))
                }
            }
        } else {
            SLabel("Proposed text")
            SCard { Text(proposal.body, color = c.textPrimary, fontSize = 12.sp, lineHeight = 17.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(12.dp)) }
        }
        if (proposal.evidence.isNotEmpty()) {
            SLabel("Evidence")
            SCard {
                proposal.evidence.forEachIndexed { i, e ->
                    if (i > 0) SDivider()
                    Text(e.text ?: if (e.flagId != null) "An agent flag" else "A past chat", color = c.textSecondary, fontSize = 13.sp, lineHeight = 18.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp))
                }
            }
        }
        SError(error)
        Box(Modifier.padding(top = 12.dp), contentAlignment = Alignment.Center) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Approve", "knowledge_approve", enabled = !busy) { confirm = true }
                SButton("Reject", "knowledge_reject", enabled = !busy, destructive = true) { decide(false) }
            }
        }
        if (proposal.targetKind == "skill") SNote(KnowledgeReview.SKILL_NOTE)
        if (confirm) AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Approve this change?") },
            text = { Text(if (proposal.targetKind == "skill") KnowledgeReview.SKILL_NOTE else "It is saved to your knowledge files.") },
            confirmButton = { TextButton(onClick = { confirm = false; decide(true) }) { Text("Approve") } },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } },
        )
    }
}
