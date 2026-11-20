package com.allternit.feature.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import com.allternit.core.design.R as DesignR

/* Account pages: Profile (AccountProfileView), Delete account, Organization & access, Privacy, Notifications. */

private const val DELETE_SUPPORT = "mailto:privacy@allternit.com?subject=Delete%20my%20Allternit%20account"

/** iOS AccountProfileView: who is signed in, the organization, and Delete account (two taps: the row, then Delete). */
@Composable
internal fun ProfilePage(host: SettingsHost, back: () -> Unit, push: (String) -> Unit) {
    val c = AllternitTheme.colors
    val account = host.account
    SPage("Profile", tag = "settingsProfile", onBack = back) {
        SCard {
            SRow(account?.displayName ?: "Not signed in", "profile_name", detail = account?.email ?: "Sign in to sync across devices", chevron = false)
            if (account?.organizationId != null) {
                SDivider()
                SRow("Accounts", "profile_org", detail = "Bots, chats and billing follow the active account.", onClick = { push("organization") })
            }
        }
        if (account != null) {
            SLabel("Danger zone")
            SCard { SRow("Delete account", "delete-account-row", icon = DesignR.drawable.ph_trash, destructive = true, onClick = { push("delete-account") }) }
            SNote("Deleting closes your Allternit account and signs you out everywhere.")
        } else {
            SNote("Sign in from the Settings root to see your account here.")
        }
    }
}

/**
 * iOS DeleteAccountView. The same call iOS makes (Clerk `user.delete()`), reachable only after typing DELETE.
 * Nothing here runs on its own.
 */
@Composable
internal fun DeleteAccountPage(host: SettingsHost, back: () -> Unit, onGone: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    var confirm by remember { mutableStateOf("") }
    var deleting by remember { mutableStateOf(false) }
    var deleted by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val confirmed = confirm.trim() == "DELETE"
    SPage("Delete account", tag = "settingsDeleteAccount", onBack = if (deleting || deleted) null else back) {
        if (deleted) {
            StateBox("Your account is deleted", "You've been signed out. Thanks for trying Allternit.", "Done") {
                scope.launch {
                    AllternitAuth.signOut()
                    onGone()
                }
            }
        } else {
            SLabel("What happens")
            SCard {
                host.account?.email?.let { SRow(it, "delete_email", chevron = false) }
                for ((i, line) in DELETE_POINTS.withIndex()) {
                    if (i > 0 || host.account?.email != null) SDivider()
                    SRow(line, "delete_point_$i", chevron = false)
                }
            }
            SNote("Want a copy first? Export your data from Settings > Privacy before you delete.")
            SLabel("Confirm")
            SField("Type DELETE", confirm, "delete_confirm", { confirm = it }, enabled = !deleting)
            SNote("Type DELETE in capitals to turn on the button.")
            SCard {
                SRow(if (deleting) "Deleting..." else "Delete my account", "delete_button", icon = DesignR.drawable.ph_trash, destructive = confirmed && !deleting,
                    enabled = confirmed && !deleting && host.account != null, chevron = false, onClick = {
                        deleting = true
                        error = null
                        scope.launch {
                            val result = AllternitAuth.deleteAccount()
                            if (result.ok) deleted = true else error = result.error ?: "Couldn't delete your account."
                            deleting = false
                        }
                    })
            }
            if (error != null) {
                SError("Couldn't delete your account. $error")
                SButton("Email privacy@allternit.com", "delete_email_support") { openLink(DELETE_SUPPORT) }
            }
        }
    }
}

private val DELETE_POINTS = listOf(
    "Your Allternit account is closed and you're signed out on every device.",
    "You can't sign back in to it. Signing up again with the same email starts a new, empty account.",
    "Your paid plan is cancelled right away and you won't be charged again. The rest of the period isn't refunded.",
    "Your cloud computers, phone numbers and bot mailboxes are deleted or released.",
    "The chats, bots, memory and files Allternit stores for you are erased.",
    "A computer you own keeps its own files. Allternit and its payment processor keep billing and audit records.",
    "Credits left on the account are not refunded.",
    "This can't be undone.",
)

