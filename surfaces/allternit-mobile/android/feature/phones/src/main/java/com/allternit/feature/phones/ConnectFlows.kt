package com.allternit.feature.phones

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryField
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.PillRow
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.RelayPick
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.add
import kotlinx.serialization.json.put

/*
 * The non-email connect flows (iOS ChannelConnectFlows.swift + ConnectAPI.swift ChannelConnectAPI): a phone
 * number (buy, port in, texting registration), managed Telegram bots and the shared-app OAuth channels
 * (Slack, Discord, Teams, WhatsApp Business). Nothing is bought, ported or connected until the person taps
 * the button that says so; opening this sheet only reads availability.
 */

@Composable
private fun ConnectButton(text: String, tag: String, enabled: Boolean = true, filled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(
        text, color = if (filled) c.onAccent else c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center,
        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(if (filled) c.accentPrimary else c.fillNeutral)
            .clickable(enabled = enabled, role = Role.Button, onClick = onClick).testTag(tag).padding(vertical = 14.dp),
    )
}

@Composable private fun Note(text: String, tag: String? = null) {
    val c = AllternitTheme.colors
    Text(text, color = c.textSecondary, fontSize = 14.sp, modifier = if (tag != null) Modifier.testTag(tag) else Modifier)
}

@Composable private fun Problem(text: String?) { if (text != null) Text(text, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.testTag("connectError")) }

@Composable private fun Busy(text: String) {
    val c = AllternitTheme.colors
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary); Text(text, color = c.textSecondary, fontSize = 14.sp) }
}

@Composable private fun Heading(text: String) { Text(text, color = AllternitTheme.colors.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold) }

private fun openUrl(context: android.content.Context, url: String) {
    runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
}

/** iOS ChannelConnectAPI.runtimeId: the computer the cloud binds a new connection to. */
private suspend fun runtimeId(host: PhonesHost): Pair<String?, String?> = when (val p = host.backend.relay.resolve()) {
    is RelayPick.Computer -> p.runtimeId to null
    else -> null to "No computer is online for your account. Turn your computer on (or connect one), then try again."
}

private fun JsonElement?.items(vararg keys: String): List<JsonObject> = listUnder(*keys)

// MARK: - Phone number (PhoneWizard.tsx)

private val verticals = listOf(
    "PROFESSIONAL" to "Professional services", "HEALTHCARE" to "Healthcare", "RETAIL" to "Retail", "REAL_ESTATE" to "Real estate",
    "EDUCATION" to "Education", "HOSPITALITY" to "Restaurants and hospitality", "TECHNOLOGY" to "Technology",
    "CONSTRUCTION" to "Construction and home services", "NON_PROFIT" to "Non-profit", "TRANSPORTATION" to "Transportation",
)

private data class NumberOption(val e164: String, val detail: String)
private data class RegState(val state: String, val reason: String?)

private fun registrationState(v: JsonElement?, individual: Boolean): RegState {
    val reg = v.obj.obj("registration")
    val state = reg.str("state") ?: "pending"
    val reason = reg.str("rejectionReason")
    return when {
        state == "approved" -> RegState("approved", null)
        state == "rejected" -> RegState("rejected", reason?.replace("campaign: ", ""))
        reason != null && reason.startsWith("waiting:") -> RegState("waiting", reason.removePrefix("waiting:").trim())
        individual && reg.str("campaignId").isNullOrEmpty() -> RegState("needs_code", null)
        else -> RegState("in_review", null)
    }
}

private fun JsonObject?.obj(key: String): JsonObject? = this?.get(key) as? JsonObject

