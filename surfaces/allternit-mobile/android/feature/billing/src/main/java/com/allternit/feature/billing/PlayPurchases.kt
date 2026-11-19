package com.allternit.feature.billing

import android.app.Activity
import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.android.billingclient.api.BillingClient
import com.android.billingclient.api.BillingClientStateListener
import com.android.billingclient.api.BillingFlowParams
import com.android.billingclient.api.BillingResult
import com.android.billingclient.api.PendingPurchasesParams
import com.android.billingclient.api.ProductDetails
import com.android.billingclient.api.Purchase
import com.android.billingclient.api.PurchasesUpdatedListener
import com.android.billingclient.api.QueryProductDetailsParams
import com.android.billingclient.api.QueryPurchasesParams
import com.android.billingclient.api.queryProductDetails
import com.android.billingclient.api.queryPurchasesAsync
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlin.coroutines.resume

/** What the last purchase step left to say (iOS StorePurchaseNotice). */
sealed interface StorePurchaseNotice {
    data class Purchased(val line: String) : StorePurchaseNotice
    data class Restored(val count: Int) : StorePurchaseNotice
    data object NothingToRestore : StorePurchaseNotice
    data object Pending : StorePurchaseNotice
    data object Cancelled : StorePurchaseNotice
    data object SandboxNotGranted : StorePurchaseNotice
    data class NotGranted(val line: String) : StorePurchaseNotice
    data object SignedOut : StorePurchaseNotice
    data class AlreadySubscribed(val provider: String?, val manageUrl: String?, val message: String) : StorePurchaseNotice
    data class OwnedByAnotherAccount(val message: String) : StorePurchaseNotice
    data class FirstPurchaseLimit(val message: String) : StorePurchaseNotice
    data object StoreUnavailable : StorePurchaseNotice
    data class Failed(val message: String, val canRetry: Boolean) : StorePurchaseNotice

    val isError: Boolean
        get() = when (this) {
            is Purchased, is Restored, NothingToRestore, Pending, Cancelled, SandboxNotGranted -> false
            else -> true
        }
}

/** iOS StorePurchaseCopy, with Google Play in place of the App Store. */
object StorePurchaseCopy {
    const val SIGN_IN = "Sign in to buy"
    const val SIGNED_OUT = "Sign in to your Allternit account to buy. Purchases are added to the account you're signed in to."
    const val PENDING = "Waiting for approval. The purchase finishes when it's approved."
    const val CANCELLED = "Purchase cancelled."
    const val SANDBOX = "Test purchase. No credits were added."
    const val NOTHING_TO_RESTORE = "No purchases to restore for this Google account."
    const val STORE_UNAVAILABLE = "Google Play isn't available right now. Try again later."
    const val SERVER_ERROR = "Your purchase went through, but Allternit couldn't add it yet. We'll keep trying. You won't be charged again."
    const val PREFLIGHT_ERROR = "Couldn't start the purchase. Try again."
    const val OWNED_BY_ANOTHER = "This purchase belongs to a different Allternit account. Sign in to that account to use it."
    const val CHANGE_IN_PLAY = "Your plan was bought with another Google account. Change it in that account's Play Store subscriptions."

    fun restored(count: Int): String = if (count == 1) "Restored 1 purchase." else "Restored $count purchases."

    fun billedBy(provider: String?): String = when (provider) {
        "apple" -> "Apple"
        "google" -> "Google Play"
        else -> "the web"
    }

    fun text(notice: StorePurchaseNotice): String = when (notice) {
        is StorePurchaseNotice.Purchased -> notice.line
        is StorePurchaseNotice.Restored -> restored(notice.count)
        StorePurchaseNotice.NothingToRestore -> NOTHING_TO_RESTORE
        StorePurchaseNotice.Pending -> PENDING
        StorePurchaseNotice.Cancelled -> CANCELLED
        StorePurchaseNotice.SandboxNotGranted -> SANDBOX
        is StorePurchaseNotice.NotGranted -> notice.line
        StorePurchaseNotice.SignedOut -> SIGNED_OUT
        is StorePurchaseNotice.AlreadySubscribed -> notice.message
        is StorePurchaseNotice.OwnedByAnotherAccount -> notice.message
        is StorePurchaseNotice.FirstPurchaseLimit -> notice.message
        StorePurchaseNotice.StoreUnavailable -> STORE_UNAVAILABLE
        is StorePurchaseNotice.Failed -> notice.message
    }
}