/** iOS OrganizationAccessView: `GET me` and `POST me/organization`. */
@Composable
internal fun OrganizationPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (load, reload) = rememberLoad("me") { Net.get("me").toLoad("load your profile") }
    var creating by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    SPage("Organization & Access", tag = "settingsOrganization", onBack = back) {
        LoadView(load, reload, "load your profile") { body ->
            val user = body.obj.child("user") ?: body.obj
            SLabel("Profile")
            SCard {
                SRow(user.str("display_name") ?: user.str("displayName") ?: host.account?.displayName ?: "Account", "org_name", detail = user.str("email") ?: host.account?.email, value = user.str("role"), chevron = false)
                SDivider()
                SRow("User ID", "org_user_id", value = user.str("id") ?: user.str("user_id"), chevron = false)
            }
            SLabel("Organization")
            val orgId = user.str("organization_id") ?: user.str("organizationId")
            SCard {
                if (orgId != null) {
                    SRow("Organization", "org_id", value = orgId, chevron = false)
                    SDivider()
                    SRow("Role", "org_role", value = (user.str("organization_role") ?: user.str("organizationRole"))?.removePrefix("org:") ?: "member", chevron = false)
                } else {
                    SRow("No organization", "org_none", detail = "No organization resolved for this account.", chevron = false)
                    SDivider()
                    SRow(if (creating) "Creating..." else "Create personal organization", "org_create", enabled = !creating, chevron = false, onClick = {
                        creating = true
                        error = null
                        scope.launch {
                            val r = Net.send("POST", "me/organization")
                            if (r is ApiResult.Ok) reload() else error = r.friendly("create the organization")
                            creating = false
                        }
                    })
                }
            }
            SError(error)
            SNote("Real Clerk deployments get organization scope from Clerk's org-creation flow; this panel provides the fallback path used by self-hosted builds.")
        }
    }
}

/** iOS PrivacySettingsView: the model toggle, bulk archive and delete of chats, export, privacy policy. */
@Composable
internal fun PrivacyPage(host: SettingsHost, back: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val openLink = rememberLinkOpener()
    var improve by remember { mutableStateOf(SettingsPrefs.improveModel(context)) }
    var confirm by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf<String?>(null) }
    SPage("Privacy", tag = "settingsPrivacy", onBack = back) {
        SCard {
            SToggleRow("Improve the model for everyone", "privacy_improve", improve, detail = "Allow your chats to be used to improve Allternit's models. Saved on this device.",
                onChange = { improve = it; SettingsPrefs.setImproveModel(context, it) })
        }
        SLabel("Your data")
        SCard {
            SRow("Archive all chats", "privacy_archive", enabled = !working && host.account != null, onClick = { confirm = "archive" })
            SDivider()
            SRow("Delete all chats", "privacy_delete", destructive = true, enabled = !working && host.account != null, onClick = { confirm = "delete" })
            SDivider()
            SRow("Export data", "privacy_export", onClick = { openLink("https://ai.allternit.com/settings/privacy") })
            SDivider()
            SRow("Privacy policy", "privacy_policy", onClick = { openLink("https://allternit.com/privacy") })
        }
        if (working) SNote("Working...")
        result?.let { SNote(it) }
        confirm?.let { verb ->
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { confirm = null },
                title = { Text(if (verb == "archive") "Archive all chats?" else "Delete all chats?") },
                text = { Text(if (verb == "archive") "Archived chats are hidden from your history but kept on the server." else "This permanently deletes every conversation. This can't be undone.") },
                confirmButton = {
                    androidx.compose.material3.TextButton(onClick = {
                        confirm = null
                        working = true
                        result = null
                        scope.launch { result = bulkChats(verb); working = false }
                    }) { Text(if (verb == "archive") "Archive All Chats" else "Delete All Chats", color = if (verb == "delete") Danger else AllternitTheme.colors.textPrimary) }
                },
                dismissButton = { androidx.compose.material3.TextButton(onClick = { confirm = null }) { Text("Cancel") } },
            )
        }
    }
}

/** iOS runBulkOperation: list `agent-sessions`, then PATCH `{active:false}` or DELETE each. */
private suspend fun bulkChats(verb: String): String {
    val list = Net.get("agent-sessions")
    if (list !is ApiResult.Ok) return "Couldn't $verb chats. Listing conversations failed: ${list.friendly("list your chats")}"
    val sessions = list.value.listUnder("sessions", "items")
    val targets = if (verb == "archive") sessions.filter { it.bool("active") != false } else sessions
    if (targets.isEmpty()) return "Nothing to $verb. There are no chats to $verb."
    var ok = 0
    var firstError: String? = null
    for (s in targets) {
        val id = s.str("id") ?: continue
        val r = if (verb == "archive") Net.send("PATCH", "agent-sessions/${com.allternit.feature.artifacts.LibraryNet.escape(id)}", buildJsonObject { put("active", false) })
        else Net.send("DELETE", "agent-sessions/${com.allternit.feature.artifacts.LibraryNet.escape(id)}")
        if (r is ApiResult.Ok) ok += 1 else if (firstError == null) firstError = r.friendly("$verb the chat")
    }
    val noun = if (ok == 1) "chat" else "chats"
    return if (ok == targets.size) "Done. ${if (verb == "archive") "Archived" else "Deleted"} $ok $noun." else "Couldn't $verb all chats. $ok of ${targets.size} succeeded. First error: $firstError"
}

