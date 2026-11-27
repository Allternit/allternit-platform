package com.allternit.feature.billing

import com.allternit.core.api.ApiClients
import com.allternit.core.api.ApiConfig
import com.allternit.core.auth.AllternitAuth
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException

/*
 * The phone's side of the store purchase contract (cloud-api routes/billing_store.rs, docs api/cloud-billing.mdx
 * "Store purchases"), the Android port of iOS Core/Store/StoreBillingClient.swift. The server never trusts the
 * phone: it reads every purchase from the Play Developer API itself, grants credits once, and only then
 * acknowledges the subscription or consumes the pack. The app never acknowledges or consumes.
 *
 * - GET  /billing/plans, /billing/packs            `store.google.{enabled, product_id}`
 * - POST /billing/store/preflight                  `{provider: "google", kind, id, environment}` → `{account_token, product_id}` or 409/403/503
 * - POST /billing/store/google/purchases           `{kind, product_id, purchase_token}` → `{granted, reason?, subscription?, balance_usd?, idempotent_replay}`
 */

/** What a store item is on the server. */
enum class StoreKind(val wire: String) { Plan("plan"), Pack("pack") }

/**
 * One plan or credit pack Google Play can sell right now: the server has Google switched on for it
 * (`store.google.enabled`). Prices come from Play, never from here (D11).
 */
data class StoreCatalogItem(
    val kind: StoreKind,
    /** Server id: `plus`, `super` (Max), `ultra`, `credits_10` … */
    val id: String,
    val label: String,
    /** Play product id: `plan_plus`, `plan_max`, `plan_ultra`, `credits_10` … `credits_100`. */
    val productId: String,
    /** Monthly credits for a plan, the pack's credits for a pack. */
    val creditsUsd: Double,
    /** Plans only: how far unused credits roll over. */
    val rolloverCapUsd: Double? = null,
)

/** `POST /billing/store/preflight` 200. */
data class StorePreflight(val accountToken: String, val productId: String)

/** The plan as the purchase answer and `GET /billing/subscription` report it (iOS StorePlanState / BillingSubscription). */
data class BillingSubscription(
    val planId: String,
    val label: String,
    val status: String,
    val billingProvider: String? = null,
    val renewsAt: String? = null,
    val manageUrl: String? = null,
    val conflict: Boolean = false,
) {
    val isPaid: Boolean get() = planId != "free" && status in setOf("active", "trialing", "past_due", "unpaid")

    /** iOS BillingSubscription.billedByLine. */
    val billedByLine: String?
        get() = if (!isPaid) null else when (billingProvider) {
            "apple" -> "Billed by Apple. Manage it in App Store subscriptions."
            "google" -> "Billed by Google Play. Manage it in Play Store subscriptions."
            "stripe" -> "Billed on the web."
            else -> null
        }
}

/** `POST /billing/store/google/purchases` 200. Every 200 means the server recorded the purchase. */
data class StorePurchaseAnswer(
    val granted: Boolean,
    /** When nothing was granted: `sandbox_not_granted`, `refunded`, `pending`, `canceled`, `unknown_account`, `account_closed`. */
    val reason: String? = null,
    val message: String? = null,
    val subscription: BillingSubscription? = null,
    val balanceUsd: Double? = null,
    val idempotentReplay: Boolean = false,
)

/** A non-2xx answer from a store route: `{ "error": "<code>", "message": … }` plus the 409 `already_subscribed` fields. */
data class StoreRefusal(
    val status: Int,
    /** `already_subscribed`, `owned_by_another_account`, `first_purchase_limit`, `store_billing_not_configured`, `store_unavailable`, … or empty. */
    val code: String,
    val message: String? = null,
    val billingProvider: String? = null,
    val manageUrl: String? = null,
) {
    /** Worth trying again later with the same purchase (5xx, a store or network outage). 4xx answers won't change. */
    val isRetryable: Boolean get() = status >= 500 || status == 429 || status == 408
}

