package com.allternit.feature.billing

import android.content.Context
import android.content.Intent
import android.net.Uri
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.util.Locale

/*
 * Allternit Open on Android (CONTRACT-open-tier-2026-11-26, same rows and copy as iOS I1). Android has no storefront
 * check, so a row shows by terms alone (D14). No host names in any text here (D15).
 */

object OpenModelsCopy {
    const val TERMS_URL = "https://ai.allternit.com/settings/open-models"
    const val TERMS_ROW = "Accept terms on the web"
    const val TERMS_DETAIL = "Open models need one-time terms, accepted on the web."
    const val GROUP = "Open models · via Allternit Cloud"
    const val SUBTITLE = "Far fewer refusals"
    const val STATUS_TITLE = "Open models"
    const val RESEARCH_URL = "https://services.allternit.com"
    const val RESEARCH_TITLE = "OPEN RESEARCH"
    const val RESEARCH_PRICE = "By agreement"
    const val RESEARCH_LINE_1 = "Larger models, higher rate limits"
    const val RESEARCH_LINE_2 = "For verified security and red teams"
    const val RESEARCH_OPEN_LINE = "Open models: all, on agreed credits"
    const val RESEARCH_ACTION = "Ask us"
    const val RESEARCH_ACTIVE = "Active on your account"

    /** CONTRACT §4 messages. */
    const val MSG_PLAN_REQUIRED = "Open models on Allternit Cloud need a paid plan. On Free, run open models on your own device with Freeweights."
    const val MSG_ALLOWANCE_USED = "This month's open-model allowance is used. Add credits to keep going."
    const val MSG_BOUGHT_REQUIRED = "On Plus, open models run on credits you buy. Add credits to use them."
    const val MSG_RESEARCH_REQUIRED = "This model is part of Open Research, available by agreement."

    /** The §4 sentence for an open lock reason, or null when the reason is not an open one. */
    fun message(reason: String?): String? = when (reason) {
        "plan_required", "open_plan_required" -> MSG_PLAN_REQUIRED
        "allowance_used" -> MSG_ALLOWANCE_USED
        "bought_credits_required" -> MSG_BOUGHT_REQUIRED
        "open_research_required" -> MSG_RESEARCH_REQUIRED
        else -> null
    }

    fun title(reason: String?): String? = when (reason) {
        "plan_required", "open_plan_required" -> "Open models need a paid plan"
        "allowance_used", "bought_credits_required" -> "Add credits for open models"
        "open_research_required" -> "Open Research"
        else -> null
    }

    /** What a plan card says about open models: Free is on-device, Plus runs on bought credits, Max/Ultra have plan credits. */
    fun planLine(planId: String): String? = when (planId) {
        "free" -> "Open models: on your device"
        "plus" -> "Open models: with credits you buy"
        "super", "max" -> "Open models: up to \$55/mo of plan credits"
        "ultra" -> "Open models: up to \$110/mo of plan credits"
        else -> null
    }

    /** The reason in a relayed chat error (402 `open_plan_required` / `open_lane_requires_credits`), or null. */
    fun reason(text: String?): String? {
        val t = text?.lowercase()?.takeIf { it.isNotBlank() } ?: return null
        if ("open_plan_required" in t || "open models on allternit cloud need a paid plan" in t) return "plan_required"
        if ("allowance_used" in t || "open-model allowance is used" in t) return "allowance_used"
        if ("bought_credits_required" in t || "open_lane_requires_credits" in t || "open models run on credits you buy" in t) return "bought_credits_required"
        if ("open_research_required" in t || "part of open research" in t) return "open_research_required"
        return null
    }
}

object OpenModelsGate {
    /** `open_models` from the credits read, else the usage read; null on an older backend (no open rows, as today). */
    val entitlement: OpenModelsEntitlement? get() = AccountPlan.credits?.openModels ?: AccountPlan.usage?.openModels

    /** Open rows list only after terms (D14). Research rows also need the flag. */
    fun showsRow(access: String?): Boolean {
        val e = entitlement ?: return false
        return e.termsAccepted && (access != "research" || e.research)
    }

    /** True when the account has not accepted terms: the list shows one link row instead of open rows. */
    fun needsTerms(): Boolean = entitlement?.let { !it.termsAccepted } == true