/** cloudRegistrationForm: the carrier registration body. */
private fun registrationBody(values: Map<String, String>, individual: Boolean): JsonObject {
    fun v(k: String) = (values[k] ?: "").trim()
    val name = v("legalName").ifEmpty { listOf(v("firstName"), v("lastName")).filter { it.isNotEmpty() }.joinToString(" ") }
    var samples = v("sampleMessages").lines().map { it.trim() }.filter { it.isNotEmpty() }
    if (samples.isEmpty()) samples = listOf("$name: ${v("useCase").removeSuffix(".")}. Reply STOP to opt out.")
    return buildJsonObject {
        put("entityType", if (individual) "SOLE_PROPRIETOR" else "PRIVATE_PROFIT")
        put("legalName", name); put("displayName", name); put("website", v("website"))
        put("vertical", v("vertical").ifEmpty { "PROFESSIONAL" })
        put("street", v("street")); put("city", v("city")); put("state", v("state").uppercase())
        put("postalCode", v("postalCode")); put("country", "US")
        put("contactFirstName", v("firstName")); put("contactLastName", v("lastName"))
        put("contactEmail", v("email")); put("contactPhone", v("phone").ifEmpty { v("mobilePhone") })
        put("useCase", if (individual) "SOLE_PROPRIETOR" else "CUSTOMER_CARE")
        put("useCaseSummary", v("useCase"))
        put("sampleMessages", buildJsonArray { samples.forEach { add(it) } })
        if (individual) put("mobilePhone", v("mobilePhone")) else put("ein", v("ein").filter { it.isDigit() })
        if (v("optInPageUrl").isNotEmpty()) put("optInPageUrl", v("optInPageUrl"))
    }
}

