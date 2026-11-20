package com.allternit.feature.phones

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.bots.BotRecord
import com.allternit.feature.bots.BotsApi
import com.allternit.feature.bots.BotsResult
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import com.allternit.core.design.R as DesignR

/*
 * Connect (iOS Features/Connect): "Give a bot a phone" (GiveBotPhoneSheet), the connect wizard's flows and
 * VendorConnectView. Every call is `/api/v1/...` on allternit-api (iOS ConnectHTTP). Nothing here buys a
 * number, sends a message or creates a paid resource without the person tapping the button that says so.
 */

/** iOS ChannelProvider. */
private enum class ChannelProvider(val label: String, val blurb: String) {
    Sms("Phone number", "Call or text your bots on a real phone number. Calls work instantly; texting follows carrier approval."),
    Email("Email", "Anyone can email your bots. You approve replies, or automate them for known senders."),
    Telegram("Telegram", "Chat with your bots in Telegram. Allternit creates the bot, you tap Confirm once."),
    Discord("Discord", "Your bots answer in a Discord server through the shared Allternit app."),
    Slack("Slack", "Your bots answer in Slack through the shared Allternit app."),
    Teams("Microsoft Teams", "Your bots answer in Microsoft Teams chats through the shared Allternit app."),
    Whatsapp("WhatsApp", "Your bots answer on a WhatsApp Business number, connected through Meta."),
}

private suspend fun call(method: String, path: String, body: JsonElement? = null, doing: String): Pair<JsonElement?, String?> {
    val r = if (method == "GET") LibraryNet.get(path) else LibraryNet.send(method, path, body)
    return when (r) {
        is ApiResult.Ok -> r.value to null
        is ApiResult.HttpError -> null to (if (r.status == 404) "unavailable" else r.friendly(doing))
        else -> null to r.friendly(doing)
    }
}

/** iOS GiveBotPhoneSheet: pick the bot, then how people reach it. */
@Composable
fun GiveBotPhoneSheet(host: PhonesHost, botId: String?, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var bots by remember { mutableStateOf<List<BotRecord>?>(null) }
    var picked by remember { mutableStateOf(botId) }
    var flow by remember { mutableStateOf<ChannelProvider?>(null) }
    LaunchedEffect(Unit) {
        bots = (BotsApi.listAgents() as? BotsResult.Ok)?.value.orEmpty()
        if (picked == null) picked = bots?.firstOrNull()?.id
    }
    LibrarySheet("Give a bot a phone", "giveBotPhoneSheet", onDismiss, leading = LibrarySheetAction("Close", testTag = "giveBotPhoneClose", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (!host.signedIn) {
                FriendlyState(FriendlyStyle.Empty, "Sign in to connect", "Numbers, email addresses and channels belong to your Allternit account.", Modifier, "Sign in", { onDismiss(); host.openSignIn() }) { Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary) }
                return@Column
            }
            val list = bots
            if (botId == null) {
                Text("Which bot?", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                when {
                    list == null -> CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    list.isEmpty() -> Text("You don’t have a bot yet. Make one in Bots first.", color = c.textSecondary, fontSize = 13.sp)
                    else -> list.forEach { b ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(role = Role.RadioButton) { picked = b.id }.testTag("giveBotPick${b.id}"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            BotAvatar(b, 28.dp)
                            Text(b.name, color = c.textPrimary, fontSize = 16.sp, modifier = Modifier.weight(1f))
                            if (picked == b.id) Icon(painterResource(DesignR.drawable.ph_check), null, tint = c.accentPrimary, modifier = Modifier.size(18.dp))
                        }
                    }
                }
            }
            Text("How people reach ${list?.firstOrNull { it.id == picked }?.name ?: "your bot"}", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
            ChannelProvider.entries.forEach { p ->
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(enabled = !list.isNullOrEmpty(), role = Role.Button) { flow = p }.testTag("giveBotProvider${p.name}").padding(vertical = 6.dp), verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Box(Modifier.size(32.dp).clip(CircleShape).background(c.bgSecondary))
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                        Text(p.label, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        Text(p.blurb, color = c.textSecondary, fontSize = 12.sp)
                    }
                    Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                }
            }
        }
    }
    flow?.let { p -> ConnectWizardSheet(p, picked, onClose = { flow = null }) }
}

