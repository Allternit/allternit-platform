package com.allternit.feature.billing

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.allternit.core.design.AllternitTheme

/*
 * The store purchase pieces (iOS Features/Settings/StorePurchaseViews.swift): the notice, the "Billed by" line,
 * plan tickets, the pack grid, Restore Purchases, the subscription terms and the purchase sheet. Google Play in place
 * of the App Store. Google Play policy allows no web purchase links in a Play-distributed app, so there is no
 * "Buy on allternit.com" here: with Google off, screens show plan status only, the way iOS does outside the US.
 */

internal val StoreWarn = Color(0xFFB7791F)

object StoreViewCopy {
    const val SHEET_TITLE_PLANS = "Choose a plan"
    const val SHEET_TITLE_PACKS = "Buy credits"
    const val SHEET_BODY = "Plans and credits work on all your devices. Credits are added to the account you're signed in to."
    const val PACKS_HEADER = "Credit packs"
    const val RESTORE = "Restore Purchases"
    const val RESTORING = "Restoring…"
    const val MANAGE_GOOGLE = "Manage in Play Store"
    const val LOADING = "Loading prices…"
    const val TRY_AGAIN = "Try again"
    const val SUBSCRIBE = "Subscribe"
    const val SWITCH_PLAN = "Switch"
    const val BUYING = "Buying…"
    const val CONFLICT = "You have two plans. Cancel one."
    const val DONE = "Done"
    const val TERMS_TAIL = "Payment is charged to your Google Play account at confirmation of purchase. A subscription renews automatically each month until you cancel it. Manage or cancel a subscription in the Play Store under Payments & subscriptions > Subscriptions. Cancelling stops the next renewal; you keep the plan until the period ends. Credit packs are one-time purchases. Refunds are handled by Google Play."
    const val TERMS_URL = "https://allternit.com/terms"
    const val PRIVACY_URL = "https://allternit.com/privacy"

    fun renewalLine(price: String): String = "Renews automatically every month at $price until you cancel. Cancel any time in the Play Store under Subscriptions."

    fun pricesSentence(pairs: List<Pair<String, String>>): String? {
        if (pairs.isEmpty()) return null
        val parts = pairs.map { "${it.first} is ${it.second} per month" }
        val joined = when (parts.size) {
            1 -> parts[0]
            2 -> "${parts[0]} and ${parts[1]}"
            else -> parts.dropLast(1).joinToString(", ") + ", and " + parts.last()
        }
        return "$joined."
    }
}

fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

fun openStoreLink(context: Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

private fun signedIn(): Boolean = AllternitAuth.state.value is AuthState.SignedIn

@Composable
private fun InlineButton(title: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 36.dp).clip(RoundedCornerShape(9.dp)).background(c.bgPrimary)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(9.dp)).testTag(tag).clickable(onClick = onClick).padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
internal fun PrimaryButton(title: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.fillMaxWidth().heightIn(min = 46.dp).clip(RoundedCornerShape(12.dp)).background(if (enabled) c.textPrimary else c.borderStrong)
            .testTag(tag).clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = c.bgPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
internal fun OutlineButton(title: String, tag: String, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(10.dp)).background(c.bgPrimary)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(10.dp)).testTag(tag).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { Text(title, color = c.textPrimary, fontSize = 13.5.sp, fontWeight = FontWeight.SemiBold) }
}

@Composable
internal fun GhostButton(title: String, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        Modifier.heightIn(min = 44.dp).testTag(tag).clickable(enabled = enabled, onClick = onClick).padding(horizontal = 4.dp),
        contentAlignment = Alignment.CenterStart,
    ) { Text(title, color = if (enabled) c.textPrimary else c.textSecondary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
}

/** iOS StorePurchaseNoticeView: what the last purchase step left to say, with its one action. */
@Composable
fun StorePurchaseNoticeView() {
    val notice = PlayPurchases.notice ?: return
    val c = AllternitTheme.colors
    val context = LocalContext.current
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp))
            .padding(12.dp).testTag("storeNotice"),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(if (notice.isError) "!" else "✓", color = if (notice.isError) StoreWarn else c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Bold)
            Text(StorePurchaseCopy.text(notice), color = c.textPrimary, fontSize = 13.sp, modifier = Modifier.weight(1f))
        }
        when {
            notice is StorePurchaseNotice.Failed && notice.canRetry && PlayPurchases.canRetry ->
                InlineButton(StoreViewCopy.TRY_AGAIN, "storeNoticeRetry") { context.findActivity()?.let { PlayPurchases.retry(it) } }
            notice is StorePurchaseNotice.StoreUnavailable ->
                InlineButton(StoreViewCopy.TRY_AGAIN, "storeNoticeReload") { PlayPurchases.notice = null; PlayPurchases.reloadCatalog() }
            notice is StorePurchaseNotice.AlreadySubscribed -> {
                val url = if (notice.provider == "google") (notice.manageUrl ?: playSubscriptionsUrl()) else notice.manageUrl?.takeIf { notice.provider != "stripe" }
                if (url != null) InlineButton(if (notice.provider == "google") StoreViewCopy.MANAGE_GOOGLE else "Manage on ${StorePurchaseCopy.billedBy(notice.provider)}", "storeNoticeManage") { openStoreLink(context, url) }
            }
        }
    }
}

