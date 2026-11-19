package com.allternit.feature.computer

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.core.rfb.ComputerDriveScreen
import com.allternit.core.rfb.ComputerRecord
import com.allternit.core.rfb.ComputersApi
import com.allternit.core.rfb.DriveTopBar
import kotlinx.coroutines.launch
import java.text.DateFormat
import java.time.Instant
import java.util.Date

/**
 * ACI › Computers (iOS ACIComputersPane, AciScreen.tsx `ComputersPane`): Add a computer (the pairing
 * steps), View a computer's screen, the paired computers, linked phones, cloud computer backups and
 * workflows. A computer opens its live desktop (Files-drive computers open their detail on iOS; the
 * Android Files drive is not built, so every computer opens straight to its desktop, as iOS does for
 * a computer without Files).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun AciComputersPane(host: AciHost, devices: List<AciDevice>?, onRefresh: suspend () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var pairOpen by remember { mutableStateOf(false) }
    var pickerOpen by remember { mutableStateOf(false) }
    var openComputer by remember { mutableStateOf<ComputerRecord?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    val workflowRuntime = devices?.firstOrNull { it.isOnline } ?: devices?.firstOrNull()

    suspend fun openDesktop(device: AciDevice) {
        val computers = (ComputersApi.list() as? ComputersApi.Result.Ok)?.value.orEmpty()
        val match = computers.firstOrNull { it.id == device.id }
            ?: computers.firstOrNull { it.name == device.displayName || (it.host != null && it.host == device.hostname) }
        if (match != null) openComputer = match else pickerOpen = true
    }

    PullToRefreshBox(isRefreshing = refreshing, onRefresh = {
        scope.launch {
            refreshing = true
            onRefresh()
            refreshing = false
        }
    }, modifier = Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 32.dp).testTag("aciComputers")) {
            AciViewHeader("Computers")
            AciWideButton(DesignR.drawable.ph_plus, "Add a computer", "aciAddComputer") { pairOpen = true }
            AciWideButton(DesignR.drawable.ph_desktop, "View a computer's screen", "aciViewScreen") { pickerOpen = true }
            when {
                devices == null -> Text("Loading computers…", color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().padding(vertical = 48.dp))
                devices.isEmpty() -> AciDashedCard {
                    Column(Modifier.padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textPrimary.copy(alpha = 0.4f), modifier = Modifier.size(40.dp).padding(bottom = 4.dp))
                        Spacer(Modifier.height(8.dp))
                        Text("No computers paired", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                        Spacer(Modifier.height(4.dp))
                        Text("Open Allternit Desktop on a computer signed in to this account, and it shows up here.", color = c.textTertiary, fontSize = 12.sp, textAlign = TextAlign.Center)
                    }
                }
                else -> devices.forEach { d ->
                    val online = d.status != "offline"
                    Column(
                        Modifier.fillMaxWidth().heightIn(min = 44.dp).clickable(onClickLabel = "Open desktop for ${d.displayName}") { scope.launch { openDesktop(d) } }
                            .padding(vertical = 16.dp).testTag("aciComputerRow"),
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Text(d.displayName, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                            Box(Modifier.size(8.dp).clip(CircleShape).background(if (online) AllternitStatic.StatusSuccess else c.textTertiary))
                            Text(if (online) "Online" else "Offline", color = c.textPrimary, fontSize = 14.sp)
                        }
                        Text(lastSeen(d.lastSeenAt), color = c.textSecondary, fontSize = 13.sp)
                    }
                    Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                }
            }
            Spacer(Modifier.height(24.dp))
            AciLinkedPhonesSection(host.signedIn)
            AciCloudBackupsSection(host.signedIn)
            Spacer(Modifier.height(24.dp))
            AciWorkflowsSection(host, workflowRuntime)
        }
    }
    if (pairOpen) AciPairComputerSheet { pairOpen = false }
    if (pickerOpen) AciComputerScreenSheet(onDismiss = { pickerOpen = false })
    openComputer?.let { computer ->
        Dialog(onDismissRequest = { openComputer = null }, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
            ComputerDriveScreen(computer, backTitle = "Computers", onBack = { openComputer = null })
        }
    }
}

/** "Last seen Oct 29, 2026, 3:04 PM", or the web's fallback line. */
internal fun lastSeen(raw: String?): String {
    val millis = raw?.let { runCatching { Instant.parse(it).toEpochMilli() }.getOrNull() } ?: return "Last seen time unavailable"
    return "Last seen " + DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(millis))
}

