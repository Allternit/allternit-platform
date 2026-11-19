package com.allternit.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private const val DOWNLOAD_URL = "https://www.allternit.com/download"
private const val PLANS_URL = "https://allternit.com/pricing"
private const val CREDITS_URL = "https://ai.allternit.com/settings/billing"

/**
 * The no-computer state in a chat (iOS ComputeGateView, and RuntimeRelayError.unreachable for a
 * computer that doesn't answer). Shown in the feed instead of a raw error, with a way to connect.
 * Plan chats stay first-class: the copy says Allternit Cloud models and own keys work without
 * a computer, and the composer keeps the draft.
 */
@Composable
internal fun ComputeGateCard(reason: ComputeGateReason, backend: ChatBackend, onConnected: () -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var linking by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    var checking by remember { mutableStateOf(false) }
    var sheet by remember { mutableStateOf(false) }

    // While linking, look for the new computer every 5 s (iOS ComputeGateView poll).
    LaunchedEffect(linking) {
        while (linking) {
            delay(5_000)
            if (backend.relay.resolve(force = true) is RelayPick.Computer) {
                linking = false
                onConnected()
            }
        }
    }

    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp)
            .clip(RoundedCornerShape(16.dp))
            .background(c.bgPanel)
            .border(1.dp, c.borderSubtle, RoundedCornerShape(16.dp))
            .padding(16.dp)
            .testTag("chatComputeGate"),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(painterResource(DesignR.drawable.ph_desktop), null, tint = c.textPrimary, modifier = Modifier.size(18.dp))
            Text(
                if (reason == ComputeGateReason.NoComputer) "Agents need a computer" else "Can't reach your computer",
                color = c.textPrimary,
                fontSize = 17.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f).semantics { heading() },
            )
            Box(
                Modifier.size(32.dp).clickable(role = Role.Button, onClickLabel = "Hide", onClick = onDismiss),
                contentAlignment = Alignment.Center,
            ) { Icon(painterResource(DesignR.drawable.ph_x), "Hide", tint = c.textSecondary, modifier = Modifier.size(12.dp)) }
        }
        if (reason == ComputeGateReason.Unreachable) {
            Text(ChatErrors.UNREACHABLE, color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                CapsuleButton(if (checking) "Checking…" else "Try again", primary = true, enabled = !checking, label = "Checks your computer again") {
                    checking = true
                    scope.launch {
                        val pick = backend.relay.resolve(force = true)
                        checking = false
                        if (pick is RelayPick.Computer) onConnected()
                    }
                }
                CapsuleButton("Connect a computer", icon = DesignR.drawable.ph_desktop) { sheet = true }
            }
        } else {
            Text("Pick where your agents run. Allternit Cloud models and your own API keys work without one.", color = c.textSecondary, fontSize = 14.sp, lineHeight = 19.sp)
            GateOption(
                title = "Use your own computer",
                cost = "Free. Link the Allternit Desktop app.",
                button = "Link my computer",
                testTag = "chatComputeGateLink",
                onClick = { linking = true },
            ) {
                if (linking) {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text("1. On your computer, open $DOWNLOAD_URL", color = c.textPrimary, fontSize = 13.sp)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CapsuleButton(if (copied) "Copied" else "Copy link", icon = DesignR.drawable.ph_copy) {
                                copyToClipboard(context, DOWNLOAD_URL)
                                copied = true
                            }
                            CapsuleButton("Share", icon = R.drawable.chat_share) { shareText(context, DOWNLOAD_URL) }
                        }
                        Text("2. Open Allternit and approve the link it opens in your browser.", color = c.textPrimary, fontSize = 13.sp)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = c.textSecondary)
                            Text("3. Waiting for your computer…", color = c.textSecondary, fontSize = 13.sp)
                        }
                    }
                }
            }
            GateOption(title = "Subscribe", cost = "Hosted computers run on Allternit servers. They sleep when idle.", button = "See plans", testTag = "chatComputeGatePlans", onClick = { openUrl(context, PLANS_URL) })
            GateOption(title = "Buy credits", cost = "Manage your plan on the web.", button = "Buy credits", testTag = "chatComputeGateCredits", onClick = { openUrl(context, CREDITS_URL) })
            Text(
                "Already have one? Pick a computer",
                color = c.accentPrimary,
                fontSize = 13.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clickable(role = Role.Button) { sheet = true }.padding(vertical = 4.dp),
            )
        }
    }
    if (sheet) {
        ConnectComputerSheet(backend, onDismiss = { sheet = false }, onPicked = onConnected)
    }
}