/** The package Play bills (both the play and nativeapp flavors are com.allternit.app). */
const val PLAY_PACKAGE = "com.allternit.app"

/** Play's subscription page for one product, or all of them. */
fun playSubscriptionsUrl(productId: String? = null): String =
    "https://play.google.com/store/account/subscriptions" + (productId?.let { "?sku=$it&package=$PLAY_PACKAGE" } ?: "")

/**
 * Plans and credit packs through Google Play Billing (iOS StorePurchaseManager). Every purchase goes through the
 * server: preflight returns the account token (passed to Play as the obfuscated account id) and the product, the
 * purchase token is posted to `/billing/store/google/purchases`, and the server grants once, then acknowledges the
 * subscription or consumes the pack. The app never acknowledges or consumes: a purchase the server has not recorded
 * stays open in Play and is posted again on the next launch, sign-in or Restore.
 */
object PlayPurchases {
    enum class CatalogState { Idle, Loading, Ready, Unavailable, NotConfigured }

    var catalogState by mutableStateOf(CatalogState.Idle); private set
    var items by mutableStateOf<List<StoreCatalogItem>>(emptyList()); private set
    val products = mutableStateMapOf<String, ProductDetails>()
    var busyProductId by mutableStateOf<String?>(null); private set
    var isRestoring by mutableStateOf(false); private set
    var notice by mutableStateOf<StorePurchaseNotice?>(null)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var client: BillingClient? = null
    private var retryPurchase: Purchase? = null
    private var retryItem: StoreCatalogItem? = null
    private val inFlight = mutableSetOf<String>()
    private var catalogLoadedAt = 0L
    private var started = false

    val plans: List<StoreCatalogItem> get() = items.filter { it.kind == StoreKind.Plan }
    val packs: List<StoreCatalogItem> get() = items.filter { it.kind == StoreKind.Pack }

    /** Kinds Play can sell right now (iOS StorefrontGate.storeKinds). Empty while the server keeps Google off (D10). */
    fun showsStoreButtons(kind: StoreKind? = null): Boolean = if (kind == null) items.isNotEmpty() else items.any { it.kind == kind }

    val canRetry: Boolean get() = retryPurchase != null || retryItem != null

    private fun isSignedIn(): Boolean = AllternitAuth.state.value is AuthState.SignedIn

    /**
     * App launch: load the catalog and post every open purchase; post them again after each sign-in (iOS start()).
     * Safe to call more than once.
     */
    fun start(context: Context) {
        ensureClient(context)
        if (started) return
        started = true
        scope.launch { loadCatalog() }
        scope.launch {
            AllternitAuth.state.map { it is AuthState.SignedIn }.distinctUntilChanged().filter { it }.collect { replayOpen() }
        }
    }

    private fun ensureClient(context: Context): BillingClient {
        client?.let { return it }
        val made = BillingClient.newBuilder(context.applicationContext)
            .setListener(listener)
            .enablePendingPurchases(PendingPurchasesParams.newBuilder().enableOneTimeProducts().build())
            .build()
        client = made
        return made
    }

    /** Connects (or reconnects) to Play; false when Play is not on this phone or refused. */
    private suspend fun connected(): BillingClient? {
        val c = client ?: return null
        if (c.isReady) return c
        val ok = suspendCancellableCoroutine { cont ->
            c.startConnection(object : BillingClientStateListener {
                override fun onBillingSetupFinished(result: BillingResult) {
                    if (cont.isActive) cont.resume(result.responseCode == BillingClient.BillingResponseCode.OK)
                }

                override fun onBillingServiceDisconnected() {
                    if (cont.isActive) cont.resume(false)
                }
            })
        }
        return if (ok) c else null
    }

    // Catalog

