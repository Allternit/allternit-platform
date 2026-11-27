package com.allternit.feature.settings

import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.str
import com.allternit.feature.billing.AccountPlan
import com.allternit.feature.billing.OpenModelsStatusRow
import com.allternit.feature.billing.OpenResearchCard
import com.allternit.feature.billing.PlayPurchases
import com.allternit.feature.billing.StoreKind
import com.allternit.feature.billing.StorePackGrid
import com.allternit.feature.billing.StorePlanTicket
import com.allternit.feature.billing.StorePurchaseNoticeView
import com.allternit.feature.billing.StoreRestoreButton
import com.allternit.feature.billing.StoreTermsView
import com.allternit.feature.billing.StoreViewCopy
import com.allternit.feature.billing.openStoreLink
import com.allternit.feature.billing.playSubscriptionsUrl
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull
import java.net.URLEncoder
import java.time.Duration
import java.time.Instant
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/*
 * Google Play Payments policy (the reason this file looks the way it does): a Play app sells plans and credits only
 * through Play Billing. So these pages carry no card form, no card update, no payment-method list and no link out to
 * pay on the web. Plans and packs are bought through [PlayPurchases]; "Manage subscription" opens Play's own
 * subscription center. Stripe invoices are shown read only: a paid invoice's PDF opens in a Custom Tab, and an open
 * invoice shows its status with no PDF (a Stripe PDF of an open invoice carries a pay link).
 */

private val Amber = Color(0xFFB7791F)