/**
 * iOS StoreBilledByView: "Billed by Google Play" with Manage in Play Store, or the store that bills the plan. A
 * web (Stripe) plan shows the line with no link: the Play app does not link out to web billing.
 */
@Composable
fun StoreBilledByView(subscription: BillingSubscription?) {
    val s = subscription ?: return
    val line = s.billedByLine ?: return
    val c = AllternitTheme.colors
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag("storeBilledBy")) {
        if (s.conflict) Text(StoreViewCopy.CONFLICT, color = StoreWarn, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
        Text(line, color = c.textSecondary, fontSize = 13.sp)
        if (s.billingProvider == "google") {
            val productId = PlayPurchases.items.firstOrNull { it.kind == StoreKind.Plan && it.id == s.planId }?.productId
            GhostButton(StoreViewCopy.MANAGE_GOOGLE, "storeManageGoogle") { openStoreLink(context, playSubscriptionsUrl(productId)) }
        }
    }
}

/** iOS PlanTicketView + StorePlanTicket: a plan with its credits, Play's price and Subscribe / Switch. */
@Composable
fun StorePlanTicket(item: StoreCatalogItem) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val sub = AccountPlan.subscription
    val current = sub?.isPaid == true && sub.planId == item.id
    val price = PlayPurchases.displayPrice(item)
    val busy = PlayPurchases.busyProductId == item.productId
    val disabled = PlayPurchases.busyProductId != null && !busy
    val action = when {
        !signedIn() -> StorePurchaseCopy.SIGN_IN
        busy -> StoreViewCopy.BUYING
        sub?.isPaid == true -> StoreViewCopy.SWITCH_PLAN
        else -> StoreViewCopy.SUBSCRIBE
    }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Column(
            Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPrimary)
                .border(if (current) 2.dp else 1.dp, if (current) c.textPrimary else c.borderSubtle, RoundedCornerShape(14.dp))
                .padding(14.dp).testTag("storePlan_${item.id}"),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(item.label, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                if (!price.isNullOrBlank()) Text(price, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            }
            Text(String.format(java.util.Locale.US, "$%.0f in Allternit Cloud credits each month", item.creditsUsd), color = c.textSecondary, fontSize = 13.sp)
            item.rolloverCapUsd?.let { Text(String.format(java.util.Locale.US, "Unused credits roll over, up to $%.0f", it), color = c.textSecondary, fontSize = 13.sp) }
            if (current) {
                Text("Current plan", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            } else {
                Box(
                    Modifier.fillMaxWidth().heightIn(min = 40.dp).clip(RoundedCornerShape(10.dp)).background(if (disabled) c.borderStrong else c.textPrimary)
                        .testTag("storePlanBuy_${item.id}")
                        .clickable(enabled = !disabled && !busy) {
                            if (signedIn()) context.findActivity()?.let { PlayPurchases.purchase(it, item) } else PlayPurchases.notice = StorePurchaseNotice.SignedOut
                        },
                    contentAlignment = Alignment.Center,
                ) { Text(action, color = c.bgPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
            }
        }
        if (!current && !price.isNullOrBlank()) Text(StoreViewCopy.renewalLine(price), color = c.textTertiary, fontSize = 11.sp)
    }
}

/** iOS StorePackGrid / StorePackButton: the four packs with Play's prices. */
@Composable
fun StorePackGrid() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        for (item in PlayPurchases.packs) {
            val busy = PlayPurchases.busyProductId == item.productId
            val price = PlayPurchases.displayPrice(item) ?: ""
            Column(
                Modifier.weight(1f).heightIn(min = 52.dp).clip(RoundedCornerShape(10.dp)).background(c.bgPrimary)
                    .border(1.dp, c.borderSubtle, RoundedCornerShape(10.dp))
                    .semantics { contentDescription = if (busy) "Buying ${item.label} credits" else "${item.label} credits, $price" }
                    .testTag("storePack_${item.id}")
                    .clickable(enabled = PlayPurchases.busyProductId == null) {
                        if (signedIn()) context.findActivity()?.let { PlayPurchases.purchase(it, item) } else PlayPurchases.notice = StorePurchaseNotice.SignedOut
                    }
                    .padding(vertical = 8.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
                verticalArrangement = Arrangement.Center,
            ) {
                if (busy) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                else Text(item.label, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                Text(price, color = c.textSecondary, fontSize = 11.5.sp)
            }
        }
    }
}

/** iOS StoreRestoreButton. */
@Composable
fun StoreRestoreButton() {
    GhostButton(if (PlayPurchases.isRestoring) StoreViewCopy.RESTORING else StoreViewCopy.RESTORE, "storeRestore", enabled = !PlayPurchases.isRestoring) {
        PlayPurchases.restore()
    }
}

/** iOS StoreTermsView: Play's prices for each plan, the renewal terms, Terms of Use and Privacy Policy. */
@Composable
fun StoreTermsView() {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val prices = StoreViewCopy.pricesSentence(PlayPurchases.plans.mapNotNull { item -> PlayPurchases.displayPrice(item)?.let { PlayPurchases.displayName(item) to it } })
    Column(verticalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.testTag("storeTerms")) {
        Text(listOfNotNull(prices, StoreViewCopy.TERMS_TAIL).joinToString(" "), color = c.textTertiary, fontSize = 11.sp)
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            GhostButton("Terms of Use", "storeTermsLink") { openStoreLink(context, StoreViewCopy.TERMS_URL) }
            GhostButton("Privacy Policy", "storePrivacyLink") { openStoreLink(context, StoreViewCopy.PRIVACY_URL) }
        }
    }
}