    /** A locked open row opens the credits sheet with the §4 reason; null when the row is open to them. */
    fun lockedRequest(modelName: String?, inputPer1mUsd: Double?, outputPer1mUsd: Double?): CloudCreditsRequest? {
        val e = entitlement ?: return null
        if (e.allowed) return null
        val reason = e.reason?.takeIf { OpenModelsCopy.message(it) != null } ?: return null
        return CloudCreditsRequest(reason, modelName, inputPer1mUsd, outputPer1mUsd)
    }
}

/** Opens the web terms page in the browser (an Intent, so the app's chosen browser handles it). */
fun openOpenModelsTerms(context: Context) = openUrl(context, OpenModelsCopy.TERMS_URL)

private fun openUrl(context: Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

private fun shortDate(raw: String): String? = runCatching {
    val date = runCatching { OffsetDateTime.parse(raw).toLocalDate() }.getOrElse { LocalDate.parse(raw.take(10)) }
    date.format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
}.getOrNull()

private fun money(v: Double) = String.format(Locale.US, "$%.2f", v)

/** The "Accept terms on the web" row, drawn in the model list's row style. */
@Composable
fun OpenTermsRow(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    Column(
        modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).testTag("openModelsTermsRow")
            .clickable(onClickLabel = OpenModelsCopy.TERMS_ROW) { openOpenModelsTerms(context) }
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(OpenModelsCopy.TERMS_ROW, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
        Text(OpenModelsCopy.TERMS_DETAIL, color = c.textSecondary, fontSize = 11.sp)
    }
}

/**
 * Open models status: plan allowance used / allowance with a meter and the period end (Max, Ultra), plus bought credits.
 * Draws nothing before terms or on an older backend.
 */
@Composable
fun OpenModelsStatusRow(modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val e = OpenModelsGate.entitlement ?: return
    if (!e.termsAccepted) return
    Column(modifier.fillMaxWidth().testTag("openModelsStatus"), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(OpenModelsCopy.STATUS_TITLE, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
            if (e.planAllowanceUsd > 0) Text("${money(e.usedUsd)} of ${money(e.planAllowanceUsd)}", color = c.textSecondary, fontSize = 12.sp)
        }
        if (e.planAllowanceUsd > 0) {
            val fraction = (e.usedUsd / e.planAllowanceUsd).coerceIn(0.0, 1.0)
            Box(Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(c.bgSecondary)) {
                Box(Modifier.fillMaxWidth(fraction.toFloat()).height(6.dp).clip(RoundedCornerShape(3.dp)).background(if (fraction >= 0.9) Color(0xFFB7791F) else c.textPrimary))
            }
            e.periodEnd?.let(::shortDate)?.let { Text("Resets $it", color = c.textTertiary, fontSize = 11.sp) }
        }
        Text("${money(e.boughtBalanceUsd)} bought credits", color = c.textSecondary, fontSize = 12.sp)
    }
}

/** Open Research as a fifth plan card, in the same ticket style as [StorePlanTicket]. */
@Composable
fun OpenResearchCard() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val active = OpenModelsGate.entitlement?.research == true
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPrimary).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp))
            .padding(14.dp).testTag("openResearchCard"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(OpenModelsCopy.RESEARCH_TITLE, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            Text(OpenModelsCopy.RESEARCH_PRICE, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        }
        Text(OpenModelsCopy.RESEARCH_LINE_1, color = c.textSecondary, fontSize = 13.sp)
        Text(OpenModelsCopy.RESEARCH_LINE_2, color = c.textSecondary, fontSize = 13.sp)
        Text(OpenModelsCopy.RESEARCH_OPEN_LINE, color = c.textSecondary, fontSize = 13.sp)
        if (active) {
            Text(OpenModelsCopy.RESEARCH_ACTIVE, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        } else {
            Box(
                Modifier.fillMaxWidth().heightIn(min = 40.dp).clip(RoundedCornerShape(10.dp)).background(c.textPrimary)
                    .testTag("openResearchAsk").clickable { openUrl(context, OpenModelsCopy.RESEARCH_URL) },
                contentAlignment = Alignment.Center,
            ) { Text(OpenModelsCopy.RESEARCH_ACTION, color = c.bgPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
        }
    }
}