/** "Add a computer": hero, title, one line and the three numbered steps. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciPairComputerSheet(onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = c.bgPanel, shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp)) {
        Column(Modifier.verticalScroll(rememberScrollState()).testTag("aciPairSheet"), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Box {
                AciPromoHero(height = 176.dp, topRadius = 0.dp)
                Box(Modifier.padding(16.dp).size(44.dp).clip(CircleShape).background(c.bgPrimary).clickable(onClickLabel = "Close pairing instructions") { onDismiss() },
                    contentAlignment = Alignment.Center) { Icon(painterResource(DesignR.drawable.ph_x), null, tint = c.textSecondary, modifier = Modifier.size(18.dp)) }
            }
            Column(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Text("Add a computer", color = c.textPrimary, fontSize = 26.sp, fontWeight = FontWeight.SemiBold)
                Text("Connect a computer to open its desktop from your phone.", color = c.textSecondary, fontSize = 17.sp, lineHeight = 23.sp)
                Step(1, buildAnnotatedString { append("Install Allternit Desktop on the computer.") })
                Step(2, buildAnnotatedString {
                    append("Open it and sign in with the same account you use on this phone. Or open a terminal on the computer and run ")
                    withStyle(SpanStyle(fontFamily = FontFamily.Monospace, fontSize = 14.sp)) { append("ao fabric pair") }
                    append(".")
                })
                Step(3, buildAnnotatedString { append("Return to Computers on your phone. Tap your computer to open its live desktop.") })
            }
        }
    }
}

@Composable
private fun Step(n: Int, text: androidx.compose.ui.text.AnnotatedString) {
    val c = AllternitTheme.colors
    Row(Modifier.padding(start = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("$n.", color = c.textPrimary, fontSize = 16.sp, textAlign = TextAlign.End, modifier = Modifier.width(20.dp))
        Text(text, color = c.textPrimary, fontSize = 16.sp, lineHeight = 22.sp)
    }
}

/** "View a computer's screen": any computer (cloud, a bot's, a paired Mac), then its live screen. */
@Composable
internal fun AciComputerScreenSheet(onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    var computers by remember { mutableStateOf<List<ComputerRecord>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var driving by remember { mutableStateOf<ComputerRecord?>(null) }
    LaunchedEffect(attempt) {
        loading = true
        when (val r = ComputersApi.list()) {
            is ComputersApi.Result.Ok -> { computers = r.value; error = null }
            is ComputersApi.Result.Failed -> error = r.message
        }
        loading = false
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        val d = driving
        if (d != null) {
            ComputerDriveScreen(d, backTitle = "Computer", onBack = { driving = null })
        } else {
            Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("aciScreenPicker")) {
                DriveTopBar(backTitle = "Back", title = "Computer", onBack = onDismiss)
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
                    when {
                        loading && computers.isEmpty() -> CircularProgressIndicator(Modifier.padding(32.dp).size(24.dp), strokeWidth = 2.dp)
                        error != null -> FriendlyState(FriendlyStyle.Error, "Couldn't load computers", error.orEmpty(), Modifier.padding(24.dp),
                            actionTitle = "Retry", onAction = { attempt++ }) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = AllternitStatic.StatusError) }
                        computers.isEmpty() -> FriendlyState(FriendlyStyle.Empty, "No computers yet",
                            "Open Allternit Desktop on a computer signed in to this account, and it shows up here.", Modifier.padding(24.dp)) {
                            Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textSecondary)
                        }
                        else -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
                            Column(Modifier.clip(RoundedCornerShape(12.dp)).background(c.fillNeutral)) {
                                computers.forEachIndexed { i, comp ->
                                    Row(
                                        Modifier.fillMaxWidth().clickable { driving = comp }.padding(horizontal = 16.dp, vertical = 12.dp).testTag("aciScreenRow"),
                                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                                    ) {
                                        Box(Modifier.size(8.dp).clip(CircleShape).background(if (comp.isRunning) AllternitStatic.StatusSuccess else c.textTertiary))
                                        Column(Modifier.weight(1f)) {
                                            Text(comp.name, color = c.textPrimary, fontSize = 15.sp)
                                            Text("${comp.kindLabel} · ${comp.status.replaceFirstChar { it.uppercase() }}", color = c.textSecondary, fontSize = 12.5.sp)
                                        }
                                        Icon(painterResource(DesignR.drawable.ph_arrow_right), null, tint = c.textTertiary, modifier = Modifier.size(14.dp))
                                    }
                                    if (i < computers.lastIndex) Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** LinkedPhonesSection.tsx: phones paired to this account; signed out or none shows the setup line. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AciLinkedPhonesSection(signedIn: Boolean) {
    val c = AllternitTheme.colors
    var phones by remember { mutableStateOf<List<CloudPhone>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var callAudioFor by remember { mutableStateOf<CloudPhone?>(null) }
    LaunchedEffect(signedIn, attempt) {
        if (!signedIn) return@LaunchedEffect
        phones = null
        error = null
        when (val r = AciApi.phones()) {
            is AciApi.Result.Ok -> phones = r.value
            is AciApi.Result.Failed -> error = r.message.ifEmpty { "Couldn’t load your phones." }
        }
    }
    if (!signedIn || phones?.isEmpty() == true) {
        AciDashedCard(radius = 12.dp) {
            Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Icon(painterResource(DesignR.drawable.ph_identification_card), null, tint = c.textSecondary, modifier = Modifier.size(26.dp))
                Text("Live phone screens need Allternit Desktop with a phone connected there. Set the phone up from Allternit Desktop, then open it here again.",
                    color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp, modifier = Modifier.weight(1f))
            }
        }
        return
    }
    Column(Modifier.fillMaxWidth().testTag("aciPhones")) {
        Text("Phones", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 4.dp))
        val list = phones
        when {
            error != null -> Row(Modifier.padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(error.orEmpty(), color = AllternitStatic.StatusError, fontSize = 14.sp, modifier = Modifier.weight(1f))
                AciPillButton("Retry", "aciPhonesRetry") { attempt++ }
            }
            list == null -> Text("Loading phones…", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(vertical = 12.dp))
            else -> list.forEach { p ->
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f).heightIn(min = 44.dp).padding(vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(p.name, color = c.textPrimary, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                            Box(Modifier.size(8.dp).clip(CircleShape).background(if (p.isLive) AllternitStatic.StatusSuccess else c.textTertiary))
                            Text(
                                when {
                                    p.isAndroidApp -> if (p.online) (if (p.canTap == true) "Screen and taps" else "Screen only") else "Phone app offline"
                                    p.isKeyboard -> p.keyboardLabel
                                    p.isAppPhone -> if (p.healthyTapRoute) "Screen and taps" else "Screen"
                                    p.online -> "Online"
                                    else -> "Desktop offline"
                                },
                                color = c.textPrimary, fontSize = 14.sp,
                            )
                        }
                        val sub = when {
                            p.isAndroidApp && !p.online -> "Open the Allternit app on the phone"
                            p.isKeyboard -> "A Bluetooth keyboard and mouse, not another phone"
                            p.isAppPhone && !p.isLive -> "Open the Allternit app on your iPhone and tap Share screen"
                            p.model.isNotEmpty() -> p.model
                            else -> null
                        }
                        if (sub != null) Text(sub, color = c.textSecondary, fontSize = 13.sp)
                    }
                    // Call audio is the Android cable bridge; a linked iPhone has only its screen.
                    if (!p.isAndroidApp && !p.isIphone) {
                        AciPillButton("Call audio", "aciCallAudio", label = "Call audio for ${p.name}") { callAudioFor = p }
                        Spacer(Modifier.width(8.dp))
                    }
                }
                Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
            }
        }
    }
    callAudioFor?.let { p ->
        ModalBottomSheet(onDismissRequest = { callAudioFor = null }, containerColor = c.bgPanel) {
            Column(Modifier.padding(20.dp).testTag("aciCallAudioSheet"), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Text("Call audio · ${p.name}", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                val answers = when {
                    p.callBridgeEnabled == null -> "Shown in Allternit Desktop"
                    p.callBridgeEnabled == false -> "Off"
                    !p.callBridgeBot.isNullOrEmpty() -> "On · ${p.callBridgeBot}"
                    else -> "On"
                }
                Row {
                    Text("Bot answers calls", color = c.textTertiary, fontSize = 14.sp, modifier = Modifier.weight(1f))
                    Text(answers, color = c.textPrimary, fontSize = 14.sp)
                }
                Text("Changing it needs Allternit Desktop on the Mac the phone is wired to, because the audio devices are on that Mac.", color = c.textSecondary, fontSize = 14.sp)
                Spacer(Modifier.height(16.dp))
            }
        }
    }
}

/** CloudComputerBackups: hidden until the account has a cloud computer. */
@Composable
private fun AciCloudBackupsSection(signedIn: Boolean) {
    val c = AllternitTheme.colors
    var computers by remember { mutableStateOf<List<CloudComputerSummary>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(signedIn) {
        if (!signedIn) return@LaunchedEffect
        when (val r = AciApi.cloudComputers()) {
            is AciApi.Result.Ok -> computers = r.value
            is AciApi.Result.Failed -> error = r.message
        }
    }
    if (error == null && computers.isEmpty()) return
    Column(Modifier.fillMaxWidth().padding(top = 24.dp).testTag("aciBackups"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
        Text("Restore points and backups", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Text("Your cloud computer is also backed up off-site every night, encrypted.", color = c.textTertiary, fontSize = 12.sp)
        if (error != null) {
            Text(error.orEmpty(), color = AllternitStatic.StatusError, fontSize = 12.sp)
        } else computers.forEach { comp ->
            Row(Modifier.fillMaxWidth().heightIn(min = 44.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Text(comp.label, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f))
                Text(comp.status.replaceFirstChar { it.uppercase() }, color = c.textTertiary, fontSize = 12.sp)
            }
        }
    }
}

/** FabricWorkflowsPanel on the phone: compiled workflow specs on a paired computer, or why there are none. */
@Composable
private fun AciWorkflowsSection(host: AciHost, runtime: AciDevice?) {
    val c = AllternitTheme.colors
    var specs by remember { mutableStateOf<List<WorkflowSpec>>(emptyList()) }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    LaunchedEffect(runtime?.id, attempt) {
        if (runtime == null) { loading = false; return@LaunchedEffect }
        loading = true
        error = null
        when (val r = AciApi.workflowSpecs(host.backend)) {
            is AciApi.Result.Ok -> specs = r.value
            is AciApi.Result.Failed -> { specs = emptyList(); error = r.message.ifEmpty { "Could not load workflow specs." } }
        }
        loading = false
    }
    Column(Modifier.fillMaxWidth().border(1.dp, c.borderStrong, RoundedCornerShape(8.dp)).padding(16.dp).testTag("aciWorkflows"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("WORKFLOWS (RECORD → TEACH → BATCH → VERIFY)", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.4.sp)
        if (runtime == null) {
            Text("Workflows run on a paired computer running Allternit Desktop.", color = c.textSecondary, fontSize = 12.sp)
            return@Column
        }
        Text("On ${runtime.displayName}", color = c.textTertiary, fontSize = 11.sp)
        Column(Modifier.fillMaxWidth().border(1.dp, c.borderStrong, RoundedCornerShape(4.dp))) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("WORKFLOW SPECS", color = c.textTertiary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.4.sp, modifier = Modifier.weight(1f))
                Text("↻", color = c.textTertiary, fontSize = 15.sp, modifier = Modifier.clip(CircleShape).clickable(onClickLabel = "Refresh workflow specs") { attempt++ }.padding(8.dp))
            }
            Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderStrong))
            Column(Modifier.padding(12.dp)) {
                when {
                    loading -> Text("Loading…", color = c.textTertiary, fontSize = 12.sp)
                    error != null -> Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text(error.orEmpty(), color = c.textSecondary, fontSize = 12.sp)
                        Text("Try again", color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold,
                            modifier = Modifier.clip(CircleShape).background(c.bgPanel).border(1.dp, c.borderSubtle, CircleShape)
                                .clickable { attempt++ }.padding(horizontal = 14.dp, vertical = 7.dp).testTag("aciWorkflowsRetry"))
                    }
                    specs.isEmpty() -> Text("No compiled workflow specs on the gateway yet. Compile a recording first (POST /v1/browser-skills/from-recording).", color = c.textTertiary, fontSize = 12.sp)
                    else -> specs.forEach { s ->
                        Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(s.title ?: s.skillId, color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, modifier = Modifier.weight(1f))
                            if (s.hasNetworkTrace == true) Icon(painterResource(DesignR.drawable.ph_seal_check), "Has NetworkTrace", tint = AllternitStatic.StatusSuccess, modifier = Modifier.size(12.dp))
                            else Text("NO TRACE", color = c.textTertiary, fontSize = 10.sp, letterSpacing = 0.6.sp)
                        }
                        Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                    }
                }
            }
        }
    }
}