/** iOS NotificationsSettingsView: per-event choices, quiet hours, and critical-alert texts, all on cloud-api. */
@Composable
internal fun NotificationsPage(host: SettingsHost, back: () -> Unit) {
    val scope = rememberCoroutineScope()
    val (prefsLoad, reloadPrefs) = rememberLoad("prefs") { Net.cloud("GET", "api/v1/notifications/preferences").toLoad("load your notification choices") }
    val (quietLoad, reloadQuiet) = rememberLoad("quiet") { Net.cloud("GET", "api/v1/notifications/quiet-hours").toLoad("load your quiet hours") }
    val (smsLoad, reloadSms) = rememberLoad("sms") { Net.cloud("GET", "api/v1/alerts/sms").toLoad("load your alert number") }
    var error by remember { mutableStateOf<String?>(null) }
    val overrides = remember { mutableStateOf<Map<String, Boolean>>(emptyMap()) }
    SPage("Notifications", tag = "settingsNotifications", onBack = back) {
        ThisPhoneNotifications()
        SLabel("Phone notifications")
        SNote("For your whole account. These reach every phone and browser you turned notifications on for, even when Allternit is closed.")
        LoadView(prefsLoad, reloadPrefs, "load your notification choices") { body ->
            val rows = body.listUnder("preferences")
            if (body.obj.bool("pushConfigured") == false || body.obj.bool("push_configured") == false) SNote("Allternit can't send phone or browser notifications right now. Your choices are saved and apply once it can.")
            if (rows.isEmpty()) SNote("There's nothing to choose yet.") else SCard {
                rows.forEachIndexed { i, row ->
                    val event = row.str("event") ?: return@forEachIndexed
                    if (i > 0) SDivider()
                    val on = overrides.value[event] ?: (row.bool("enabled") ?: row.bool("default") ?: true)
                    SToggleRow(row.str("title") ?: event, "notif_$event", on, detail = row.str("description"), onChange = { next ->
                        overrides.value = overrides.value + (event to next)
                        error = null
                        scope.launch {
                            val r = Net.cloud("PUT", "api/v1/notifications/preferences", buildJsonObject { put("preferences", buildJsonObject { put(event, next) }) })
                            if (r !is ApiResult.Ok) {
                                overrides.value = overrides.value - event
                                error = "${row.str("title") ?: event}: ${r.friendly("save that change")}"
                            }
                        }
                    })
                }
            }
        }
        SError(error)
        SLabel("Critical alerts")
        LoadView(smsLoad, reloadSms, "load your alert number") { body ->
            SmsCard(body, reloadSms)
        }
        SLabel("Quiet hours")
        SNote("Notifications wait until quiet hours end, then you get one summary. Urgent ones still reach you. For your whole account.")
        LoadView(quietLoad, reloadQuiet, "load your quiet hours") { body -> QuietHoursCard(body.obj ?: JsonObject(emptyMap()), reloadQuiet) }
    }
}

@Composable
private fun SmsCard(body: JsonElement, reload: () -> Unit) {
    val scope = rememberCoroutineScope()
    val o = body.obj
    val saved = o.str("phone") ?: o.str("number") ?: ""
    val verified = o.bool("verified") == true
    var number by remember(saved) { mutableStateOf(saved) }
    var code by remember { mutableStateOf("") }
    var sent by remember { mutableStateOf(false) }
    var msg by remember { mutableStateOf<String?>(null) }
    SCard {
        SRow("Text me critical alerts", "sms_title", detail = if (saved.isEmpty()) "Add a number to get alert texts." else if (verified) "Verified: $saved gets critical alerts." else "Not verified yet: no alert texts go to $saved until you confirm it.", chevron = false)
    }
    SField("Phone number", number, "sms_number", { number = it })
    Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SButton("Save number", "sms_save", enabled = number.isNotBlank()) {
            scope.launch {
                val r = Net.cloud("PUT", "api/v1/alerts/sms", buildJsonObject { put("phone", number.trim()) })
                if (r is ApiResult.Ok) reload() else msg = r.friendly("save the number")
            }
        }
        SButton(if (sent) "Resend" else "Send code", "sms_send", enabled = number.isNotBlank()) {
            scope.launch {
                val r = Net.cloud("POST", "api/v1/alerts/sms/verify/start", buildJsonObject { put("phone", number.trim()) })
                if (r is ApiResult.Ok) { sent = true; msg = "Code sent." } else msg = r.friendly("send the code")
            }
        }
    }
    if (sent) {
        SField("Code", code, "sms_code", { code = it })
        SButton("Confirm", "sms_confirm", enabled = code.isNotBlank()) {
            scope.launch {
                val r = Net.cloud("POST", "api/v1/alerts/sms/verify/confirm", buildJsonObject { put("phone", number.trim()); put("code", code.trim()) })
                if (r is ApiResult.Ok) { sent = false; code = ""; msg = null; reload() } else msg = r.friendly("confirm the code")
            }
        }
    }
    SError(msg)
}

