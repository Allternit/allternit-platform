package com.allternit.feature.billing

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme

/** iOS CloudCreditsCopy (no web copy: the Play app has no web purchase link). */
object CloudCreditsCopy {
    const val TITLE_REQUIRED = "Allternit Cloud models need credits"
    const val TITLE_EXHAUSTED = "Your Cloud credits are used up"
    const val BODY_STORE = "Free accounts don't include Cloud usage. Credits work on all your devices. Plans and API keys you connect still work without credits."
    const val LINE_REQUIRED = "Free accounts don't include Cloud usage."
    const val LINE_EXHAUSTED = "Plans and API keys you connect still work without credits."
    const val PLAN = "Choose a plan"
    const val PLAN_NOTE = "Plus includes monthly credits."
    const val NOT_NOW = "Not now"
    const val OK = "OK"

    fun title(reason: String?): String = if (reason == "credits_exhausted") TITLE_EXHAUSTED else TITLE_REQUIRED
    fun line(reason: String?): String = if (reason == "credits_exhausted") LINE_EXHAUSTED else LINE_REQUIRED

    fun priceLine(model: String?, input: Double?, output: Double?): String? {
        if (model == null || input == null || output == null) return null
        return "$model: ${money(input)} per 1M input, ${money(output)} per 1M output"
    }

    private fun money(v: Double) = String.format(java.util.Locale.US, "$%.2f", v)

    /**
     * The reason in a relayed error, or null when it is not a credits error (iOS CloudCreditsGate.reason(inRelayedError:)).
     * The cloud answers 402 `credits_required` / `credits_exhausted`.
     */
    fun reason(text: String?): String? {
        val t = text?.lowercase()?.takeIf { it.isNotBlank() } ?: return null
        if ("credits_exhausted" in t || "credits are used up" in t) return "credits_exhausted"
        if ("credits_required" in t || "need credits" in t || "needs credits" in t || "payment required" in t || Regex("""\b402\b""").containsMatchIn(t)) {
            return "credits_required"
        }
        return null
    }
}

/** iOS CloudCreditsGate: a Cloud model the account can't run yet opens the credits sheet; null when it is open to them. */
object CloudCreditsGate {
    fun lockedRequest(modelName: String?, catalogProvider: String?, inputPer1mUsd: Double?, outputPer1mUsd: Double?): CloudCreditsRequest? {
        val e = AccountPlan.credits?.cloudModels ?: return null
        if (!e.isLocked(catalogProvider)) return null
        return CloudCreditsRequest(e.reason, modelName, inputPer1mUsd, outputPer1mUsd)
    }
}

/** What the sheet is about (iOS CloudCreditsRequest). */
data class CloudCreditsRequest(
    val reason: String?,
    val modelName: String? = null,
    val inputPer1mUsd: Double? = null,
    val outputPer1mUsd: Double? = null,
)

/**
 * iOS CloudCreditsSheet: a Cloud model needs credits. With Google on for packs it sells the four packs here and
 * offers the plans; with Google off it says why and closes (the iOS no-purchase state: no link, no price).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CloudCreditsSheet(request: CloudCreditsRequest, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var showPlans by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        PlayPurchases.start(context)
        PlayPurchases.loadCatalog()
    }
    val storePacks = PlayPurchases.showsStoreButtons(StoreKind.Pack)
    val storePlans = PlayPurchases.showsStoreButtons(StoreKind.Plan) && PlayPurchases.planPurchaseAllowed(AccountPlan.subscription)
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = storePacks), containerColor = c.bgPrimary) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(top = 8.dp, bottom = 12.dp)
                .testTag("cloudCreditsSheet"),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Box(Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)), contentAlignment = Alignment.Center) {
                Text("$", color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            }
            Text(CloudCreditsCopy.title(request.reason), color = c.textPrimary, fontSize = 20.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.semantics { heading() })
            if (storePacks || storePlans) {
                Text(CloudCreditsCopy.BODY_STORE, color = c.textSecondary, fontSize = 15.sp)
                CloudCreditsCopy.priceLine(request.modelName, request.inputPer1mUsd, request.outputPer1mUsd)?.let {
                    Text(it, color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                }
                if (storePacks) {
                    StorePackGrid()
                    StorePurchaseNoticeView()
                }
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (storePlans) {
                        OutlineButton(CloudCreditsCopy.PLAN, "cloudCreditsPlans") { showPlans = true }
                        Text(CloudCreditsCopy.PLAN_NOTE, color = c.textTertiary, fontSize = 12.sp)
                    }
                    if (storePacks) StoreRestoreButton()
                    GhostButton(CloudCreditsCopy.NOT_NOW, "cloudCreditsNotNow", onClick = onDismiss)
                }
            } else {
                Text(CloudCreditsCopy.line(request.reason), color = c.textSecondary, fontSize = 15.sp)
                PrimaryButton(CloudCreditsCopy.OK, "cloudCreditsOk", onClick = onDismiss)
            }
        }
    }
    if (showPlans) StorePurchaseSheet(StoreKind.Plan) { showPlans = false }
}