/**
 * Settings → Billing (iOS ComputeBillingView, web Billing frame `M1-pay-billing-phone`): the member card, the plan row
 * (what it includes, when it renews, Manage subscription in Play), credits, then read-only invoices
 * (`GET /api/v1/billing/invoices`). With Google on, plans and packs are bought here through Play Billing.
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
        // Plan row: "<Plan> plan", what it includes, the renewal or status line, who bills it.
        Column(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 12.dp).testTag("billing_currentPlan"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (AccountPlan.isLoading && AccountPlan.planName == null) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
            } else {
                val name = AccountPlan.planName ?: "Free"
                Text(if (name.endsWith("plan", ignoreCase = true)) name else "$name plan", color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                Text(
                    if (sub?.isPaid == true) "Monthly Allternit Cloud credits and a cloud computer" else "Chat with your own models and keys. Upgrade for Allternit Cloud credits.",
                    color = c.textPrimary, fontSize = 15.sp, lineHeight = 21.sp,
                )
                val status = sub?.status
                when {
                    sub?.isPaid == true && status != null && status != "active" ->
                        Text(statusCopy(status), color = if (status == "trialing") c.textSecondary else Amber, fontSize = 13.sp)
                    sub?.isPaid == true -> longDate(sub.renewsAt)?.let { Text("Your subscription will auto renew on $it.", color = c.textSecondary, fontSize = 13.sp) }
                }
                if (sub?.conflict == true) Text(StoreViewCopy.CONFLICT, color = Amber, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                // Apple and web plans: say who bills it, never a link (a Play app does not link out to other billing).
                if (sub?.billingProvider != "google") sub?.billedByLine?.let { Text(it, color = c.textSecondary, fontSize = 13.sp) }
                if (sub?.isPaid == true && sub.billingProvider == "google") {
                    val productId = PlayPurchases.items.firstOrNull { it.kind == StoreKind.Plan && it.id == sub.planId }?.productId
                    Box(Modifier.padding(top = 8.dp)) {
                        SButton("Manage subscription", tag = "billing_manageSubscription") { openStoreLink(context, playSubscriptionsUrl(productId)) }
                    }
                }
            }
        }
        SDivider()
        CreditsRow()
        // Allternit Open status meter (absent until open_models terms are accepted).
        OpenModelsStatusRow(Modifier.padding(start = 6.dp, end = 6.dp, bottom = 14.dp))
        SDivider()

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
        if (host.account != null) Invoices()
        if (PlayPurchases.showsStoreButtons()) {
            Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                StorePurchaseNoticeView()
                StoreRestoreButton()
                StoreTermsView()
            }
        }
    }
}

@Composable
private fun CreditsRow() {
    val c = AllternitTheme.colors
    val credits = AccountPlan.credits
    val legacy = AccountPlan.usage?.credits
    val detail = when {
        credits != null -> String.format(Locale.US, "$%.2f available for any task.", credits.balanceUsd)
        legacy != null -> String.format(Locale.US, "$%.2f available for any task.", legacy)
        AccountPlan.isLoading -> "Loading your credits…"
        else -> "No credit balance yet."
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 14.dp).testTag("billing_credits"), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text("Credits", color = c.textPrimary, fontSize = 16.sp)
        Text(detail, color = c.textSecondary, fontSize = 14.sp)
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

private fun parseInstant(raw: String?): Instant? {
    if (raw.isNullOrBlank()) return null
    raw.toLongOrNull()?.let { return Instant.ofEpochSecond(it) }
    return runCatching { OffsetDateTime.parse(raw).toInstant() }.getOrNull()
        ?: runCatching { LocalDate.parse(raw.take(10)).atStartOfDay(ZoneId.systemDefault()).toInstant() }.getOrNull()
}

private fun fmt(raw: String?, pattern: String): String? =
    parseInstant(raw)?.atZone(ZoneId.systemDefault())?.format(DateTimeFormatter.ofPattern(pattern, Locale.US))

private fun shortDate(raw: String): String? = fmt(raw, "MMM d")
private fun longDate(raw: String?): String? = fmt(raw, "MMM d, yyyy")

// ---- Invoices (read only) ----

private data class Invoice(val id: String, val created: String?, val totalUsd: Double, val currency: String, val status: String, val pdfUrl: String?)

private fun parseInvoices(v: JsonElement): Pair<List<Invoice>, String?> {
    val o = v as? JsonObject ?: return emptyList<Invoice>() to null
    val list = (o["invoices"] as? JsonArray).orEmpty().mapNotNull { e ->
        val i = e as? JsonObject ?: return@mapNotNull null
        val id = (i["id"] as? JsonPrimitive)?.contentOrNull ?: return@mapNotNull null
        val created = (i["created_at"] as? JsonPrimitive)?.let { p -> p.longOrNull?.toString() ?: p.contentOrNull }
        Invoice(
            id = id,
            created = created,
            totalUsd = (i["total_usd"] as? JsonPrimitive)?.doubleOrNull ?: 0.0,
            currency = (i["currency"] as? JsonPrimitive)?.contentOrNull?.uppercase() ?: "USD",
            status = (i["status"] as? JsonPrimitive)?.contentOrNull ?: "",
            pdfUrl = (i["pdf_url"]?.takeIf { it !is JsonNull } as? JsonPrimitive)?.contentOrNull?.takeIf { it.startsWith("https://") },
        )
    }
    val cursor = (o["cursor"]?.takeIf { it !is JsonNull } as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotEmpty() }
    return list to cursor
}

private fun invoiceStatus(status: String): String = when (status) {
    "paid" -> "Paid"
    "open" -> "Due"
    "void" -> "Void"
    "uncollectible" -> "Unpaid"
    "draft" -> "Draft"
    else -> status.replaceFirstChar { it.uppercase() }
}

/** The PDF of a finished invoice. An open invoice's Stripe PDF has a pay link, so it gets none. */
private fun Invoice.readablePdf(): String? = pdfUrl?.takeIf { status == "paid" || status == "void" }

/**
 * Invoices from Stripe-billed plans, read only: date, status and total; a paid one opens its PDF in a Custom Tab
 * inside the app (never the browser, never a pay page). No section at all when the account has none or the server
 * has no invoices route yet (404).
 */