@Composable
private fun QuietHoursCard(q: JsonObject, reload: () -> Unit) {
    val scope = rememberCoroutineScope()
    var enabled by remember(q) { mutableStateOf(q.bool("enabled") == true) }
    var start by remember(q) { mutableStateOf(q.str("start") ?: "22:00") }
    var end by remember(q) { mutableStateOf(q.str("end") ?: "07:00") }
    var allowCalls by remember(q) { mutableStateOf(q.bool("allowCalls") ?: q.bool("allow_calls") ?: true) }
    var error by remember { mutableStateOf<String?>(null) }
    fun save(e: Boolean = enabled, s: String = start, en: String = end, calls: Boolean = allowCalls) {
        scope.launch {
            val body = buildJsonObject {
                put("enabled", e); put("start", s); put("end", en)
                put("timeZone", q.str("timeZone") ?: q.str("time_zone") ?: java.util.TimeZone.getDefault().id)
                put("allowCalls", calls); put("letThrough", q.str("letThrough") ?: q.str("let_through") ?: "urgent")
            }
            val r = Net.cloud("PUT", "api/v1/notifications/quiet-hours", body)
            error = if (r is ApiResult.Ok) null else "Quiet hours: ${r.friendly("save that change")}"
        }
    }
    SCard {
        SToggleRow("Quiet hours", "quiet_enabled", enabled, onChange = { enabled = it; save(e = it) })
        if (enabled) {
            SDivider()
            SToggleRow("Let calls through", "quiet_calls", allowCalls, onChange = { allowCalls = it; save(calls = it) })
        }
    }
    if (enabled) {
        SField("Starts (24 hour, HH:MM)", start, "quiet_start", { start = it })
        SField("Ends (24 hour, HH:MM)", end, "quiet_end", { end = it })
        Row(Modifier.padding(top = 8.dp)) { SButton("Save hours", "quiet_save", enabled = Regex("\\d{1,2}:\\d{2}").matches(start) && Regex("\\d{1,2}:\\d{2}").matches(end)) { save() } }
    }
    SError(error)
}

/**
 * iOS NotificationsSettingsView "This iPhone" row: whether this phone may show notifications. Allow asks through the
 * priming sheet (AppPermission.Notifications, wave H2) and then the system dialog, and registers the phone for push
 * (wave I1, AllternitPush). Blocked: opens Android's notification settings for the app.
 */
@androidx.compose.runtime.Composable
private fun ThisPhoneNotifications() {
    val context = LocalContext.current
    var allowed by remember { mutableStateOf(com.allternit.feature.push.AllternitPush.notificationsAllowed(context)) }
    fun openSystemSettings() {
        runCatching {
            context.startActivity(
                android.content.Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                    .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, context.packageName)
                    .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }
    }
    val ask = com.allternit.feature.chat.voice.rememberPrimedPermission(com.allternit.feature.chat.voice.AppPermission.Notifications) { granted ->
        allowed = com.allternit.feature.push.AllternitPush.notificationsAllowed(context)
        when {
            granted && allowed -> com.allternit.feature.push.AllternitPush.registerIfAuthorized(context)
            !allowed -> openSystemSettings()
            else -> Unit
        }
    }
    SLabel("This phone")
    SNote(
        when {
            !allowed -> "Allternit hasn’t been allowed to notify you on this phone yet."
            !com.allternit.feature.push.AllternitPush.isConfigured(context) -> "This build can’t receive push notifications."
            else -> com.allternit.feature.push.AllternitPush.lastRegistrationError(context)?.let { "Push isn’t set up yet: $it" }
                ?: "Banners from Allternit on this phone, even when the app is closed."
        },
    )
    if (!allowed) SButton("Allow notifications", tag = "settingsNotificationsAllow") { ask() }
}