/** iOS ConnectWizardView: one provider's flow in a sheet. */
@Composable
private fun ConnectWizardSheet(provider: ChannelProvider, botId: String?, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    LibrarySheet(provider.label, "connectWizard${provider.name}", onClose, leading = LibrarySheetAction("Close", testTag = "connectWizardClose", onClick = onClose)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(provider.blurb, color = c.textSecondary, fontSize = 14.sp)
            when {
                botId == null -> Text("Pick a bot first.", color = c.textSecondary, fontSize = 14.sp)
                provider == ChannelProvider.Email -> EmailFlow(botId, onClose)
                else -> FriendlyState(FriendlyStyle.Empty, "Finish this one on your computer", "${provider.label} needs a sign-in or a number purchase, and those steps run in Allternit Desktop or ai.allternit.com for now. Everything you set up there shows here.", Modifier) { Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary) }
            }
        }
    }
}

private val replyModes = listOf("ask" to "Ask me before replying", "known" to "Reply on its own to known senders", "auto" to "Reply to everyone")

@Composable
private fun EmailFlow(botId: String, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var state by remember { mutableStateOf<String>("loading") }
    var address by remember { mutableStateOf("") }
    var mode by remember { mutableStateOf("ask") }
    var error by remember { mutableStateOf<String?>(null) }
    var saved by remember { mutableStateOf(false) }
    LaunchedEffect(botId) {
        val (rail, e1) = call("GET", "agent-email/status", doing = "check email")
        if (e1 == "unavailable" || rail.obj?.bool("configured") != true && e1 == null) { state = "off"; return@LaunchedEffect }
        if (e1 != null) { error = e1; state = "error"; return@LaunchedEffect }
        // iOS provisions (or returns) the bot's address with an empty POST.
        val (made, e2) = call("POST", "agents/${LibraryNet.escape(botId)}/identity/email", buildJsonObject { }, "set up the address")
        val addr = made.obj.str("address")
        if (addr == null) { error = e2 ?: "Couldn't set up the address. Try again."; state = "error"; return@LaunchedEffect }
        address = addr
        val (agent, _) = call("GET", "agents/${LibraryNet.escape(botId)}", doing = "load the bot")
        val m = agent.obj?.get("identityChannels").obj?.get("email").obj.str("replyMode")
        mode = if (m == "auto_known") "known" else m ?: "ask"
        state = "ready"
    }
    when (state) {
        "loading" -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary); Text("Setting up the address…", color = c.textSecondary, fontSize = 14.sp) }
        "off" -> Text("Allternit Mail isn't set up here yet. Email addresses for bots are coming soon.", color = c.textSecondary, fontSize = 14.sp)
        "error" -> Text(error.orEmpty(), color = AllternitStatic.StatusError, fontSize = 14.sp)
        else -> {
            Text("Your bot's address", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            Text(address, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.testTag("connectEmailAddress"))
            Text("Replies", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
            PillRow(replyModes.map { it.first }, mode, { id -> replyModes.first { it.first == id }.second }, { "connectEmailMode$it" }) { mode = it; saved = false }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            Text(if (saved) "Saved" else "Save", color = c.onAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.accentPrimary).clickable(role = Role.Button) {
                    scope.launch {
                        val wire = if (mode == "known") "auto_known" else mode
                        val body = buildJsonObject { putJsonObject("identityChannels") { putJsonObject("email") { put("replyMode", wire) } } }
                        val (_, e) = call("PATCH", "agents/${LibraryNet.escape(botId)}", body, "save the reply setting")
                        error = e; saved = e == null
                    }
                }.testTag("connectEmailSave").padding(vertical = 14.dp), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
        }
    }
}

// MARK: - VendorConnectView