/**
 * iOS StorePurchaseSheet: plans and packs through Google Play (focus picks which comes first), Restore Purchases and
 * the terms. Opened from Cloud credits, the compute gate and anywhere iOS opens it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun StorePurchaseSheet(focus: StoreKind = StoreKind.Plan, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    LaunchedEffect(Unit) {
        PlayPurchases.start(context)
        PlayPurchases.notice = null
        PlayPurchases.loadCatalog()
        AccountPlan.load()
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = c.bgPrimary) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 16.dp)
                .testTag("storePurchaseSheet"),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    if (focus == StoreKind.Pack) StoreViewCopy.SHEET_TITLE_PACKS else StoreViewCopy.SHEET_TITLE_PLANS,
                    color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f).semantics { heading() },
                )
                GhostButton(StoreViewCopy.DONE, "storeSheetDone", onClick = onDismiss)
            }
            Text(StoreViewCopy.SHEET_BODY, color = c.textSecondary, fontSize = 15.sp)
            if (!signedIn()) Text(StorePurchaseCopy.SIGNED_OUT, color = c.textPrimary, fontSize = 13.sp)
            StorePurchaseNoticeView()
            when (PlayPurchases.catalogState) {
                PlayPurchases.CatalogState.Idle, PlayPurchases.CatalogState.Loading -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text(StoreViewCopy.LOADING, color = c.textSecondary, fontSize = 15.sp)
                }
                PlayPurchases.CatalogState.Unavailable, PlayPurchases.CatalogState.NotConfigured -> if (PlayPurchases.notice == null) {
                    Text(StorePurchaseCopy.STORE_UNAVAILABLE, color = c.textPrimary, fontSize = 15.sp)
                    InlineButton(StoreViewCopy.TRY_AGAIN, "storeSheetReload") { PlayPurchases.reloadCatalog() }
                }
                PlayPurchases.CatalogState.Ready -> {
                    if (focus == StoreKind.Pack) { SheetPacks(); SheetPlans() } else { SheetPlans(); SheetPacks() }
                    StoreBilledByView(AccountPlan.subscription)
                    StoreRestoreButton()
                    StoreTermsView()
                }
            }
            Spacer(Modifier.size(4.dp))
        }
    }
}

@Composable
private fun SheetPlans() {
    if (!PlayPurchases.planPurchaseAllowed(AccountPlan.subscription) || PlayPurchases.plans.isEmpty()) return
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) { for (item in PlayPurchases.plans) StorePlanTicket(item) }
}

@Composable
private fun SheetPacks() {
    if (PlayPurchases.packs.isEmpty()) return
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(StoreViewCopy.PACKS_HEADER, color = AllternitTheme.colors.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
        StorePackGrid()
    }
}