    /** Plans and packs the server has Google on for, with Play's prices (iOS loadCatalog). */
    suspend fun loadCatalog(force: Boolean = false) {
        if (catalogState == CatalogState.Loading) return
        if (!force && catalogState == CatalogState.Ready) return
        if (!force && catalogState == CatalogState.NotConfigured && System.currentTimeMillis() - catalogLoadedAt < 600_000) return
        catalogState = CatalogState.Loading
        val enabled = when (val r = StoreBillingClient.catalog()) {
            is StoreResult.Err -> return publish(emptyList(), emptyMap(), CatalogState.Unavailable)
            is StoreResult.Ok -> r.value
        }
        if (enabled.isEmpty()) return publish(emptyList(), emptyMap(), CatalogState.NotConfigured)
        val c = connected() ?: return publish(emptyList(), emptyMap(), CatalogState.Unavailable)
        val found = coroutineScope {
            val subs = async { details(c, enabled.filter { it.kind == StoreKind.Plan }.map { it.productId }, BillingClient.ProductType.SUBS) }
            val packs = async { details(c, enabled.filter { it.kind == StoreKind.Pack }.map { it.productId }, BillingClient.ProductType.INAPP) }
            subs.await() + packs.await()
        }
        val byId = found.associateBy { it.productId }
        val sellable = enabled.filter { byId[it.productId] != null }
        publish(sellable, byId, if (sellable.isEmpty()) CatalogState.Unavailable else CatalogState.Ready)
        if (isSignedIn()) replayOpen()
    }

    /** Try again after Play or the server was unreachable. */
    fun reloadCatalog() {
        scope.launch { loadCatalog(force = true) }
    }

    private suspend fun details(c: BillingClient, ids: List<String>, type: String): List<ProductDetails> {
        if (ids.isEmpty()) return emptyList()
        val params = QueryProductDetailsParams.newBuilder()
            .setProductList(ids.map { QueryProductDetailsParams.Product.newBuilder().setProductId(it).setProductType(type).build() })
            .build()
        val result = c.queryProductDetails(params)
        return if (result.billingResult.responseCode == BillingClient.BillingResponseCode.OK) result.productDetailsList.orEmpty() else emptyList()
    }

    private fun publish(items: List<StoreCatalogItem>, products: Map<String, ProductDetails>, state: CatalogState) {
        catalogLoadedAt = System.currentTimeMillis()
        this.items = items
        this.products.clear()
        this.products.putAll(products)
        catalogState = state
    }

    /** The base plan's offer (no offer id), else the first one. */
    private fun baseOffer(details: ProductDetails): ProductDetails.SubscriptionOfferDetails? =
        details.subscriptionOfferDetails?.let { offers -> offers.firstOrNull { it.offerId == null } ?: offers.firstOrNull() }

    /** Play's localized price: the monthly recurring price for a plan, the one-time price for a pack. */
    fun displayPrice(item: StoreCatalogItem): String? {
        val details = products[item.productId] ?: return null
        return if (item.kind == StoreKind.Plan) baseOffer(details)?.pricingPhases?.pricingPhaseList?.lastOrNull()?.formattedPrice
        else details.oneTimePurchaseOfferDetails?.formattedPrice
    }

    fun displayName(item: StoreCatalogItem): String = products[item.productId]?.name?.takeIf { it.isNotBlank() } ?: "Allternit ${item.label}"

    /** Plans can be bought while no plan is billed, or the open plan is billed by Google (changed inside Play) (D5). */
    fun planPurchaseAllowed(subscription: BillingSubscription?): Boolean = subscription == null || !subscription.isPaid || subscription.billingProvider == "google"

    // Purchase