private data class ConnectedBotRef(val botId: String, val name: String)
private data class ConnectProduct(val product: String, val name: String, val offers: String, val signIn: String, val terms: String?, val state: String?, val bots: List<ConnectedBotRef>) {
    val connected: Boolean get() = state == "CONNECTED"
    val signInText: String get() = when (signIn) {
        "desktop_app" -> "Uses the desktop app"; "api_key" -> "Uses an API key"; "local_endpoint" -> "Uses your local server"; "channel" -> "Uses WhatsApp"; else -> "Sign in on the website"
    }
}
private data class ConnectOutcome(val status: String, val message: String, val steps: List<String>, val terms: String?, val bots: List<ConnectedBotRef>) {
    val statusText: String get() = when (status) {
        "ready" -> "Ready"; "needs_consent" -> "Before you connect"; "needs_sign_in" -> "Waiting for you to sign in"; "needs_key" -> "Needs your API key"
        "needs_setup" -> "One step left"; "blocked" -> "Blocked by a verification check"; else -> status
    }
}

private fun JsonObject.botRefs() = list("bots").mapNotNull { b -> b.str("botId")?.let { ConnectedBotRef(it, b.str("name") ?: it) } }
private fun JsonObject?.list(key: String): List<JsonObject> = (this?.get(key) as? JsonArray)?.mapNotNull { it as? JsonObject }.orEmpty()

private fun extraFields(product: String, status: String?): List<Triple<String, String, Boolean>> = when {
    product == "claude-managed-agents" && status != "ready" -> listOf(Triple("apiKey", "Anthropic API key", true))
    product == "muse" && status != "ready" -> listOf(Triple("agentAddress", "Muse WhatsApp number", false), Triple("phoneNumberId", "WhatsApp phone number ID", false), Triple("accessToken", "WhatsApp access token", true), Triple("appSecret", "Meta app secret", true))
    else -> emptyList()
}

/** "Use a subscription you already pay for" (iOS VendorConnectView): the catalog, then one product's connect. */
@Composable
fun VendorConnectScreen(host: PhonesHost, onBack: () -> Unit, onReady: () -> Unit = {}) {
    val c = AllternitTheme.colors
    var products by remember { mutableStateOf<List<ConnectProduct>?>(null) }
    var unavailable by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var open by remember { mutableStateOf<ConnectProduct?>(null) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(host.signedIn, attempt) {
        if (!host.signedIn) return@LaunchedEffect
        products = null; error = null
        val (v, e) = call("GET", "gateway/connect", doing = "load your connections")
        when {
            e == "unavailable" -> unavailable = true
            e != null -> error = e
            else -> products = v.listUnder("products").map { o ->
                ConnectProduct(o.str("product").orEmpty(), o.str("name") ?: o.str("product").orEmpty(), o.str("offers").orEmpty(), o.str("signIn") ?: "browser", o.str("terms"), o["account"].obj.str("state"), o.botRefs())
            }.filter { it.product.isNotEmpty() }
        }
    }
    open?.let { VendorProductScreen(it, onBack = { open = null }, onReady = onReady); return }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("vendorConnect")) {
        LibraryHeader(title = "Connect a subscription", onBack = onBack, backTag = "vendorConnectBack")
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when {
                !host.signedIn -> FriendlyState(FriendlyStyle.Empty, "Sign in to connect a bot", "Connections belong to your Allternit account.", Modifier, "Sign in", host.openSignIn) { Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary) }
                unavailable -> FriendlyState(FriendlyStyle.Empty, "Update Allternit on your computer", "This connection needs a newer Allternit on your computer.", Modifier) { Icon(painterResource(DesignR.drawable.ph_arrow_down), null, tint = c.textSecondary) }
                error != null -> FriendlyState(FriendlyStyle.Offline, "Couldn’t reach your computer", error.orEmpty(), Modifier, "Retry", { attempt++ }) { Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = AllternitStatic.StatusWarning) }
                products == null -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary); Text("Loading…", color = c.textSecondary, fontSize = 14.sp) }
                else -> {
                    Text("Use a subscription you already pay for. Connect once and every chat, GPT, Project, Gem or bot you have there becomes an Allternit bot.", color = c.textSecondary, fontSize = 14.sp)
                    products.orEmpty().forEach { p ->
                        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).clickable(role = Role.Button) { open = p }.testTag("vendorProduct${p.product}").padding(14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                                Text(p.name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                                Text(p.offers, color = c.textSecondary, fontSize = 13.sp)
                                Text(p.signInText, color = c.textTertiary, fontSize = 12.sp)
                            }
                            if (p.connected) Text("Connected", color = AllternitStatic.StatusSuccess, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                        }
                    }
                }
            }
        }
    }
}