@Composable
private fun Invoices() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var items by remember { mutableStateOf<List<Invoice>>(emptyList()) }
    var cursor by remember { mutableStateOf<String?>(null) }
    var state by remember { mutableStateOf("loading") } // loading | ready | failed | absent
    var more by remember { mutableStateOf(false) }
    var nonce by remember { mutableStateOf(0) }
    LaunchedEffect(nonce) {
        state = "loading"
        state = when (val r = Net.get("billing/invoices?limit=12")) {
            is ApiResult.Ok -> { val (list, next) = parseInvoices(r.value); items = list; cursor = next; "ready" }
            is ApiResult.HttpError -> if (r.status == 404 || r.status == 401) "absent" else "failed"
            else -> "failed"
        }
    }
    if (state == "absent" || (state == "ready" && items.isEmpty())) return
    SLabel("Invoices")
    Column(Modifier.fillMaxWidth().testTag("billing_invoices")) {
        when (state) {
            "loading" -> Row(Modifier.padding(horizontal = 6.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                Text("Loading invoices…", color = c.textSecondary, fontSize = 14.sp)
            }
            "failed" -> {
                SError("Couldn't load your invoices.")
                Box(Modifier.padding(horizontal = 6.dp)) { SButton("Try again", tag = "billing_invoicesRetry") { nonce += 1 } }
            }
            else -> {
                items.forEachIndexed { index, inv ->
                    if (index > 0) SDivider()
                    val pdf = inv.readablePdf()
                    SRow(
                        title = fmt(inv.created, "MMM d, yyyy") ?: "Invoice",
                        tag = "billing_invoice_${inv.id}",
                        detail = invoiceStatus(inv.status) + if (pdf != null) " · PDF" else "",
                        value = if (inv.currency == "USD") String.format(Locale.US, "$%.2f", inv.totalUsd) else String.format(Locale.US, "%.2f %s", inv.totalUsd, inv.currency),
                        chevron = pdf != null,
                        onClick = pdf?.let { url -> { openInAppTab(context, url) } },
                    )
                }
                val next = cursor
                if (next != null) {
                    Box(Modifier.padding(horizontal = 6.dp, vertical = 10.dp)) {
                        SButton(if (more) "Loading…" else "Show more", tag = "billing_invoicesMore", enabled = !more) {
                            more = true
                            scope.launch {
                                val r = Net.get("billing/invoices?limit=12&cursor=" + URLEncoder.encode(next, "UTF-8"))
                                if (r is ApiResult.Ok) { val (list, n) = parseInvoices(r.value); items = items + list; cursor = n }
                                more = false
                            }
                        }
                    }
                }
            }
        }
    }
}

/** A PDF in a Custom Tab over the app, whatever "Open links in" says (the invoice stays inside Allternit). */
private fun openInAppTab(context: android.content.Context, url: String) {
    runCatching { CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url)) }
}

// ---- Usage ----

/** One `windows[]` entry of `GET /api/v1/me/usage` (snake_case). */
private data class UsageWindow(val id: String, val label: String, val usedUsd: Double, val limitUsd: Double?, val usedRatio: Double?, val resetsAt: String?)

/** `pace` on `GET /api/v1/me/usage`. */
private data class UsagePace(val windowId: String, val runsOutAt: String?, val status: String)

private data class UsageReply(val windows: List<UsageWindow>, val pace: UsagePace?)

private fun parseUsage(v: JsonElement): UsageReply {
    val o = v as? JsonObject ?: return UsageReply(emptyList(), null)
    fun JsonObject.s(k: String) = (this[k]?.takeIf { it !is JsonNull } as? JsonPrimitive)?.contentOrNull
    fun JsonObject.d(k: String) = (this[k]?.takeIf { it !is JsonNull } as? JsonPrimitive)?.doubleOrNull
    val windows = (o["windows"] as? JsonArray).orEmpty().mapNotNull { e ->
        val w = e as? JsonObject ?: return@mapNotNull null
        val id = w.s("id") ?: return@mapNotNull null
        UsageWindow(id, w.s("label") ?: id, w.d("used_usd") ?: 0.0, w.d("limit_usd"), w.d("used_ratio"), w.s("resets_at"))
    }.sortedBy { when (it.id) { "5h" -> 0; "7d" -> 1; "month" -> 2; else -> 3 } }
    val pace = (o["pace"] as? JsonObject)?.let { p -> UsagePace(p.s("window_id") ?: "", p.s("runs_out_at"), p.s("status") ?: "ok") }
    return UsageReply(windows, pace)
}