@Composable
private fun GateOption(title: String, cost: String, button: String, testTag: String, onClick: () -> Unit, extra: @Composable () -> Unit = {}) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgPrimary).border(1.dp, c.borderSubtle, RoundedCornerShape(12.dp)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
        Text(cost, color = c.textSecondary, fontSize = 13.sp)
        Box(Modifier.testTag(testTag)) { CapsuleButton(button, onClick = onClick) }
        extra()
    }
}

/**
 * Pick which computer chats run on (iOS ConnectComputerSheet): Automatic, then each computer
 * with its status. The pick persists under the same key iOS and the PWA use.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConnectComputerSheet(backend: ChatBackend, onDismiss: () -> Unit, onPicked: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var devices by remember { mutableStateOf<List<RelayDevice>?>(null) }
    var loading by remember { mutableStateOf(true) }
    var failed by remember { mutableStateOf(false) }
    var selected by remember { mutableStateOf(backend.relay.selectedRuntimeId) }
    suspend fun load() {
        loading = true
        val list = backend.relay.listDevices()
        failed = list == null
        devices = list
        loading = false
    }
    LaunchedEffect(Unit) { load() }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = state, containerColor = c.bgPrimary) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Connect a computer", color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f).semantics { heading() })
                Text("Refresh", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.clickable(role = Role.Button) { scope.launch { load() } }.padding(8.dp))
                Text("Done", color = c.accentPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable(role = Role.Button, onClick = onDismiss).padding(8.dp))
            }
            val active = backend.relay.activeRuntimeId
            val activeName = devices?.firstOrNull { it.id == active }?.name?.takeIf { it.isNotEmpty() }
            DeviceRow(
                title = "Automatic",
                subtitle = activeName?.let { "Using $it" } ?: "Your default computer, else the first online one",
                checked = selected == null,
            ) {
                selected = null
                backend.relay.selectedRuntimeId = null
                onPicked()
            }
            Text("Your computers", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp))
            when {
                loading && devices == null -> Text("Loading…", color = c.textSecondary, fontSize = 14.sp)
                failed -> Text("Couldn't load your computers", color = AllternitStatic.StatusError, fontSize = 14.sp)
                devices.isNullOrEmpty() -> Text("No computers yet.", color = c.textSecondary, fontSize = 14.sp)
                else -> devices.orEmpty().forEach { d ->
                    val status = when {
                        d.isOnline -> "Online"
                        d.wakeable -> "Asleep · wakes when used"
                        else -> "Offline"
                    }
                    DeviceRow(
                        title = d.name.ifEmpty { d.runtimeType.replaceFirstChar { it.uppercase() } },
                        subtitle = if (d.preferred) "$status · Default" else status,
                        checked = selected == d.id,
                    ) {
                        selected = d.id
                        backend.relay.selectedRuntimeId = d.id
                        onPicked()
                    }
                }
            }
            Text("From Allternit Desktop", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 6.dp))
            val context = LocalContext.current
            CapsuleButton("Get Allternit Desktop", icon = DesignR.drawable.ph_arrow_square_out) { openUrl(context, DOWNLOAD_URL) }
            Text("From a terminal", color = c.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
            Text("ao fabric pair", color = c.textPrimary, fontSize = 14.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.clip(RoundedCornerShape(8.dp)).background(c.fillNeutral).padding(10.dp))
            Spacer(Modifier.height(24.dp))
        }
    }
}

@Composable
private fun DeviceRow(title: String, subtitle: String, checked: Boolean, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Row(
        Modifier.fillMaxWidth().heightIn(min = 52.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).clickable(role = Role.Button, onClick = onClick).padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            Text(subtitle, color = c.textSecondary, fontSize = 12.sp)
        }
        if (checked) Icon(painterResource(DesignR.drawable.ph_check), "Selected", tint = c.accentPrimary, modifier = Modifier.size(16.dp))
    }
}