@Composable
internal fun PhoneNumberFlow(host: PhonesHost, botId: String, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var stage by remember { mutableStateOf("loading") }
    var working by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var tollFree by remember { mutableStateOf(false) }
    var areaCode by remember { mutableStateOf("") }
    var options by remember { mutableStateOf<List<NumberOption>>(emptyList()) }
    var chosen by remember { mutableStateOf<String?>(null) }
    var accountId by remember { mutableStateOf<String?>(null) }
    var e164 by remember { mutableStateOf("") }
    var individual by remember { mutableStateOf(false) }
    var porting by remember { mutableStateOf(false) }
    var registration by remember { mutableStateOf<RegState?>(null) }
    var portState by remember { mutableStateOf("pending") }
    var pin by remember { mutableStateOf("") }
    val form = remember { mutableStateMapOf("vertical" to "PROFESSIONAL") }
    LaunchedEffect(Unit) {
        val (v, e) = call("GET", "channels/availability", doing = "check phone numbers")
        stage = if (e == "unavailable" || (e == null && v.obj.bool("phone") != true)) "off" else "intro"
    }
    // Waiting steps poll like iOS (5 s for the carrier registration, 10 s for a port).
    LaunchedEffect(stage, accountId) {
        val id = accountId ?: return@LaunchedEffect
        if (stage == "registration") while (true) {
            val (v, e) = call("GET", "phone/numbers/${LibraryNet.escape(id)}/registration", doing = "check the registration")
            registration = if (e == "unavailable") RegState("not_submitted", null) else if (v != null) registrationState(v, individual) else registration
            if (registration?.state == "approved") { stage = "done"; break }
            if (registration?.state == "rejected") break
            delay(5000)
        }
        if (stage == "port-wait") while (true) {
            val (v, _) = call("GET", "phone/numbers/${LibraryNet.escape(id)}/port", doing = "check the port")
            val s = (v.obj.obj("port").str("state") ?: "").lowercase()
            portState = when { s in listOf("ported", "completed", "done", "activated") -> "done"; s in listOf("cancelled", "canceled", "exception", "rejected", "failed") -> "rejected"; else -> "pending" }
            if (portState != "pending") { if (portState == "done") stage = "done"; break }
            delay(10000)
        }
    }
    fun run(block: suspend () -> Unit) { if (working) return; working = true; error = null; scope.launch { block(); working = false } }
    when (stage) {
        "loading" -> Busy("Checking phone numbers…")
        "off" -> Note("Give a bot a real phone number: anyone can call it or text it, and it answers or follows up. Phone numbers aren’t available on your account yet. They come with an upcoming update.", "connectPhoneOff")
        "intro" -> {
            Heading("A real number for your bot")
            Note("Pick a number and your bot can take calls and texts.")
            listOf("Search by area code and pick a local or toll-free number.", "Calls work the moment the number is yours.", "SMS turns on when the carrier approves your registration.", "Already have a number? Choose to port it in below.").forEach { Note("•  $it") }
            Text("Number", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
            PillRow(listOf("new", "port"), if (porting) "port" else "new", { if (it == "port") "Port my existing number" else "Get a new number" }, { "connectPhoneSource$it" }) { porting = it == "port" }
            ConnectButton("Continue", "connectPhoneContinue") { stage = if (porting) "port" else "search" }
        }
        "search" -> {
            Heading("Find a number")
            PillRow(listOf("local", "tollfree"), if (tollFree) "tollfree" else "local", { if (it == "tollfree") "Toll-free" else "Local" }, { "connectPhoneKind$it" }) { tollFree = it == "tollfree" }
            Note(if (tollFree) "An 8xx number with no area code. Texting verification usually takes a few days." else "A number with an area code, e.g. +1 (212) 555 0132.")
            LibraryField("Area code", areaCode, "212", "connectPhoneArea") { areaCode = it.filter { ch -> ch.isDigit() }.take(3) }
            Problem(error)
            ConnectButton(if (working) "Searching…" else "Search", "connectPhoneSearch", !working) {
                run {
                    var path = "phone/numbers/search?type=${if (tollFree) "toll_free" else "local"}&limit=10"
                    if (areaCode.isNotBlank()) path += "&areaCode=${LibraryNet.escape(areaCode.trim())}"
                    val (v, e) = call("GET", path, doing = "search numbers")
                    val found = v.items("numbers").mapNotNull { n -> n.str("e164")?.let { NumberOption(it, listOfNotNull(n.str("locality"), n.str("monthlyCost")?.let { m -> "$$m/mo carrier cost" }).filter { s -> s.isNotEmpty() }.joinToString(" · ")) } }
                    when {
                        e != null -> error = "Couldn’t search numbers: ${if (e == "unavailable") "not available yet" else e}"
                        found.isEmpty() -> error = "No numbers found for that search. Try a nearby area code, or toll-free."
                        else -> { options = found; chosen = found.first().e164; stage = "pick" }
                    }
                }
            }
        }
        "pick" -> {
            Heading("Pick your number")
            options.forEach { o ->
                Row(Modifier.fillMaxWidth().clickable(role = Role.RadioButton) { chosen = o.e164 }.testTag("connectPhoneOption${o.e164}").padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(if (chosen == o.e164) "●" else "○", color = c.accentPrimary, fontSize = 16.sp)
                    Column(Modifier.weight(1f)) { Text(o.e164, color = c.textPrimary, fontSize = 16.sp); if (o.detail.isNotEmpty()) Text(o.detail, color = c.textSecondary, fontSize = 12.sp) }
                }
            }
            Problem(error)
            // The only call here that spends money: it runs on this tap and nowhere else (iOS "Buy this number").
            ConnectButton(if (working) "Buying…" else "Buy this number", "connectPhoneBuy", !working && chosen != null) {
                run {
                    val (rt, re) = runtimeId(host)
                    if (rt == null) { error = re; return@run }
                    val (v, e) = call("POST", "phone/numbers", buildJsonObject { put("e164", chosen.orEmpty()); put("runtimeId", rt); put("botId", botId); put("type", if (tollFree) "toll_free" else "local") }, "buy the number")
                    val id = v.obj.obj("number").str("id")
                    if (id == null) { error = "Couldn’t buy the number: ${e ?: "its details didn’t come back. Check Your bots’ phones."}"; return@run }
                    accountId = id; e164 = v.obj.obj("number").str("e164") ?: chosen.orEmpty(); stage = "calls"
                }
            }
        }
        "calls" -> {
            Heading("Calls work now")
            Note("$e164 is wired: calls are answered in the cloud and land in your bot’s threads, and a missed call gets a follow-up. One more step turns on texting.")
            ConnectButton("Continue", "connectPhoneCallsNext") { stage = "sender" }
        }
        "sender" -> {
            Heading("Turn on texting")
            Note("US carriers ask who is texting before a number can send texts. Calls already work while this is reviewed.")
            PillRow(listOf("business", "individual"), if (individual) "individual" else "business", { if (it == "individual") "Just me" else "A business" }, { "connectPhoneSender$it" }) { individual = it == "individual" }
            Note(if (individual) "A person or sole proprietor without an EIN. The carrier texts you a code to confirm it’s you." else "Has a US tax ID (EIN).")
            ConnectButton("Next", "connectPhoneSenderNext") { stage = "details" }
        }
        "details" -> {
            Heading(if (individual) "About you" else "Your business")
            Note(if (individual) "For a person or sole proprietor without an EIN. Carriers allow one number per person this way." else "Use the details on file for the business.")
            @Composable fun field(key: String, label: String, ph: String, multi: Boolean = false) = LibraryField(label, form[key].orEmpty(), ph, "connectPhone_$key", singleLine = !multi, minHeight = if (multi) 90 else 48) { form[key] = it }
            if (individual) {
                field("firstName", "First name", ""); field("lastName", "Last name", "")
                field("legalName", "Name people will see (optional)", "Dana Lee Tutoring")
                field("mobilePhone", "Your mobile number", "+1 612 555 0188"); Note("The carrier texts a code here. Not the bot’s number.")
                field("email", "Email", "")
            } else {
                field("legalName", "Legal business name", "Acme Corp"); field("ein", "EIN (tax ID)", "12-3456789")
            }
            Text("INDUSTRY", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.Medium)
            PillRow(verticals.map { it.first }, form["vertical"] ?: "PROFESSIONAL", { id -> verticals.first { it.first == id }.second }, { "connectPhoneVertical$it" }) { form["vertical"] = it }
            field("street", "Street address", ""); field("city", "City", ""); field("state", "State", "MN"); field("postalCode", "ZIP code", "")
            if (!individual) {
                field("firstName", "Contact first name", ""); field("lastName", "Contact last name", ""); field("email", "Contact email", "")
                field("phone", "Contact phone", "+1 612 555 0100"); field("website", "Website (optional)", "https://acme.com")
            }
            field("useCase", "Use case", "What the number texts about, in plain language.", true)
            field("sampleMessages", "Sample messages (optional)", "One per line. Include your business name.", true)
            field("optInPageUrl", "Opt-in page (optional)", "https://acme.com/text-us")
            Note("Leave the opt-in page blank and Allternit hosts one for you.")
            Problem(error)
            ConnectButton(if (working) "Submitting…" else if (individual) "Send me the code" else "Submit registration", "connectPhoneSubmit", !working) {
                run {
                    val id = accountId
                    if (id == null) { error = "Pick a number first."; return@run }
                    val (v, e) = call("POST", "phone/numbers/${LibraryNet.escape(id)}/registration", registrationBody(form, individual), "submit the registration")
                    if (e != null) { error = "Couldn’t submit the registration: ${if (e == "unavailable") "not available yet" else e}"; return@run }
                    registration = registrationState(v, individual)
                    stage = if (registration?.state == "needs_code") "code" else "registration"
                }
            }
        }
        "code" -> {
            Heading("Enter the code")
            Note("The carrier texted a code to your mobile number. Enter it to confirm it’s you; then the registration continues by itself.")
            LibraryField("Code", pin, "123456", "connectPhoneCode") { pin = it.filter { ch -> ch.isDigit() }.take(6) }
            Problem(error)
            ConnectButton(if (working) "Checking…" else "Confirm", "connectPhoneCodeConfirm", !working && pin.length == 6) {
                run {
                    val id = accountId ?: return@run
                    val (v, e) = call("POST", "phone/numbers/${LibraryNet.escape(id)}/registration/otp", buildJsonObject { put("pin", pin) }, "check the code")
                    if (e != null) { error = e; return@run }
                    registration = registrationState(v, true); stage = "registration"
                }
            }
            ConnectButton("Send a new code", "connectPhoneCodeResend", !working, filled = false) {
                run { accountId?.let { call("POST", "phone/numbers/${LibraryNet.escape(it)}/registration/otp", buildJsonObject { }, "resend the code") } }
            }
        }
        "registration" -> {
            Heading("Texting registration")
            val r = registration
            Busy(when (r?.state) {
                "approved" -> "Approved. Texting is on."
                "rejected" -> "Rejected"
                "waiting" -> "Filed. It is waiting on Allternit’s side and continues by itself. You can close this."
                "needs_code" -> "Waiting for the code the carrier texted to your mobile."
                else -> "Waiting for carrier approval…"
            })
            if (r?.state == "rejected") {
                Problem("The carrier rejected the registration${r.reason?.let { ": $it" } ?: ""}. Go back, fix the details and submit again.")
                ConnectButton("Fix the details", "connectPhoneFix", filled = false) { stage = "details" }
            } else {
                Note("In carrier review (usually a few days to two weeks; the carriers set the pace). You can continue. Texting turns on by itself when approved.")
                ConnectButton("Continue", "connectPhoneSkip", filled = false) { stage = "done" }
            }
        }
        "port" -> {
            Heading("Which number should we port?")
            Note("Porting moves your existing number to Allternit’s carrier. Your number keeps working until the port completes.")
            LibraryField("Phone number", e164, "+1 415 555 0132", "connectPhonePortNumber") { e164 = it }
            Note("Include the country code.")
            Problem(error)
            ConnectButton(if (working) "Starting…" else "Start the port", "connectPhonePortStart", !working && e164.isNotBlank()) {
                run {
                    val (rt, re) = runtimeId(host)
                    if (rt == null) { error = re; return@run }
                    val (v, e) = call("POST", "phone/numbers/port", buildJsonObject { put("e164", e164.trim()); put("runtimeId", rt); put("botId", botId) }, "start the port")
                    val id = v.obj.obj("number").str("id")
                    if (id == null) { error = "Couldn’t start the port: ${e ?: "its details didn’t come back. Check Your bots’ phones."}"; return@run }
                    accountId = id; stage = "port-wait"
                }
            }
        }
        "port-wait" -> {
            Heading("Port in progress")
            if (portState == "rejected") Problem("The port was rejected. Fix the details and try again.") else Busy("Waiting for the carrier…")
            Note("Carriers take a few days (usually a few business days). You can keep waiting or continue; we email you when it completes.")
            ConnectButton("Continue", "connectPhonePortSkip", filled = false) { stage = "done" }
        }
        else -> {
            Heading(if (porting) "Port started" else "Your number is live")
            Note(
                if (porting) "${e164.ifEmpty { "Your number" }} answers as your bot once the port completes. Carriers take a few days; we email you when it’s done, and the number keeps working on your old carrier until then."
                else "$e164 answers calls now, and texts arrive as threads." + when (registration?.state) {
                    "approved" -> " Texting is approved. The bot can text back."
                    "in_review" -> " Texting is in carrier review; it turns on by itself when approved."
                    "waiting" -> " Texting registration is filed and waiting on Allternit’s side; it continues by itself."
                    "needs_code" -> " Texting needs the code the carrier texted to your mobile. Reopen this to enter it."
                    "rejected" -> " Texting registration was rejected. Fix the details and resubmit."
                    else -> ""
                }, "connectPhoneDone",
            )
            ConnectButton("Done", "connectPhoneFinish", onClick = onClose)
        }
    }
}

// MARK: - Telegram (TelegramWizard.tsx)

@Composable
internal fun TelegramFlow(host: PhonesHost, botId: String, onClose: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var stage by remember { mutableStateOf("loading") }
    var managed by remember { mutableStateOf(true) }
    var working by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var name by remember { mutableStateOf("") }
    var token by remember { mutableStateOf("") }
    var onboardingId by remember { mutableStateOf<String?>(null) }
    var confirmUrl by remember { mutableStateOf<String?>(null) }
    var pairUrl by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        val (v, _) = call("GET", "gateway/channel-accounts/telegram/managed/available", doing = "check Telegram")
        managed = v.obj.bool("available") ?: false
        stage = if (managed) "intro" else "token"
    }
    LaunchedEffect(stage, onboardingId) {
        val id = onboardingId ?: return@LaunchedEffect
        if (stage != "pair") return@LaunchedEffect
        while (true) {
            val (v, e) = call("GET", "channel-onboarding/telegram/${LibraryNet.escape(id)}", doing = "check Telegram")
            val s = v.obj.str("state") ?: ""
            if (s == "paired") { stage = "done"; break }
            if (s == "failed" || s == "expired") { error = v.obj.str("error") ?: if (s == "expired") "The Telegram link expired. Start again." else "Telegram couldn’t create the bot. Start again."; stage = "name"; break }
            if (s == "connected") pairUrl = v.obj.str("pairUrl")
            if (e != null && e != "unavailable") error = e
            delay(3000)
        }
    }
    fun run(block: suspend () -> Unit) { if (working) return; working = true; error = null; scope.launch { block(); working = false } }
    when (stage) {
        "loading" -> Busy("Checking Telegram…")
        "intro" -> {
            Heading("A real Telegram bot, without a token")
            Note("Allternit creates a Telegram bot for you through Telegram’s managed-bot program. You never paste a token.")
            listOf("Name the bot and we create it on Telegram.", "Tap Confirm once in the Telegram chat that opens.", "Open your new bot and tap Start to pair it with Allternit.").forEach { Note("•  $it") }
            ConnectButton("Continue", "connectTelegramContinue") { stage = "name" }
        }
        "name" -> {
            Heading("Name your Telegram bot")
            LibraryField("Bot name", name, "Acme Support", "connectTelegramName") { name = it }
            Note("Shown as the bot’s display name in Telegram.")
            Problem(error)
            ConnectButton(if (working) "Creating…" else "Create the bot", "connectTelegramCreate", !working && name.isNotBlank()) {
                run {
                    val rt = runtimeId(host)
                    if (rt.first == null) { error = rt.second; return@run }
                    val (v, e) = call("POST", "channel-onboarding/telegram", buildJsonObject { put("runtimeId", rt.first.orEmpty()); put("botName", name.trim()); put("botId", botId) }, "create the Telegram bot")
                    val id = v.obj.str("onboardingId"); val url = v.obj.str("createUrl")
                    if (id == null || url == null) { error = "Telegram didn’t accept that (${e ?: "no link to confirm"}). Try a different name."; return@run }
                    onboardingId = id; confirmUrl = url; pairUrl = null; stage = "pair"
                }
            }
        }
        "pair" -> {
            Heading("Pairing with Telegram")
            Busy("Waiting for Telegram…")
            val link = pairUrl ?: confirmUrl
            Note(if (pairUrl != null) "Your bot is created. Open it in Telegram and tap Start so Allternit knows it is yours." else "Tap Confirm in the Telegram chat that opens. This is the only step that happens in Telegram.")
            link?.let { u -> ConnectButton(if (pairUrl != null) "Open your new bot in Telegram" else "Open Telegram to confirm", "connectTelegramOpen") { openUrl(context, u) } }
            Problem(error)
            ConnectButton("Cancel connection", "connectTelegramCancel", filled = false, onClick = onClose)
        }
        "token" -> {
            Heading("Connect your Telegram bot")
            Note("Create a bot with @BotFather in Telegram (send /newbot), then paste the token it gives you.")
            LibraryField("Bot token", token, "123456:ABC-DEF…", "connectTelegramToken") { token = it.trim() }
            LibraryField("Name (optional)", name, "", "connectTelegramDisplay") { name = it }
            Problem(error)
            ConnectButton(if (working) "Connecting…" else "Connect", "connectTelegramConnect", !working && token.isNotBlank()) {
                run {
                    val (_, e) = call("POST", "gateway/channel-accounts/telegram", buildJsonObject { put("botToken", token); if (name.isNotBlank()) put("displayName", name.trim()) }, "connect Telegram")
                    if (e != null) error = if (e == "unavailable") "Telegram isn’t available on your account yet." else e else stage = "done"
                }
            }
        }
        else -> {
            Heading("Telegram is connected")
            Note(if (managed) "Your new bot answers the Allternit bots you switch it on for. Messages wait while your computer is off and arrive in order." else "Your Telegram bot answers the Allternit bots you switch it on for. Messages wait while your computer is off and arrive in order.", "connectTelegramDone")
            ConnectButton("Done", "connectTelegramFinish", onClick = onClose)
        }
    }
}