private fun windowName(w: UsageWindow): String = when (w.id) {
    "5h" -> "Current session"
    "7d" -> "This week"
    "month" -> "This month"
    else -> w.label
}

private fun windowNoun(id: String): String = when (id) { "5h" -> "session"; "7d" -> "week"; else -> "month" }

/** "in 2h 12m" under a day away, "Sun 7:19 PM" within a week, else "Dec 1". */
private fun resetPhrase(raw: String?, now: Instant = Instant.now()): String? {
    val at = parseInstant(raw) ?: return null
    val left = Duration.between(now, at)
    if (left.isNegative) return "soon"
    if (left.toHours() < 24) {
        val h = left.toHours(); val m = left.toMinutes() % 60
        return if (h > 0) "in ${h}h ${m}m" else "in ${m.coerceAtLeast(1)}m"
    }
    val zoned = at.atZone(ZoneId.systemDefault())
    return if (left.toDays() < 7) zoned.format(DateTimeFormatter.ofPattern("EEE h:mm a", Locale.US))
    else zoned.format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
}

/** "today at 4:10 PM", "tomorrow", "on Sun", "on Dec 3". */
private fun runOutPhrase(at: Instant): String {
    val zone = ZoneId.systemDefault()
    val today = LocalDate.now(zone)
    val day = at.atZone(zone).toLocalDate()
    return when {
        day == today -> "today at " + at.atZone(zone).format(DateTimeFormatter.ofPattern("h:mm a", Locale.US))
        day == today.plusDays(1) -> "tomorrow"
        day.isBefore(today.plusDays(7)) -> "on " + at.atZone(zone).format(DateTimeFormatter.ofPattern("EEE", Locale.US))
        else -> "on " + at.atZone(zone).format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
    }
}

/** The one pace line, or null: shown only when the server gives `pace.runs_out_at`. */
private fun paceLine(reply: UsageReply): String? {
    val pace = reply.pace ?: return null
    val runsOut = parseInstant(pace.runsOutAt) ?: return null
    val window = reply.windows.firstOrNull { it.id == pace.windowId }
    val noun = windowNoun(pace.windowId)
    val reset = resetPhrase(window?.resetsAt)
    val resetAt = parseInstant(window?.resetsAt)
    return when {
        pace.status == "out" -> "You've reached this $noun's limit." + (reset?.let { " It resets $it." } ?: "")
        resetAt == null || runsOut.isBefore(resetAt) -> "Heads up. At this pace you'll run out ${runOutPhrase(runsOut)}" + (reset?.let { ", before the $noun resets $it." } ?: ".")
        else -> "At this pace you won't run out before the $noun resets" + (reset?.let { " $it." } ?: ".")
    }
}

@Composable
private fun UsageBar(fraction: Double) {
    val c = AllternitTheme.colors
    val fill = when {
        fraction >= 1.0 -> Danger
        fraction >= 0.75 -> Color(0xFFF59E0B)
        else -> c.textSecondary
    }
    Box(Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)).background(c.fillNeutral)) {
        Box(Modifier.fillMaxWidth(fraction.toFloat().coerceIn(0f, 1f)).height(6.dp).clip(RoundedCornerShape(3.dp)).background(fill))
    }
}

