package com.allternit.feature.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import com.allternit.feature.billing.AccountPlan
import com.allternit.feature.billing.OpenModelsStatusRow
import com.allternit.feature.billing.OpenResearchCard
import com.allternit.feature.billing.PlayPurchases
import com.allternit.feature.billing.StoreBilledByView
import com.allternit.feature.billing.StoreKind
import com.allternit.feature.billing.StorePackGrid
import com.allternit.feature.billing.StorePlanTicket
import com.allternit.feature.billing.StorePurchaseNoticeView
import com.allternit.feature.billing.StoreRestoreButton
import com.allternit.feature.billing.StoreTermsView
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.format.DateTimeFormatter
import java.util.Locale

/**
 * Settings → Billing (iOS ComputeBillingView, title "Billing"): the member card, the plan and who bills it, usage
 * bars and credits from the real routes (`/api/v1/me/usage`, `/api/v1/hosted-runtimes/entitlement`,
 * `/api/v1/billing/credits`, `/api/v1/billing/subscription`).
 *
 * Where the server has Google switched on and Play returns the products, plans and credit packs are bought here
 * through Google Play Billing ([PlayPurchases]), priced by Play, with Restore Purchases and the subscription terms.
 * With Google off the page shows plan status only, with no link and no price: what iOS shows where it can sell
 * nothing. A Play app may not link out to buy on the web, so there are no web links.
 */
@Composable
internal fun BillingPage(host: SettingsHost, back: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    LaunchedEffect(Unit) {
        PlayPurchases.start(context)
        PlayPurchases.notice = null
        if (host.account != null) AccountPlan.load()
        PlayPurchases.loadCatalog()
    }
    val sub = AccountPlan.subscription
    SPage("Billing", tag = "settingsBilling", onBack = back) {
        MemberCard(
            plan = AccountPlan.planName ?: "Free",
            member = host.account?.firstName?.trim()?.takeIf { it.isNotEmpty() } ?: "Member",
            renews = if (sub?.isPaid == true) sub.renewsAt else null,
        )
        SLabel("Plan")
        SCard {
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp).testTag("billing_currentPlan"), verticalAlignment = Alignment.CenterVertically) {
                Text("Current plan", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                if (AccountPlan.isLoading && AccountPlan.planName == null) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                } else {
                    Box(Modifier.clip(RoundedCornerShape(50)).background(c.bgSecondary).padding(horizontal = 10.dp, vertical = 4.dp)) {
                        Text(AccountPlan.planName ?: "Free", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    }
                }
            }
            val status = sub?.status
            if (sub?.isPaid == true && status != null && status != "active") {
                Text(statusCopy(status), color = if (status == "trialing") c.textSecondary else Color(0xFFB7791F), fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp).padding(bottom = 10.dp))
            }
            if (sub?.billedByLine != null) {
                SDivider()
                Box(Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) { StoreBilledByView(sub) }
            }
        }
        SLabel("Usage")
        SCard { Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) { PlanUsageRows() } }

        // Google Play Billing: plan tickets priced by Play, while no plan is billed on the web or by Apple (D5).
        if (PlayPurchases.showsStoreButtons(StoreKind.Plan) && PlayPurchases.planPurchaseAllowed(sub)) {
            SLabel(if (sub?.isPaid == true) "Change plan" else "Choose a plan")
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                for (item in PlayPurchases.plans) StorePlanTicket(item)
                OpenResearchCard()
            }
        }
        if (PlayPurchases.showsStoreButtons(StoreKind.Pack)) {
            SLabel("Buy credits")
            StorePackGrid()
        }
        if (PlayPurchases.showsStoreButtons()) {
            Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                StorePurchaseNoticeView()
                StoreRestoreButton()
                StoreTermsView()
            }
        }
    }
}

private fun statusCopy(status: String): String = when (status) {
    "trialing" -> "Free trial"
    "past_due" -> "Payment past due."
    "unpaid" -> "Payment failed. Your plan is paused."
    else -> status.replaceFirstChar { it.uppercase() }
}

/** iOS MemberCardView: the plan as a card with the member's name and the renewal date. */
@Composable
private fun MemberCard(plan: String, member: String, renews: String?) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().padding(vertical = 8.dp).clip(RoundedCornerShape(18.dp)).background(c.textPrimary).padding(20.dp).testTag("billing_memberCard"),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text("ALLTERNIT", color = c.bgPrimary.copy(alpha = 0.7f), fontSize = 11.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 2.sp)
        Text(plan, color = c.bgPrimary, fontSize = 24.sp, fontWeight = FontWeight.SemiBold)
        Row(Modifier.fillMaxWidth().padding(top = 18.dp)) {
            Text(member, color = c.bgPrimary, fontSize = 14.sp, modifier = Modifier.weight(1f))
            renews?.let { shortDate(it) }?.let { Text("Renews $it", color = c.bgPrimary.copy(alpha = 0.8f), fontSize = 13.sp) }
        }
    }
}