// MARK: - Slack, Discord, Teams, WhatsApp Business (shared-app OAuth)

private class OAuthCopy(val title: String, val body: String, val label: String, val hint: String, val waiting: String, val done: String)

private fun oauthCopy(p: ChannelProvider) = when (p) {
    ChannelProvider.Slack -> OAuthCopy("Add Allternit to Slack", "You authorize the Allternit app on Slack’s own page. Pick your workspace, then close the page to come back.", "Add to Slack", "If your workspace needs admin approval, Slack tells you right away. Your admin gets the request, and this keeps waiting for the grant.", "Waiting for Slack…", "The bots answer where you invite them. Mention a bot by name to hand it a conversation, or DM it directly.")
    ChannelProvider.Discord -> OAuthCopy("Add Allternit to your server", "You authorize the Allternit app on Discord’s own page. Pick your server, then close the page to come back.", "Add to Discord", "Discord asks for a server and the permissions the bots need. The app’s token stays in Allternit’s cloud, never on your computer.", "Waiting for Discord…", "The bots answer in the channels you allowed. \"@botname\" in a message picks who answers.")
    ChannelProvider.Teams -> OAuthCopy("Sign in with Microsoft", "You sign in on Microsoft’s own page and consent for yourself. If your org blocks new apps, ask your IT admin to allow \"Allternit\" in the Teams admin center, Teams apps.", "Connect Teams", "A work or school account works. Allternit asks for the profile and the ability to chat as you.", "Waiting for Microsoft…", "Teams sees one app (@Allternit), so start a message with the bot’s name, like \"@Allternit researcher, draft the reply\".")
    else -> OAuthCopy("Connect your WhatsApp Business number", "You connect the number on Meta’s own signup page, then close the page to come back.", "Continue with Meta", "You need a WhatsApp Business account. Personal numbers aren’t supported on the phone.", "Waiting for Meta…", "Your bots answer on your WhatsApp Business number.")
}