/** A flat window row: name and reset on the left, "35% used" on the right, a bar only when the window has a limit. */
@Composable
private fun WindowRow(title: String, sub: String?, right: String, fraction: Double?, tag: String) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 14.dp).testTag(tag), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(title, color = c.textPrimary, fontSize = 16.sp)
                if (sub != null) Text(sub, color = c.textSecondary, fontSize = 14.sp)
            }
            Text(right, color = c.textSecondary, fontSize = 15.sp)
        }
        if (fraction != null) UsageBar(fraction)
    }
}

/**
 * Settings → Usage (iOS UsagePageView, web frame `M1-usage-phone`): "Your usage" with the plan, the pace line (only when
 * the server says when you would run out), the 5-hour / 7-day / month rows from `windows` (a bar only where the window
 * has a limit), usage credits with Play packs where Google sells them, then Connected accounts.
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
    var reply by remember { mutableStateOf<UsageReply?>(null) }
    var failed by remember { mutableStateOf(false) }
    var nonce by remember { mutableStateOf(0) }
    LaunchedEffect(host.account, nonce) {
        if (host.account == null) return@LaunchedEffect
        failed = false
        when (val r = Net.get("me/usage")) {
            is ApiResult.Ok -> reply = parseUsage(r.value)
            else -> failed = reply == null
        }
    }
    SPage("Usage", tag = "settingsUsage", onBack = back) {
        if (host.account == null) {
            SNote("Sign in to see your plan usage.")
        } else {
            Row(Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Your usage", color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                AccountPlan.planName?.let { Text(it, color = c.textSecondary, fontSize = 15.sp) }
            }
            val r = reply
            r?.let(::paceLine)?.let {
                Text(it, color = c.textPrimary, fontSize = 18.sp, lineHeight = 25.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 8.dp).testTag("usage_pace"))
            }
            SDivider()
            when {
                r != null && r.windows.isNotEmpty() -> r.windows.forEachIndexed { index, w ->
                    if (index > 0) SDivider()
                    val right = w.usedRatio?.let { "${(it * 100).toInt()}% used" } ?: String.format(Locale.US, "$%.2f used", w.usedUsd)
                    val fraction = if (w.limitUsd != null) (w.usedRatio ?: if (w.limitUsd > 0) w.usedUsd / w.limitUsd else 0.0) else null
                    WindowRow(windowName(w), resetPhrase(w.resetsAt)?.let { "Resets $it" }, right, fraction, "usage_window_${w.id}")
                }
                // Older backend (no `windows`): the monthly allowance from `weeklyUsed` / `weeklyLimit`.
                AccountPlan.usage?.let { it.weeklyLimit > 0 && it.weeklyUsed.isFinite() } == true -> {
                    val u = AccountPlan.usage!!
                    val f = (u.weeklyUsed / u.weeklyLimit).coerceIn(0.0, 1.0)
                    WindowRow("This month", resetPhrase(u.resetsAt)?.let { "Resets $it" }, "${(f * 100).toInt()}% used", f, "usage_window_month")
                }
                failed -> {
                    SError("Couldn't load your usage.")
                    Box(Modifier.padding(horizontal = 6.dp)) { SButton("Try again", tag = "usage_retry") { nonce += 1 } }
                }
                r == null || AccountPlan.isLoading -> Row(Modifier.padding(horizontal = 6.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Loading usage…", color = c.textSecondary, fontSize = 15.sp)
                }
                else -> Column(Modifier.padding(horizontal = 6.dp, vertical = 14.dp).testTag("billing_usageEmpty")) {
                    Text("No usage to show", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text("Usage shows here once your account uses Allternit Cloud.", color = c.textSecondary, fontSize = 13.sp)
                }
            }
            // Cloud computer time keeps its own hour allowance (it does not count toward the windows).
            val fraction = AccountPlan.cloudHoursFraction
            val detail = AccountPlan.cloudHoursDetail
            if (fraction != null && detail != null) {
                SDivider()
                val next = LocalDate.now().withDayOfMonth(1).plusMonths(1).format(DateTimeFormatter.ofPattern("MMM d", Locale.US))
                WindowRow("Cloud computer", "Resets $next", detail, fraction.coerceIn(0.0, 1.0), "usage_cloudHours")
            }

            // Usage credits.
            Column(Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 28.dp, bottom = 8.dp).testTag("usage_credits"), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                val credits = AccountPlan.credits
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text("Usage credits", color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                    val amount = credits?.balanceUsd ?: AccountPlan.usage?.credits
                    if (amount != null) Text(String.format(Locale.US, "$%.2f", amount), color = c.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                }
                Text(
                    when {
                        credits != null -> String.format(Locale.US, "Available for any task. $%.2f used this month.", credits.monthToDateUsageUsd)
                        AccountPlan.usage?.credits != null -> "Available for any task."
                        AccountPlan.isLoading -> "Loading your credits…"
                        else -> "No credit balance yet."
                    },
                    color = c.textSecondary, fontSize = 14.sp,
                )
            }
            if (PlayPurchases.showsStoreButtons(StoreKind.Pack)) { SLabel("Buy credits"); StorePackGrid() }
        }
        SLabel("Connected accounts")
        ConnectedAccounts(signedIn = host.account != null, cloudWindows = reply?.windows.orEmpty())
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
 * provider out; with no card at all the section says so in one line. Kept and visible on every account.
 */