    /** Preflight, then Play's purchase sheet. The answer arrives in [listener]. */
    fun purchase(activity: Activity, item: StoreCatalogItem) {
        if (busyProductId != null) return
        if (!isSignedIn()) { notice = StorePurchaseNotice.SignedOut; return }
        if (products[item.productId] == null) { notice = StorePurchaseNotice.StoreUnavailable; return }
        busyProductId = item.productId
        notice = null
        retryItem = null
        retryPurchase = null
        scope.launch {
            val preflight = when (val r = StoreBillingClient.preflight(item)) {
                is StoreResult.Err -> {
                    notice = refusalNotice(r.error, preflight = true)
                    if (r.error.isRetryable) retryItem = item
                    busyProductId = null
                    return@launch
                }
                is StoreResult.Ok -> r.value
            }
            val c = connected()
            val details = products[preflight.productId] ?: products[item.productId]
            if (c == null || details == null) {
                notice = StorePurchaseNotice.StoreUnavailable
                busyProductId = null
                return@launch
            }
            val productParams = BillingFlowParams.ProductDetailsParams.newBuilder().setProductDetails(details)
            if (item.kind == StoreKind.Plan) {
                val offer = baseOffer(details)
                if (offer == null) {
                    notice = StorePurchaseNotice.StoreUnavailable
                    busyProductId = null
                    return@launch
                }
                productParams.setOfferToken(offer.offerToken)
            }
            val flow = BillingFlowParams.newBuilder()
                .setProductDetailsParamsList(listOf(productParams.build()))
                .setObfuscatedAccountId(preflight.accountToken)
            if (item.kind == StoreKind.Plan) {
                // A plan change inside Play replaces the open subscription, so it bills once (D5).
                val owned = ownedPurchases(c, BillingClient.ProductType.SUBS).firstOrNull { p ->
                    p.purchaseState == Purchase.PurchaseState.PURCHASED && preflight.productId !in p.products
                }
                if (owned != null) {
                    flow.setSubscriptionUpdateParams(
                        BillingFlowParams.SubscriptionUpdateParams.newBuilder()
                            .setOldPurchaseToken(owned.purchaseToken)
                            .setSubscriptionReplacementMode(BillingFlowParams.SubscriptionUpdateParams.ReplacementMode.WITH_TIME_PRORATION)
                            .build(),
                    )
                } else if (AccountPlan.subscription?.let { it.isPaid && it.billingProvider == "google" && it.planId != item.id } == true) {
                    notice = StorePurchaseNotice.AlreadySubscribed("google", playSubscriptionsUrl(), StorePurchaseCopy.CHANGE_IN_PLAY)
                    busyProductId = null
                    return@launch
                }
            }
            val result = c.launchBillingFlow(activity, flow.build())
            if (result.responseCode != BillingClient.BillingResponseCode.OK) {
                notice = flowNotice(result.responseCode)
                busyProductId = null
            }
        }
    }

    private val listener = PurchasesUpdatedListener { result, purchases ->
        scope.launch {
            when (result.responseCode) {
                BillingClient.BillingResponseCode.OK -> {
                    for (purchase in purchases.orEmpty()) handleUpdate(purchase)
                }
                BillingClient.BillingResponseCode.ITEM_ALREADY_OWNED -> restore()
                else -> notice = flowNotice(result.responseCode)
            }
            busyProductId = null
        }
    }

    private suspend fun handleUpdate(purchase: Purchase) {
        if (purchase.purchaseState == Purchase.PurchaseState.PENDING) {
            notice = StorePurchaseNotice.Pending
            return
        }
        if (purchase.purchaseState != Purchase.PurchaseState.PURCHASED) return
        if (!isSignedIn()) { notice = StorePurchaseNotice.SignedOut; return }
        when (val d = deliver(purchase)) {
            is Delivery.Delivered -> notice = answerNotice(d.answer, purchase.products.firstOrNull())
            is Delivery.Refused -> {
                notice = refusalNotice(d.error, preflight = false)
                if (d.error.isRetryable) retryPurchase = purchase
            }
            Delivery.Busy -> Unit
        }
    }

    private fun flowNotice(code: Int): StorePurchaseNotice = when (code) {
        BillingClient.BillingResponseCode.USER_CANCELED -> StorePurchaseNotice.Cancelled
        BillingClient.BillingResponseCode.ITEM_UNAVAILABLE -> StorePurchaseNotice.Failed("This item isn't for sale in Google Play right now.", canRetry = false)
        BillingClient.BillingResponseCode.SERVICE_UNAVAILABLE,
        BillingClient.BillingResponseCode.SERVICE_DISCONNECTED,
        BillingClient.BillingResponseCode.BILLING_UNAVAILABLE,
        BillingClient.BillingResponseCode.NETWORK_ERROR -> StorePurchaseNotice.StoreUnavailable
        else -> StorePurchaseNotice.Failed(StorePurchaseCopy.PREFLIGHT_ERROR, canRetry = false)
    }

    /** Try again: the same purchase token when the server could not record it, else the purchase from the top. */
    fun retry(activity: Activity) {
        val purchase = retryPurchase
        if (purchase != null) {
            retryPurchase = null
            notice = null
            scope.launch { handleUpdate(purchase) }
        } else {
            retryItem?.let { purchase(activity, it) }
        }
    }

