package com.allternit.feature.phones

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.BorderStroke
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibrarySheet
import com.allternit.feature.artifacts.LibrarySheetAction
import com.allternit.feature.bots.BotAvatar
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.computer.AciApi
import com.allternit.feature.computer.AciDevice
import com.allternit.feature.computer.CloudPhone
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** What the shell gives the phone screens. */
class PhonesHost(
    val signedIn: Boolean,
    val backend: ChatBackend,
    val openSignIn: () -> Unit,
    /** Opens a bot's phone in Bots mode (iOS onOpenPhone). */
    val openBotPhone: (botId: String) -> Unit,
    /** Opens the Inbox phone-hold review (a task card's "Needs your OK"). */
    val openInbox: () -> Unit,
    /** Settings-style entries reached from the Phones sheet (iOS Settings › Connections rows). */
    val openPhoneControl: () -> Unit = {},
    val openAndroidScreen: () -> Unit = {},
    val openVendorConnect: () -> Unit = {},
)

/** The drawer's Phones button (iOS PhonesSheet): one row per bot with a phone and its unread, "Give a bot a phone". */
@Composable
fun PhonesSheet(host: PhonesHost, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var load by remember { mutableStateOf<PhonesLoad?>(null) }
    var attempt by remember { mutableStateOf(0) }
    var giving by remember { mutableStateOf(false) }
    LaunchedEffect(host.signedIn, attempt) {
        if (!host.signedIn) return@LaunchedEffect
        load = null
        load = BotPhonesApi.load()
        // iOS refreshes every minute while the sheet is up.
        while (true) { delay(60_000); load = BotPhonesApi.load() }
    }
    LibrarySheet("Your bots' phones", "phonesSheet", onDismiss, leading = LibrarySheetAction("Done", testTag = "phonesDone", onClick = onDismiss)) {
        Column(Modifier.fillMaxWidth().heightIn(min = 320.dp).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            val l = load
            when {
                !host.signedIn -> FriendlyState(FriendlyStyle.Empty, "Sign in to see your bots' phones", "Your bots' numbers, email addresses and channels live in your account.", Modifier, "Sign in", { onDismiss(); host.openSignIn() }) { Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary) }
                l == null -> Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(vertical = 12.dp)) {
                    CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp, color = c.textSecondary)
                    Text("Checking your bots…", color = c.textSecondary, fontSize = 14.sp)
                }
                l is PhonesLoad.Failed -> FriendlyState(FriendlyStyle.Offline, "Couldn't load your bots' phones", l.message, Modifier, "Retry", { attempt++ }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusWarning) }
                l is PhonesLoad.Ok && l.entries.none { it.hasPhone } -> Text("No bot has a phone yet. Give one a number, an email address or a messaging channel.", color = c.textSecondary, fontSize = 13.sp)
                l is PhonesLoad.Ok -> l.entries.filter { it.hasPhone }.forEach { e ->
                    Row(
                        Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(12.dp)).clickable(role = Role.Button, onClickLabel = "${e.bot.name}'s phone") { onDismiss(); host.openBotPhone(e.bot.id) }.testTag("phonesRow${e.bot.id}").padding(vertical = 6.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        BotAvatar(e.bot, 30.dp)
                        Text(e.bot.name, color = c.textPrimary, fontSize = 16.sp, maxLines = 1, modifier = Modifier.weight(1f))
                        if (e.unread > 0) Text(if (e.unread > 99) "99+" else "${e.unread}", color = androidx.compose.ui.graphics.Color.White, fontSize = 11.sp, fontWeight = FontWeight.Bold, modifier = Modifier.clip(CircleShape).background(AllternitStatic.StatusError).padding(horizontal = 7.dp, vertical = 2.dp))
                        Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textSecondary, modifier = Modifier.size(12.dp))
                    }
                }
            }
            Row(
                Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable(enabled = host.signedIn, role = Role.Button) { giving = true }.testTag("phonesGive").padding(vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Icon(painterResource(DesignR.drawable.ph_plus), null, tint = if (host.signedIn) c.accentPrimary else c.textTertiary, modifier = Modifier.size(18.dp))
                Text("Give a bot a phone", color = if (host.signedIn) c.accentPrimary else c.textTertiary, fontSize = 16.sp)
            }
            for ((label, tag, go) in listOf(Triple("Phone control (this phone)", "phonesControl", host.openPhoneControl), Triple("Android phone screen share", "phonesAndroidScreen", host.openAndroidScreen), Triple("Connect a subscription", "phonesVendorConnect", host.openVendorConnect))) {
                Text(label, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(role = Role.Button) { onDismiss(); go() }.testTag(tag).padding(vertical = 10.dp))
            }
        }
    }
    if (giving) GiveBotPhoneSheet(host, botId = null, onDismiss = { giving = false; attempt++ })
}