@Composable
private fun ConnectedAccounts(signedIn: Boolean, cloudWindows: List<UsageWindow>) {
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
    // The Cloud card shows the month window (the plan's grant), from `windows` when present, else the legacy fields.
    val month = cloudWindows.firstOrNull { it.id == "month" && it.limitUsd != null }
    val cloudMonth = month?.let { m -> QuotaWindow("This month", (m.usedRatio ?: 0.0).coerceIn(0.0, 1.0), m.resetsAt?.let(::shortDate)?.let { "Resets $it" }) }
        ?: AccountPlan.usage?.takeIf { u -> u.weeklyLimit > 0 && u.weeklyUsed.isFinite() }?.let { u ->
            QuotaWindow("This month", (u.weeklyUsed / u.weeklyLimit).coerceIn(0.0, 1.0), u.resetsAt?.let(::shortDate)?.let { "Resets $it" })
        }
    val all = (if (signedIn) listOf(AccountCard("Allternit Cloud", AccountPlan.planName, listOfNotNull(cloudMonth))) else emptyList()) + cards
    Column(Modifier.testTag("usage_connectedAccounts"), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (all.isEmpty()) {
            if (loading) Text("Loading connected accounts…", color = c.textSecondary, fontSize = 13.sp)
            else SNote("No connected account reports usage. Subscriptions and API keys you connect show their limits here.", art = com.allternit.core.design.EmptyArtKind.Coins)
        }
        all.forEach { card ->
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp)).padding(14.dp).testTag("usage_account_${card.provider}"),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    com.allternit.core.design.AppLogo(card.provider, id = com.allternit.core.design.AppLogoCatalog.providerKey(if (card.provider == "Allternit Cloud") "allternit" else card.provider), size = 24.dp)
                    Text(card.provider.replaceFirstChar { it.uppercase() }, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    card.plan?.let { Text(it, color = c.textSecondary, fontSize = 11.sp, modifier = Modifier.clip(RoundedCornerShape(50)).background(c.fillNeutral).padding(horizontal = 8.dp, vertical = 2.dp)) }
                }
                if (card.windows.isEmpty()) Text("This account doesn't report limits.", color = c.textTertiary, fontSize = 12.sp)
                card.windows.forEach { w ->
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Row(Modifier.fillMaxWidth()) {
                            Text(w.label, color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            Text("${(w.ratio * 100).toInt()}% used", color = c.textSecondary, fontSize = 14.sp)
                        }
                        UsageBar(w.ratio)
                        w.reset?.let { Text(it, color = c.textTertiary, fontSize = 11.sp) }
                    }
                }
            }
        }
    }
}