    // Restore and replay

    private suspend fun ownedPurchases(c: BillingClient, type: String): List<Purchase> {
        val result = c.queryPurchasesAsync(QueryPurchasesParams.newBuilder().setProductType(type).build())
        return if (result.billingResult.responseCode == BillingClient.BillingResponseCode.OK) result.purchasesList else emptyList()
    }

    /** Posts purchases the server may not have recorded: unacknowledged plans and every unconsumed pack (iOS replayUnfinished). */
    suspend fun replayOpen() {
        if (!isSignedIn()) return
        val c = connected() ?: return
        val open = ownedPurchases(c, BillingClient.ProductType.SUBS).filter { !it.isAcknowledged } + ownedPurchases(c, BillingClient.ProductType.INAPP)
        for (purchase in open) {
            if (purchase.purchaseState == Purchase.PurchaseState.PURCHASED) deliver(purchase)
        }
    }

    /** Restore Purchases: posts every plan and open pack this Google account owns (iOS restore()). */
    fun restore() {
        if (isRestoring) return
        if (!isSignedIn()) { notice = StorePurchaseNotice.SignedOut; return }
        isRestoring = true
        notice = null
        scope.launch {
            try {
                val c = connected()
                if (c == null) { notice = StorePurchaseNotice.StoreUnavailable; return@launch }
                var restored = 0
                var failure: StorePurchaseNotice? = null
                var pending = false
                val owned = ownedPurchases(c, BillingClient.ProductType.SUBS) + ownedPurchases(c, BillingClient.ProductType.INAPP)
                for (purchase in owned) {
                    if (purchase.purchaseState == Purchase.PurchaseState.PENDING) { pending = true; continue }
                    if (purchase.purchaseState != Purchase.PurchaseState.PURCHASED) continue
                    when (val d = deliver(purchase)) {
                        is Delivery.Delivered -> when {
                            d.answer.granted -> restored += 1
                            d.answer.reason == "sandbox_not_granted" -> failure = failure ?: StorePurchaseNotice.SandboxNotGranted
                        }
                        is Delivery.Refused -> {
                            failure = refusalNotice(d.error, preflight = false)
                            if (d.error.isRetryable) retryPurchase = purchase
                        }
                        Delivery.Busy -> Unit
                    }
                }
                notice = when {
                    restored > 0 -> StorePurchaseNotice.Restored(restored)
                    failure != null -> failure
                    pending -> StorePurchaseNotice.Pending
                    else -> StorePurchaseNotice.NothingToRestore
                }
            } finally {
                isRestoring = false
            }
        }
    }

    sealed interface Delivery {
        data class Delivered(val answer: StorePurchaseAnswer) : Delivery
        data class Refused(val error: StoreBillingError) : Delivery
        data object Busy : Delivery
    }

    /** Posts one purchase token. The server acknowledges or consumes it once it is recorded; the app never does. */
    private suspend fun deliver(purchase: Purchase): Delivery {
        val token = purchase.purchaseToken
        if (!inFlight.add(token)) return Delivery.Busy
        try {
            val productId = purchase.products.firstOrNull() ?: return Delivery.Refused(StoreBillingError.InvalidResponse)
            val kind = items.firstOrNull { it.productId == productId }?.kind
                ?: if (productId.startsWith("plan_")) StoreKind.Plan else StoreKind.Pack
            return when (val r = StoreBillingClient.postGooglePurchase(kind, productId, token)) {
                is StoreResult.Ok -> {
                    if (r.value.granted || r.value.reason == "refunded") AccountPlan.refresh()
                    Delivery.Delivered(r.value)
                }
                is StoreResult.Err -> Delivery.Refused(r.error)
            }
        } finally {
            inFlight.remove(token)
        }
    }

    // Notices

    private fun successNotice(answer: StorePurchaseAnswer, productId: String?): StorePurchaseNotice {
        if (answer.idempotentReplay) return StorePurchaseNotice.Purchased("This purchase is already on your account.")
        answer.subscription?.let { return StorePurchaseNotice.Purchased("${it.label} is active. Its credits are on your account.") }
        val label = items.firstOrNull { it.productId == productId }?.label ?: "Credits"
        return StorePurchaseNotice.Purchased("$label in credits added to your account.")
    }