private fun shortDate(raw: String): String? = runCatching {
    val date = runCatching { OffsetDateTime.parse(raw).toLocalDate() }.getOrElse { LocalDate.parse(raw.take(10)) }
    date.format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
}.getOrNull()

/** iOS PlanUsageRows: the monthly Cloud allowance, cloud computer hours and the credit balance. */
@Composable
private fun PlanUsageRows(showCredits: Boolean = true) {
    val c = AllternitTheme.colors
    val usage = AccountPlan.usage
    if (usage != null && usage.weeklyLimit > 0 && usage.weeklyUsed.isFinite()) {
        UsageBar("Allternit Cloud · this month", "", (usage.weeklyUsed / usage.weeklyLimit).coerceIn(0.0, 1.0), usage.resetsAt?.let(::shortDate)?.let { "Resets $it" })
    }
    val fraction = AccountPlan.cloudHoursFraction
    val detail = AccountPlan.cloudHoursDetail
    if (fraction != null && detail != null) {
        val next = LocalDate.now().withDayOfMonth(1).plusMonths(1).format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
        UsageBar("Cloud computer · this month", detail, fraction.coerceIn(0.0, 1.0), "Resets $next")
    }
    val credits = if (showCredits) AccountPlan.credits else null
    if (credits != null) {
        Row(Modifier.fillMaxWidth().testTag("billing_credits")) {
            Text("Credits", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
            Text(String.format(Locale.US, "$%.2f balance · $%.2f used this month", credits.balanceUsd, credits.monthToDateUsageUsd), color = c.textSecondary, fontSize = 12.sp)
        }
    } else if (showCredits && usage?.credits != null) {
        Row(Modifier.fillMaxWidth().testTag("billing_credits")) {
            Text("Credits", color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
            Text(usage.credits.toString(), color = c.textSecondary, fontSize = 15.sp)
        }
    }
    if (showCredits) OpenModelsStatusRow()
    if ((usage == null || usage.weeklyLimit <= 0) && fraction == null && (credits == null || !showCredits)) {
        if (AccountPlan.isLoading) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                Text("Loading usage…", color = c.textSecondary, fontSize = 15.sp)
            }
        } else {
            Column(Modifier.testTag("billing_usageEmpty")) {
                Text("No usage to show", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text("Usage shows here once your account uses Allternit Cloud.", color = c.textSecondary, fontSize = 13.sp)
            }
        }
    }
}

@Composable
private fun UsageBar(label: String, detail: String, fraction: Double, footnote: String?) {
    val c = AllternitTheme.colors
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(Modifier.fillMaxWidth()) {
            Text(label, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
            if (detail.isNotEmpty()) Text(detail, color = c.textSecondary, fontSize = 12.sp)
        }
        Box(Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(c.bgSecondary)) {
            Box(Modifier.fillMaxWidth(fraction.toFloat()).height(6.dp).clip(RoundedCornerShape(3.dp)).background(if (fraction >= 0.9) Color(0xFFB7791F) else c.textPrimary))
        }
        footnote?.let { Text(it, color = c.textTertiary, fontSize = 11.sp) }
    }
}

/**
 * Settings → Usage (iOS UsagePageView): the Allternit Cloud allowance and cloud computer hours as bars, then the credit
 * balance with Buy credits where Google Play sells packs. No web purchase link (a Play app may not link out).
 */
@Composable
internal fun UsagePage(host: SettingsHost, back: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    LaunchedEffect(Unit) {
        PlayPurchases.start(context)
        if (host.account != null) AccountPlan.load()
        PlayPurchases.loadCatalog()
    }
    SPage("Usage", tag = "settingsUsage", onBack = back) {
        SLabel("Allternit Cloud")
        if (host.account == null) SNote("Sign in to see your plan usage.")
        else SCard { Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) { PlanUsageRows(showCredits = false) } }
        SLabel("Connected accounts")
        ConnectedAccounts(signedIn = host.account != null)
        if (host.account != null) {
            SLabel("Credits")
            SCard {
                Column(Modifier.padding(16.dp).testTag("usage_credits"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    val credits = AccountPlan.credits
                    if (credits != null) {
                        Text(String.format(Locale.US, "$%.2f", credits.balanceUsd), color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
                        Text(String.format(Locale.US, "$%.2f used this month", credits.monthToDateUsageUsd), color = c.textTertiary, fontSize = 12.sp)
                    } else if (AccountPlan.usage?.credits != null) {
                        Text(AccountPlan.usage?.credits.toString(), color = c.textPrimary, fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
                    } else Text(if (AccountPlan.isLoading) "Loading your credits…" else "No credit balance yet.", color = c.textSecondary, fontSize = 13.sp)
                }
            }
            if (PlayPurchases.showsStoreButtons(StoreKind.Pack)) { SLabel("Buy credits"); StorePackGrid() }
        }
        SNote("Bars turn amber as you near a limit and red once it is reached.", Modifier.padding(top = 14.dp))
    }
}

private data class QuotaWindow(val label: String, val ratio: Double, val reset: String?)
private data class AccountCard(val provider: String, val plan: String?, val windows: List<QuotaWindow>)

private fun windowTitle(id: String, label: String): String {
    val text = "$id $label".lowercase()
    val base = if (id == "5h") "5-hour" else if (id.startsWith("7d")) "Weekly" else if (id.startsWith("month")) "Monthly" else label
    for (family in listOf("opus", "sonnet", "haiku", "fable")) if (text.contains(family)) return "$base · ${family.replaceFirstChar { it.uppercase() }}"
    return base
}

private fun parseQuota(provider: String, v: JsonElement): AccountCard? {
    val quota = (v as? JsonObject)?.get("quota") as? JsonObject ?: return null
    val plan = (quota["plan"] as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotEmpty() }
    val windows = (quota["windows"] as? JsonArray).orEmpty().mapNotNull { w ->
        val o = w as? JsonObject ?: return@mapNotNull null
        val id = (o["id"] as? JsonPrimitive)?.contentOrNull.orEmpty()
        val label = (o["label"] as? JsonPrimitive)?.contentOrNull ?: id
        val ratio = ((o["usedRatio"] ?: o["used_ratio"]) as? JsonPrimitive)?.doubleOrNull ?: return@mapNotNull null
        val reset = ((o["resetAt"] ?: o["reset_at"]) as? JsonPrimitive)?.contentOrNull?.let(::shortDate)?.let { "Resets $it" }
        QuotaWindow(windowTitle(id, label), ratio.coerceIn(0.0, 1.0), reset)
    }.sortedBy { if (it.label.startsWith("5-hour")) 0 else if (it.label.startsWith("Weekly")) 1 else 2 }
    return if (plan == null && windows.isEmpty()) null else AccountCard(provider, plan, windows)
}

/**
 * iOS AccountUsageCardsView: the Allternit Cloud card, then one card per connected account that reports a plan or limit
 * windows (`GET providers/:id/quota`), closest to a limit first. A read that fails (no computer, gate refusal) leaves that
 * provider out; with no card at all the section says so in one line.
 */
@Composable
private fun ConnectedAccounts(signedIn: Boolean) {
    val c = AllternitTheme.colors
    var cards by remember { mutableStateOf<List<AccountCard>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    LaunchedEffect(signedIn) {
        loading = true
        val ids = linkedSetOf<String>()
        val models = Net.get("models")
        if (models is ApiResult.Ok) models.value.listUnder("models", "items").forEach { m ->
            val id = m.str("provider") ?: m.str("providerId") ?: return@forEach
            if (id != "allternit" && id != "local" && id != "ollama") ids.add(id)
        }
        val found = ids.mapNotNull { id -> (Net.get("providers/$id/quota") as? ApiResult.Ok)?.value?.let { parseQuota(id, it) } }
        cards = found.sortedByDescending { it.windows.maxOfOrNull { w -> w.ratio } ?: -1.0 }
        loading = false
    }
    val cloudPlan = AccountPlan.usage?.let { u -> if (u.weeklyLimit > 0 && u.weeklyUsed.isFinite()) (u.weeklyUsed / u.weeklyLimit).coerceIn(0.0, 1.0) else null }
    val all = (if (signedIn) listOf(AccountCard("Allternit Cloud", null, listOfNotNull(cloudPlan?.let { QuotaWindow("Monthly", it, AccountPlan.usage?.resetsAt?.let(::shortDate)?.let { r -> "Resets $r" }) }))) else emptyList()) + cards
    Column(Modifier.testTag("usage_connectedAccounts"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (all.isEmpty()) {
            if (loading) Text("Loading connected accounts…", color = c.textSecondary, fontSize = 13.sp)
            else SNote("No connected account reports usage. Subscriptions and API keys you connect show their limits here.", art = com.allternit.core.design.EmptyArtKind.Coins)
        }
        all.forEach { card ->
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgSecondary).padding(12.dp).testTag("usage_account_${card.provider}"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    com.allternit.core.design.AppLogo(card.provider, id = com.allternit.core.design.AppLogoCatalog.providerKey(if (card.provider == "Allternit Cloud") "allternit" else card.provider), size = 24.dp)
                    Text(card.provider.replaceFirstChar { it.uppercase() }, color = c.textPrimary, fontSize = 14.5.sp, fontWeight = FontWeight.SemiBold)
                    card.plan?.let { Text(it, color = c.textSecondary, fontSize = 10.5.sp, modifier = Modifier.clip(RoundedCornerShape(50)).background(c.fillNeutral).padding(horizontal = 7.dp, vertical = 1.dp)) }
                }
                if (card.windows.isEmpty()) Text("This account doesn't report limits.", color = c.textTertiary, fontSize = 12.sp)
                card.windows.forEach { w -> UsageBar(w.label, "${(w.ratio * 100).toInt()}%", w.ratio, w.reset) }
            }
        }
    }
}