sealed interface StoreBillingError {
    /** 401: no Allternit session. */
    data object SignedOut : StoreBillingError
    data class Refused(val refusal: StoreRefusal) : StoreBillingError
    /** The network or the API route is down. */
    data class Unreachable(val detail: String) : StoreBillingError
    data object InvalidResponse : StoreBillingError

    val isRetryable: Boolean
        get() = when (this) {
            SignedOut -> false
            is Refused -> refusal.isRetryable
            is Unreachable, InvalidResponse -> true
        }
}

/** A call that either answers [T] or fails with a [StoreBillingError]. */
sealed interface StoreResult<out T> {
    data class Ok<T>(val value: T) : StoreResult<T>
    data class Err(val error: StoreBillingError) : StoreResult<Nothing>
}

/** `GET /api/v1/hosted-runtimes/entitlement` (camelCase): the plan name and the cloud computer hours. */
data class HostedEntitlement(val planDisplayName: String, val maxHoursMonthly: Int, val usedSecondsMonthly: Int)

/** `GET /api/v1/billing/credits`. */
data class BillingCredits(
    val balanceUsd: Double,
    val monthToDateUsageUsd: Double,
    val cloudModels: CloudModelsEntitlement? = null,
    val openModels: OpenModelsEntitlement? = null,
)

/**
 * Allternit Open: `open_models` on `/billing/credits` and `/me/usage` (CONTRACT-open-tier §3). Absent (older backend) is
 * null, and the phone then lists no open rows, as it does today. [reason] is `terms_required`, `plan_required`,
 * `allowance_used`, `bought_credits_required` or null.
 */
data class OpenModelsEntitlement(
    val allowed: Boolean,
    val reason: String?,
    val termsAccepted: Boolean,
    val plan: String?,
    val planAllowanceUsd: Double,
    val usedUsd: Double,
    val planRemainingUsd: Double,
    val boughtBalanceUsd: Double,
    val periodEnd: String?,
    val research: Boolean,
)

internal fun parseOpenModels(root: JsonObject): OpenModelsEntitlement? = (root["open_models"] as? JsonObject)?.let { o ->
    OpenModelsEntitlement(
        allowed = o.bool("allowed") ?: false,
        reason = o.str("reason"),
        termsAccepted = o.bool("terms_accepted") ?: false,
        plan = o.str("plan"),
        planAllowanceUsd = o.num("plan_allowance_usd") ?: 0.0,
        usedUsd = o.num("used_usd") ?: 0.0,
        planRemainingUsd = o.num("plan_remaining_usd") ?: 0.0,
        boughtBalanceUsd = o.num("bought_balance_usd") ?: 0.0,
        periodEnd = o.str("period_end"),
        research = o.bool("research") ?: false,
    )
}

/** iOS CloudModelsEntitlement: `cloud_models` on `/billing/credits`. Absent (older backend) means not locked. */
data class CloudModelsEntitlement(val allowed: Boolean, val reason: String?, val byokProviders: List<String>) {
    fun isLocked(provider: String?): Boolean = !allowed && (provider == null || provider !in byokProviders)
}

/** `GET /api/v1/me/usage`: the monthly Allternit Cloud allowance (iOS UsageSnapshot). */
data class UsageSnapshot(val weeklyUsed: Double, val weeklyLimit: Double, val resetsAt: String?, val credits: Double?, val openModels: OpenModelsEntitlement? = null)

/** The calls the purchase manager and the Billing page make, on the account token (same route as iOS APIClient.shared). */
object StoreBillingClient {
    private val config = ApiConfig()
    private val okHttp: OkHttpClient by lazy { ApiClients.okHttp(bearer = AllternitAuth.tokenProvider) }
    private val json = Json { ignoreUnknownKeys = true }