    private fun answerNotice(answer: StorePurchaseAnswer, productId: String?): StorePurchaseNotice {
        if (answer.granted) return successNotice(answer, productId)
        return when (answer.reason) {
            "sandbox_not_granted" -> StorePurchaseNotice.SandboxNotGranted
            "refunded" -> StorePurchaseNotice.NotGranted("This purchase was refunded. Nothing was added.")
            "pending" -> StorePurchaseNotice.Pending
            "canceled" -> StorePurchaseNotice.NotGranted("This purchase was cancelled. Nothing was added.")
            else -> StorePurchaseNotice.NotGranted(answer.message ?: "Nothing was added to your account.")
        }
    }

    private fun refusalNotice(error: StoreBillingError, preflight: Boolean): StorePurchaseNotice = when (error) {
        StoreBillingError.SignedOut -> StorePurchaseNotice.SignedOut
        is StoreBillingError.Refused -> {
            val refusal = error.refusal
            when (refusal.code) {
                "already_subscribed" -> StorePurchaseNotice.AlreadySubscribed(
                    refusal.billingProvider, refusal.manageUrl,
                    refusal.message ?: "This account already has a plan billed by ${StorePurchaseCopy.billedBy(refusal.billingProvider)}.",
                )
                "owned_by_another_account" -> StorePurchaseNotice.OwnedByAnotherAccount(refusal.message ?: StorePurchaseCopy.OWNED_BY_ANOTHER)
                "first_purchase_limit" -> StorePurchaseNotice.FirstPurchaseLimit(refusal.message ?: "This purchase is over the limit for a first purchase.")
                "store_billing_not_configured" -> {
                    publish(emptyList(), emptyMap(), CatalogState.NotConfigured)
                    StorePurchaseNotice.Failed(refusal.message ?: "Buying in the app isn't available yet.", canRetry = false)
                }
                else -> StorePurchaseNotice.Failed(
                    if (preflight) refusal.message ?: StorePurchaseCopy.PREFLIGHT_ERROR else StorePurchaseCopy.SERVER_ERROR,
                    canRetry = refusal.isRetryable,
                )
            }
        }
        is StoreBillingError.Unreachable, StoreBillingError.InvalidResponse ->
            StorePurchaseNotice.Failed(if (preflight) StorePurchaseCopy.PREFLIGHT_ERROR else StorePurchaseCopy.SERVER_ERROR, canRetry = true)
    }
}

/** The account's plan, credits, cloud computer hours and Cloud allowance (iOS AccountPlanStore + UsageStore). */
object AccountPlan {
    var subscription by mutableStateOf<BillingSubscription?>(null); private set
    var credits by mutableStateOf<BillingCredits?>(null); private set
    var entitlement by mutableStateOf<HostedEntitlement?>(null); private set
    var usage by mutableStateOf<UsageSnapshot?>(null); private set
    var isLoading by mutableStateOf(false); private set

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    fun refresh() {
        scope.launch { load() }
    }

    suspend fun load() {
        if (AllternitAuth.state.value !is AuthState.SignedIn) return
        isLoading = true
        try {
            coroutineScope {
                val s = async { StoreBillingClient.subscription() }
                val c = async { StoreBillingClient.credits() }
                val e = async { StoreBillingClient.entitlement() }
                val u = async { StoreBillingClient.usage() }
                subscription = s.await()
                credits = c.await()
                entitlement = e.await()
                usage = u.await()
            }
        } finally {
            isLoading = false
        }
    }

    /** The subscription's label, else the entitlement's plan name. */
    val planName: String?
        get() {
            val s = subscription
            if (s != null && s.planId != "free") return s.label
            return entitlement?.planDisplayName?.takeIf { it.isNotBlank() } ?: s?.label
        }

    val cloudHoursFraction: Double?
        get() = entitlement?.takeIf { it.maxHoursMonthly > 0 }?.let { it.usedSecondsMonthly / 3600.0 / it.maxHoursMonthly }

    val cloudHoursDetail: String?
        get() = entitlement?.takeIf { it.maxHoursMonthly > 0 }?.let { String.format(java.util.Locale.US, "%.1f of %d h", it.usedSecondsMonthly / 3600.0, it.maxHoursMonthly) }
}