/** iOS AppPhonesSettingsView: let the bots use this phone; the paired phone, Allternit Link boards, unpair. */
@Composable
fun AppPhonesSettingsScreen(host: PhonesHost, onBack: () -> Unit, onOpenLive: (CloudPhone) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var devices by remember { mutableStateOf<List<AciDevice>?>(null) }
    var phones by remember { mutableStateOf<List<CloudPhone>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var boards by remember { mutableStateOf<List<LinkBoard>?>(null) }
    var boardsGate by remember { mutableStateOf(false) }
    var confirmUnpair by remember { mutableStateOf<CloudPhone?>(null) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(host.signedIn, attempt) {
        if (!host.signedIn) return@LaunchedEffect
        error = null
        when (val d = AciApi.devices()) {
            is AciApi.Result.Ok -> devices = d.value
            is AciApi.Result.Failed -> { devices = emptyList(); error = d.message }
        }
        (AciApi.phones() as? AciApi.Result.Ok)?.let { phones = it.value }
        val (b, gate) = host.backend.boards()
        boards = b; boardsGate = gate
    }
    val mine = phones.firstOrNull { it.isAndroidApp }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("appPhonesSettings")) {
        LibraryHeader(title = "Phone control", onBack = onBack, backTag = "appPhonesBack")
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("Let your bots use this phone", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
            Text("Pair this phone with one of your Desktops. Your bots can then see the screen while you share it and use the ACI browser in this app.", color = c.textSecondary, fontSize = 14.sp)
            when {
                !host.signedIn -> FriendlyState(FriendlyStyle.Empty, "Sign in to pair this phone", "Pairing links this phone to your Allternit account.", Modifier, "Sign in", host.openSignIn) { Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary) }
                devices == null -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary); Text("Loading your Desktops…", color = c.textSecondary, fontSize = 14.sp) }
                mine != null -> {
                    Text("Bots can reach this app", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                    Text("${mine.name} is ${if (mine.online) "online" else "offline"}. Bots reach this app only while it's open; a banner shows when they do, with Stop.", color = c.textSecondary, fontSize = 13.sp)
                    if (mine.online) Pill("Watch this phone", "appPhonesWatch") { onOpenLive(mine) }
                    Pill("Unpair this phone", "appPhonesUnpair", danger = true) { confirmUnpair = mine }
                }
                error != null -> { Text(error.orEmpty(), color = AllternitStatic.StatusError, fontSize = 14.sp); Pill("Try again", "appPhonesRetry") { attempt++ } }
                devices.orEmpty().isEmpty() -> Text("No Desktop is paired with your account yet. Open Allternit Desktop and sign in, then pull to refresh.", color = c.textSecondary, fontSize = 14.sp)
                else -> {
                    Text("Choose a Desktop", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium)
                    Text("The Desktop must be open. You'll approve this phone there. Pairing starts from Allternit's Share screen › Link with a code on this phone.", color = c.textSecondary, fontSize = 13.sp)
                    devices.orEmpty().forEach { d ->
                        Row(Modifier.fillMaxWidth().heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Box(Modifier.size(8.dp).clip(CircleShape).background(if (d.isOnline) AllternitStatic.StatusSuccess else c.textTertiary))
                            Text(d.displayName, color = c.textPrimary, fontSize = 15.sp, modifier = Modifier.weight(1f))
                            Text(if (d.isOnline) "Online" else "Offline", color = c.textSecondary, fontSize = 13.sp)
                        }
                    }
                }
            }
            Text("Allternit Link", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, modifier = Modifier.padding(top = 8.dp))
            when {
                boardsGate -> Text("Connect a computer to check your Allternit Link boards.", color = c.textSecondary, fontSize = 13.sp)
                boards == null -> Text("Looking for Allternit Link boards…", color = c.textSecondary, fontSize = 13.sp)
                boards.orEmpty().isEmpty() -> Text("Setup and pairing are done in Allternit Desktop. A board is armed only by pressing its own button; this app can only stop it.", color = c.textSecondary, fontSize = 13.sp)
                else -> boards.orEmpty().forEach { b ->
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Column(Modifier.weight(1f)) { Text(b.name, color = c.textPrimary, fontSize = 15.sp); Text(b.state, color = c.textSecondary, fontSize = 12.sp) }
                        Pill("Stop", "appPhonesStopBoard${b.id}", danger = true) { scope.launch { if (host.backend.stopBoard(b.id)) boards = host.backend.boards().first } }
                    }
                }
            }
        }
    }
    confirmUnpair?.let { p ->
        AlertDialog(
            onDismissRequest = { confirmUnpair = null },
            title = { Text("Unpair this phone?") },
            text = { Text("Bots on your Desktop won't be able to see this screen or use the ACI browser until you pair again.") },
            confirmButton = { TextButton(onClick = { confirmUnpair = null; scope.launch { if (AppPhonesApi.unpair(p.serial.removePrefix("app:"))) attempt++ else error = "Couldn't unpair. Try again." } }, modifier = Modifier.testTag("appPhonesConfirmUnpair")) { Text("Unpair", color = AllternitStatic.StatusError) } },
            dismissButton = { TextButton(onClick = { confirmUnpair = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun Pill(label: String, tag: String, danger: Boolean = false, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Text(label, color = if (danger) AllternitStatic.StatusError else c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.clip(CircleShape).border(BorderStroke(1.dp, if (danger) AllternitStatic.StatusError else c.borderStrong), CircleShape).clickable(role = Role.Button, onClick = onClick).testTag(tag).padding(horizontal = 16.dp, vertical = 10.dp))
}

/** iOS AndroidScreenPairingView: add an Android phone's screen: a six-digit code, or approve a request it made. */
@Composable
fun AndroidScreenPairingScreen(host: PhonesHost, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var lane by remember { mutableStateOf<AppPhonesApi.Lane>(AppPhonesApi.Lane.Checking) }
    var attempt by remember { mutableStateOf(0) }
    var code by remember { mutableStateOf<String?>(null) }
    var claimId by remember { mutableStateOf<String?>(null) }
    var claimError by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var pending by remember { mutableStateOf<List<PendingPair>>(emptyList()) }
    var answerError by remember { mutableStateOf<String?>(null) }
    var approvedName by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(host.signedIn, attempt) {
        if (!host.signedIn) { lane = AppPhonesApi.Lane.SignedOut; return@LaunchedEffect }
        lane = AppPhonesApi.Lane.Checking
        lane = when (val d = AciApi.devices()) {
            is AciApi.Result.Ok -> AppPhonesApi.lane(d.value)
            is AciApi.Result.Failed -> AppPhonesApi.Lane.Error(d.message)
        }
    }
    val ready = lane as? AppPhonesApi.Lane.Ready
    LaunchedEffect(ready?.runtimeId) {
        val id = ready?.runtimeId ?: return@LaunchedEffect
        while (true) { pending = AppPhonesApi.pending(id); delay(3_000) }
    }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().testTag("androidScreenPairing")) {
        LibraryHeader(title = "Android phone (screen share)", onBack = onBack, backTag = "androidPairBack")
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("See and tap your Android from any device.", color = c.textSecondary, fontSize = 14.sp)
            when (val l = lane) {
                AppPhonesApi.Lane.Checking -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) { CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = c.textSecondary); Text("Checking your computer…", color = c.textSecondary, fontSize = 14.sp) }
                is AppPhonesApi.Lane.Ready -> Text("The screen goes through ${l.name}. When you open the phone, tap Start on the phone to share.", color = c.textSecondary, fontSize = 14.sp)
                AppPhonesApi.Lane.None -> Text(AppPhonesApi.NO_RUNTIME, color = c.textSecondary, fontSize = 14.sp)
                is AppPhonesApi.Lane.Offline -> Text("${l.name} is offline. Open Allternit Desktop on it, or start your cloud computer, then check again.", color = c.textSecondary, fontSize = 14.sp)
                is AppPhonesApi.Lane.Old -> Text(AppPhonesApi.OLD_RUNTIME, color = c.textSecondary, fontSize = 14.sp)
                AppPhonesApi.Lane.SignedOut -> Text("Sign in to Allternit to add a phone.", color = c.textSecondary, fontSize = 14.sp)
                is AppPhonesApi.Lane.Error -> Text(l.message, color = AllternitStatic.StatusError, fontSize = 14.sp)
            }
            if (lane !is AppPhonesApi.Lane.Ready && lane != AppPhonesApi.Lane.Checking) Pill("Check again", "androidPairCheck") { attempt++ }
            if (ready != null) {
                listOf(
                    "On the Android phone, install Allternit with Phone Link from allternit.com/android. The Google Play version has no screen share.",
                    "On the phone, open Settings › Accessibility › Installed apps › Allternit and turn it on. Without it you can watch the screen but not tap.",
                    "In the Allternit app, tap Share screen › Link with a code and type this code.",
                ).forEachIndexed { i, t -> Text("${i + 1}. $t", color = c.textSecondary, fontSize = 14.sp) }
                code?.let { Text(it.chunked(3).joinToString(" "), color = c.textPrimary, fontSize = 34.sp, fontWeight = FontWeight.Bold, modifier = Modifier.testTag("androidPairCode")) }
                Pill(if (busy) "Making a code…" else if (code != null) "Get a new code" else "Get a code", "androidPairGetCode") {
                    if (!busy) { busy = true; claimError = null; scope.launch {
                        val id = java.util.UUID.randomUUID().toString().lowercase()
                        val (cd, err) = AppPhonesApi.claim(id, ready.runtimeId)
                        code = cd; claimError = err; claimId = id; busy = false
                    } }
                }
                claimError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
                approvedName?.let { Text("$it is linked. Open it from Computers › Phones, then tap Start on the phone.", color = c.textPrimary, fontSize = 14.sp) }
                pending.forEach { p ->
                    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPanel).padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text("${p.name} wants to link", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                        Text("Check that the phone shows ${p.code.chunked(3).joinToString(" ")}. ${p.model}", color = c.textSecondary, fontSize = 13.sp)
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Pill("Approve", "androidPairApprove${p.phoneId}") { scope.launch { answerError = AppPhonesApi.decide(p.phoneId, true); if (answerError == null) { approvedName = p.name; pending = pending.filterNot { it.phoneId == p.phoneId } } } }
                            Pill("Decline", "androidPairDecline${p.phoneId}", danger = true) { scope.launch { answerError = AppPhonesApi.decide(p.phoneId, false); if (answerError == null) pending = pending.filterNot { it.phoneId == p.phoneId } } }
                        }
                    }
                }
                answerError?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp) }
            }
        }
    }
}