    /** Plans and packs with Google switched on. Empty when the server has no Google secrets yet (dark by default, D10). */
    suspend fun catalog(): StoreResult<List<StoreCatalogItem>> = coroutineScope {
        val plans = async { get("billing/plans") }
        val packs = async { get("billing/packs") }
        val p = plans.await()
        val k = packs.await()
        when {
            p is StoreResult.Err -> p
            k is StoreResult.Err -> k
            else -> StoreResult.Ok(items((p as StoreResult.Ok).value, (k as StoreResult.Ok).value))
        }
    }

    suspend fun preflight(item: StoreCatalogItem): StoreResult<StorePreflight> {
        val body = buildJsonObject {
            put("provider", "google")
            put("kind", item.kind.wire)
            put("id", item.id)
            // Play does not tell the app whether the buyer is a license tester; the server reads that from the
            // purchase itself (D8), so the app always says production.
            put("environment", "production")
        }
        return when (val r = post("billing/store/preflight", body)) {
            is StoreResult.Err -> r
            is StoreResult.Ok -> {
                val o = r.value as? JsonObject
                val token = o.str("account_token")
                val product = o.str("product_id")
                if (token.isNullOrBlank() || product.isNullOrBlank()) StoreResult.Err(StoreBillingError.InvalidResponse)
                else StoreResult.Ok(StorePreflight(token, product))
            }
        }
    }

    suspend fun postGooglePurchase(kind: StoreKind, productId: String, purchaseToken: String): StoreResult<StorePurchaseAnswer> {
        val body = buildJsonObject {
            put("kind", kind.wire)
            put("product_id", productId)
            put("purchase_token", purchaseToken)
        }
        return when (val r = post("billing/store/google/purchases", body)) {
            is StoreResult.Err -> r
            is StoreResult.Ok -> {
                val o = r.value as? JsonObject ?: return StoreResult.Err(StoreBillingError.InvalidResponse)
                StoreResult.Ok(
                    StorePurchaseAnswer(
                        granted = o.bool("granted") ?: false,
                        reason = o.str("reason"),
                        message = o.str("message"),
                        subscription = (o["subscription"] as? JsonObject)?.let(::subscription),
                        balanceUsd = o.num("balance_usd"),
                        idempotentReplay = o.bool("idempotent_replay") ?: false,
                    ),
                )
            }
        }
    }

    suspend fun subscription(): BillingSubscription? = (get("billing/subscription") as? StoreResult.Ok)?.value?.let { it as? JsonObject }?.let(::subscription)

    suspend fun credits(): BillingCredits? = ((get("billing/credits") as? StoreResult.Ok)?.value as? JsonObject)?.let {
        val cm = it["cloud_models"] as? JsonObject
        BillingCredits(
            it.num("balance_usd") ?: 0.0, it.num("month_to_date_usage_usd") ?: 0.0,
            cm?.let { o ->
                CloudModelsEntitlement(
                    allowed = o.bool("allowed") ?: true,
                    reason = o.str("reason"),
                    byokProviders = (o["byok_providers"] as? kotlinx.serialization.json.JsonArray)?.mapNotNull { (it as? kotlinx.serialization.json.JsonPrimitive)?.content } ?: emptyList(),
                )
            },
            parseOpenModels(it),
        )
    }

    suspend fun entitlement(): HostedEntitlement? = ((get("hosted-runtimes/entitlement") as? StoreResult.Ok)?.value as? JsonObject)?.let {
        HostedEntitlement(it.str("planDisplayName") ?: "", it.num("maxHoursMonthly")?.toInt() ?: 0, it.num("usedSecondsMonthly")?.toInt() ?: 0)
    }

    suspend fun usage(): UsageSnapshot? = ((get("me/usage") as? StoreResult.Ok)?.value as? JsonObject)?.let {
        UsageSnapshot(it.num("weeklyUsed") ?: 0.0, it.num("weeklyLimit") ?: 0.0, it.str("resetsAt"), it.num("credits"), parseOpenModels(it))
    }

    // Wire

    private fun subscription(o: JsonObject): BillingSubscription? {
        val planId = o.str("plan_id") ?: return null
        return BillingSubscription(
            planId = planId,
            label = o.str("label") ?: planId,
            status = o.str("status") ?: "",
            billingProvider = o.str("billing_provider"),
            renewsAt = o.str("renews_at"),
            manageUrl = o.str("manage_url"),
            conflict = o.bool("conflict") ?: false,
        )
    }