/** iOS VendorProductView + OneStepConnect: Connect, consent, extra fields, then wait for sign-in (polls every 3 s). */
@Composable
private fun VendorProductScreen(product: ConnectProduct, onBack: () -> Unit, onReady: () -> Unit) {
    val c = AllternitTheme.colors
    var result by remember { mutableStateOf<ConnectOutcome?>(null) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var polls by remember { mutableStateOf(0) }
    var accept by remember { mutableStateOf(false) }
    val fields = remember { androidx.compose.runtime.mutableStateMapOf<String, String>() }
    var go by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val status = result?.status ?: if (product.connected) "ready" else null
    val consent = if (result?.status == "needs_consent") result?.terms ?: product.terms else null

    LaunchedEffect(go) {
        if (go == 0) return@LaunchedEffect
        busy = true; error = null
        val body = buildJsonObject {
            put("product", product.product)
            if (accept) put("acceptTerms", true)
            fields.forEach { (k, v) -> if (v.isNotBlank()) put(k, v.trim()) }
        }
        val (v, e) = call("POST", "gateway/connect", body, "connect")
        busy = false
        if (e != null || v == null) { error = e ?: "Couldn't connect. Try again."; return@LaunchedEffect }
        val o = v.obj
        val r = ConnectOutcome(o.str("status") ?: "needs_setup", o.str("message").orEmpty(), (o?.get("action").obj?.get("steps") as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(), o?.get("action").obj.str("terms"), o?.botRefs().orEmpty())
        result = r
        if (r.status == "ready") onReady()
        if (r.status == "needs_sign_in" && polls < 100) { polls++; delay(3_000); go++ }
    }

    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("vendorProduct")) {
        LibraryHeader(title = product.name, onBack = onBack, backTag = "vendorProductBack")
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(product.offers, color = c.textSecondary, fontSize = 14.sp)
            result?.let { r ->
                Text(r.statusText, color = if (r.status == "ready") AllternitStatic.StatusSuccess else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                if (r.message.isNotEmpty()) Text(r.message, color = c.textSecondary, fontSize = 14.sp)
                r.steps.forEachIndexed { i, s -> Text("${i + 1}. $s", color = c.textSecondary, fontSize = 13.sp) }
            }
            consent?.let { t ->
                Text(t, color = c.textSecondary, fontSize = 13.sp)
                Row(Modifier.clickable(role = Role.Checkbox) { accept = !accept }.testTag("vendorAcceptTerms"), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.size(20.dp).border(BorderStroke(1.5.dp, c.textPrimary), RoundedCornerShape(5.dp)).background(if (accept) c.accentPrimary else androidx.compose.ui.graphics.Color.Transparent))
                    Text("I understand and agree", color = c.textPrimary, fontSize = 14.sp)
                }
            }
            extraFields(product.product, status).forEach { (key, label, _) -> LibraryField(label, fields[key].orEmpty(), label, "vendorField$key") { fields[key] = it } }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            val bots = result?.bots ?: product.bots
            if (status == "ready" && bots.isNotEmpty()) {
                Text("Bots from ${product.name}", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                bots.forEach { Text(it.name, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.padding(vertical = 4.dp)) }
            }
            if (status != "ready") {
                val label = when { busy -> "Connecting…"; status == "needs_sign_in" -> "Stop waiting"; else -> "Connect" }
                Text(label, color = c.onAccent, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.accentPrimary).clickable(enabled = !busy, role = Role.Button) {
                        if (status == "needs_sign_in") polls = 100 else go++
                    }.testTag("vendorConnectGo").padding(vertical = 14.dp))
            }
        }
    }
}