/** iOS PhoneTaskCardView: one phone task run, read only. Goal, status, step, the last screenshot, "Needs your OK". */
@Composable
fun PhoneTaskCard(runId: String, backend: ChatBackend, onOpenInbox: () -> Unit, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    var run by remember(runId) { mutableStateOf<PhoneRun?>(null) }
    var lost by remember(runId) { mutableStateOf(false) }
    var thumb by remember(runId) { mutableStateOf<androidx.compose.ui.graphics.ImageBitmap?>(null) }
    LaunchedEffect(runId) {
        while (true) {
            val r = backend.phoneRun(runId)
            if (r == null) lost = true else { lost = false; run = r }
            backend.runtimeBytes("/api/v1/phones/runs/${com.allternit.feature.artifacts.LibraryNet.escape(runId)}/thumb", "image/*")?.takeIf { it.first == 200 }?.let { (_, bytes) ->
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { thumb = it.asImageBitmap() }
            }
            if (r?.terminal == true) break
            delay(2_000)
        }
    }
    Column(modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), RoundedCornerShape(14.dp)).testTag("phoneTaskCard$runId").padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val r = run
        if (r == null) {
            Text(if (lost) "Can't reach your computer to follow this phone task." else "Following the phone task…", color = c.textSecondary, fontSize = 14.sp)
        } else {
            Text(r.goal.ifEmpty { "Phone task" }, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 2)
            val tint = when { r.status in setOf("completed", "done") -> AllternitStatic.StatusSuccess; r.terminal -> AllternitStatic.StatusError; else -> AllternitStatic.StatusInfo }
            Text(r.status.replace('_', ' ').replaceFirstChar { it.uppercase() } + (r.stepLabel?.let { " · $it" } ?: if (!r.terminal && r.step > 0) " · step ${r.step}" else ""), color = tint, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            thumb?.let { Image(it, null, Modifier.fillMaxWidth().heightIn(max = 260.dp).clip(RoundedCornerShape(10.dp)), contentScale = ContentScale.Fit) }
            if (r.steps > 0) Text("${r.steps} steps" + if (r.failed > 0) ", ${r.failed} failed" else "", color = c.textSecondary, fontSize = 12.sp)
            if (r.approvalId != null && !r.terminal) Pill("Needs your OK", "phoneTaskReview$runId", onClick = onOpenInbox)
        }
    }
}