private data class DiscordChannel(val id: String, val name: String, val selected: Boolean)

@Composable
internal fun OAuthChannelFlow(host: PhonesHost, provider: ChannelProvider, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val copy = remember(provider) { oauthCopy(provider) }
    val key = provider.name.lowercase()
    var stage by remember { mutableStateOf("loading") }
    var authorizeUrl by remember { mutableStateOf<String?>(null) }
    var comingSoon by remember { mutableStateOf<String?>(null) }
    var waitingOn by remember { mutableStateOf(false) }
    var workspace by remember { mutableStateOf<String?>(null) }
    var guildId by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val channels = remember { mutableStateListOf<DiscordChannel>() }
    val picked = remember { mutableStateListOf<String>() }
    LaunchedEffect(provider) {
        val soon = "${provider.blurb} This connection type isn’t available on your account yet. It ships with an upcoming update."
        val (v, e) = when (provider) {
            ChannelProvider.Slack -> call("GET", "channels/slack/install", doing = "start Slack").let { (r, err) -> if (r.obj.bool("configured") == false) null to "unavailable" else r to err }
            ChannelProvider.Discord, ChannelProvider.Whatsapp -> {
                if (provider == ChannelProvider.Whatsapp && call("GET", "channels/availability", doing = "check WhatsApp").first.obj.bool("whatsapp") != true) null to "unavailable"
                else {
                    val rt = runtimeId(host)
                    if (rt.first == null) null to rt.second
                    else call("POST", if (provider == ChannelProvider.Discord) "channels/discord/install" else "channels/whatsapp/connect", buildJsonObject { put("runtimeId", rt.first.orEmpty()) }, "start ${provider.label}")
                }
            }
            else -> call("POST", "channels/teams/connect", buildJsonObject { }, "start Teams")
        }
        authorizeUrl = v.obj.str("url")
        when {
            e == "unavailable" || (e == null && authorizeUrl == null) -> { comingSoon = soon; stage = "soon" }
            e != null -> { comingSoon = "Couldn’t start ${provider.label}: $e"; stage = "soon" }
            else -> stage = "add"
        }
    }
    LaunchedEffect(stage) {
        if (stage != "grant") return@LaunchedEffect
        while (true) {
            val pick = if (provider != ChannelProvider.Discord) host.backend.relay.resolve() else null
            val body = buildJsonObject { if (pick is RelayPick.Computer) put("runtimeId", pick.runtimeId) }
            val (v, e) = call("POST", "gateway/channel-accounts/$key", body, "check the connection")
            val account = v.obj.obj("account")
            if (e == null && account != null) {
                workspace = account.str("displayName")
                if (provider == ChannelProvider.Discord) account.str("handle")?.let { g ->
                    guildId = g
                    val (cv, _) = call("GET", "channels/discord/installs/${LibraryNet.escape(g)}/channels", doing = "load the channels")
                    channels.clear(); picked.clear()
                    cv.items("channels").forEach { ch -> ch.str("id")?.let { id -> channels.add(DiscordChannel(id, ch.str("name") ?: id, ch.bool("selected") ?: true)); if (ch.bool("selected") != false) picked.add(id) } }
                }
                stage = if (channels.isNotEmpty()) "channels" else "done"; break
            }
            if (e != null && e != "unavailable") error = e
            delay(3000)
        }
    }
    when (stage) {
        "loading" -> Busy("Starting ${provider.label}…")
        "soon" -> Note(comingSoon.orEmpty(), "connectOAuthSoon")
        "add" -> {
            Heading(copy.title)
            Note(copy.body)
            ConnectButton(copy.label, "connectOAuthAdd$key") { authorizeUrl?.let { openUrl(context, it) }; waitingOn = true }
            Note(copy.hint)
            if (waitingOn) ConnectButton("I’ve finished", "connectOAuthFinished$key", filled = false) { stage = "grant" }
        }
        "grant" -> {
            Heading("Finishing the connection")
            Busy(copy.waiting)
            Note("Finish the page that opened if you haven’t. Nothing else to do here.")
            authorizeUrl?.let { u -> ConnectButton("Open the page again", "connectOAuthReopen$key", filled = false) { openUrl(context, u) } }
            Problem(error)
            ConnectButton("Cancel connection", "connectOAuthCancel$key", filled = false, onClick = onClose)
        }
        "channels" -> {
            Heading("Which channels can the bots use?")
            Note("The bots only answer where you allow; messages in other channels never reach your computer. Direct messages and slash commands always work. Pick at least one.")
            channels.forEach { ch ->
                Row(Modifier.fillMaxWidth().clickable(role = Role.Checkbox) { if (ch.id in picked) picked.remove(ch.id) else picked.add(ch.id) }.testTag("connectDiscordChannel${ch.id}").padding(vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(if (ch.id in picked) "☑" else "☐", color = c.accentPrimary, fontSize = 16.sp)
                    Text("#${ch.name}", color = c.textPrimary, fontSize = 15.sp)
                }
            }
            Problem(error)
            ConnectButton(if (working) "Saving…" else "Save channels", "connectDiscordSave", !working && picked.isNotEmpty()) {
                val g = guildId ?: return@ConnectButton
                working = true; error = null
                scope.launch {
                    // Every channel ticked = no limit, so channels made later work too.
                    val ids = if (picked.size == channels.size) emptyList() else picked.toList()
                    val (_, e) = call("PUT", "channels/discord/installs/${LibraryNet.escape(g)}/channels", buildJsonObject { put("channelIds", buildJsonArray { ids.forEach { add(it) } }) }, "save the channels")
                    if (e != null) error = "Couldn’t save the channels: $e. The bots keep answering in every channel; try again." else stage = "done"
                    working = false
                }
            }
        }
        else -> {
            Heading("${provider.label} is connected")
            Note((workspace?.let { "Connected to $it. " } ?: "") + copy.done, "connectOAuthDone$key")
            ConnectButton("Done", "connectOAuthFinish$key", onClick = onClose)
        }
    }
}