    /** Only items Google is switched on for. An old server sends no `store` block: nothing is sold in the app. */
    internal fun items(plans: JsonElement, packs: JsonElement): List<StoreCatalogItem> {
        val planItems = (plans as? JsonObject).list("plans").mapNotNull { plan ->
            val google = (plan["store"] as? JsonObject)?.get("google") as? JsonObject ?: return@mapNotNull null
            val productId = google.str("product_id")
            if (google.bool("enabled") != true || productId.isNullOrBlank()) return@mapNotNull null
            val id = plan.str("id") ?: return@mapNotNull null
            StoreCatalogItem(StoreKind.Plan, id, plan.str("label") ?: id, productId, plan.num("monthly_credits_usd") ?: 0.0, plan.num("rollover_cap_usd"))
        }
        val packItems = (packs as? JsonObject).list("packs").mapNotNull { pack ->
            val google = (pack["store"] as? JsonObject)?.get("google") as? JsonObject ?: return@mapNotNull null
            val productId = google.str("product_id")
            if (google.bool("enabled") != true || productId.isNullOrBlank()) return@mapNotNull null
            val id = pack.str("id") ?: return@mapNotNull null
            StoreCatalogItem(StoreKind.Pack, id, pack.str("label") ?: id, productId, pack.num("credits_usd") ?: 0.0)
        }
        return planItems + packItems
    }

    // Transport (keeps the error body: the shared ApiResult drops billing_provider / manage_url)

    private fun url(path: String): HttpUrl? = (config.apiBaseUrl.toString().trimEnd('/') + "/" + path.trimStart('/')).toHttpUrlOrNull()

    private suspend fun get(path: String): StoreResult<JsonElement> {
        val url = url(path) ?: return StoreResult.Err(StoreBillingError.InvalidResponse)
        return send(Request.Builder().url(url).header("Accept", "application/json").get().build())
    }

    private suspend fun post(path: String, body: JsonObject): StoreResult<JsonElement> {
        val url = url(path) ?: return StoreResult.Err(StoreBillingError.InvalidResponse)
        val request = Request.Builder().url(url).header("Accept", "application/json")
            .post(body.toString().toRequestBody("application/json".toMediaType())).build()
        return send(request)
    }

    private suspend fun send(request: Request): StoreResult<JsonElement> = withContext(Dispatchers.IO) {
        try {
            okHttp.newCall(request).execute().use { response ->
                val text = response.body.string()
                val parsed = runCatching { if (text.isBlank()) JsonNull else json.parseToJsonElement(text) }.getOrNull()
                when {
                    response.code == 401 -> StoreResult.Err(StoreBillingError.SignedOut)
                    response.isSuccessful -> if (parsed == null) StoreResult.Err(StoreBillingError.InvalidResponse) else StoreResult.Ok(parsed)
                    else -> {
                        val o = parsed as? JsonObject
                        StoreResult.Err(
                            StoreBillingError.Refused(
                                StoreRefusal(
                                    status = response.code,
                                    code = o.str("error") ?: "",
                                    message = o.str("message"),
                                    billingProvider = o.str("billing_provider"),
                                    manageUrl = o.str("manage_url"),
                                ),
                            ),
                        )
                    }
                }
            }
        } catch (e: IOException) {
            StoreResult.Err(StoreBillingError.Unreachable(e.message ?: "offline"))
        }
    }
}

internal fun JsonObject?.str(key: String): String? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.contentOrNull
internal fun JsonObject?.num(key: String): Double? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.let { it.doubleOrNull ?: it.contentOrNull?.toDoubleOrNull() }
internal fun JsonObject?.bool(key: String): Boolean? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.booleanOrNull
internal fun JsonObject?.list(key: String): List<JsonObject> = (this?.get(key) as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()
