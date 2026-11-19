package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiConfig
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.RuntimeRelay
import com.allternit.feature.shell.PhoneHeader
import com.allternit.feature.shell.PhoneIconButton
import com.allternit.feature.shell.ShellApi
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/** iOS ConnGroup: the order, the title and the one-line blurb of each Settings > Connections group. */
private val CONNECTION_GROUPS = listOf(
    Triple("brains", "Brains", "The AI that thinks. Use a plan you already pay for, or ours."),
    Triple("computers", "Computers", "Only needed when Allternit should click and type in apps for you."),
    Triple("phones", "Phones", "Talk to Allternit from your phone, and let it use your phone."),
    Triple("tools", "Coding tools", "Command-line agents. They run on a computer and use your plans."),
    Triple("voice", "Voice", "Who speaks when Allternit talks back."),
    Triple("apps", "Apps", "Mail, files and work apps Allternit can use for you."),
    Triple("channels", "Channels", "Where people can message your bots."),
)

/**
 * Settings > Connections (iOS ConnectionsSettingsView + ConnectionRowView + ConnectSheetView): the connect kit's
 * rows in seven groups, each with the same Connect sheet setup uses, and Disconnect for a row that is on.
 * It reuses [ConnectionsStore], [ConnectionRow] and [ConnectSheetHost] from first-run setup, never a second connect screen.
 */
@Composable
internal fun ConnectionsSettingsScreen(onBack: () -> Unit, openPhoneControl: () -> Unit, openAndroidScreen: () -> Unit) {
    val c = setupColors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val relay = remember { RuntimeRelay(ShellApi.okHttp, ApiConfig(), context) }
    val store = remember { ConnectionsStore() }
    var sheet by remember { mutableStateOf<ConnSheet?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) { store.load(relay) }

    fun reload() { scope.launch { store.load(relay) } }

    fun connect(id: String) {
        message = null
        when (id) {
            "phone-control" -> openPhoneControl()
            "phone", "android-screen" -> openAndroidScreen()
            "gizzi", "server", "cloud-computer" -> sheet = ConnSheet.OnComputer(id)
            "openai-voice", "elevenlabs" -> message = "Voice keys are set under Voice and dictation."
            else -> sheet = connectRoute(id)
        }
    }

    fun fallback(id: String) {
        sheet = when {
            id == "this-mac" -> ConnSheet.OnComputer("this-mac")
            id == "github" -> ConnSheet.Login("copilot")
            id.endsWith("-key") || id == "api-key" -> ConnSheet.Keys(if (id == "api-key") null else id)
            else -> null
        }
    }

    Column(Modifier.fillMaxSize().background(AllternitTheme.colors.bgPrimary).statusBarsPadding().navigationBarsPadding().testTag("settingsConnections")) {
        PhoneHeader(title = "Connections", leading = { PhoneIconButton(DesignR.drawable.ph_caret_left, "Back", testTag = "settingsBack", onClick = onBack) })
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 32.dp)) {
            if (store.loadError != null) Text(store.loadError ?: "", color = c.errFg, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp).testTag("connectionsError"))
            message?.let { Text(it, color = c.sub, fontSize = 13.sp, modifier = Modifier.padding(vertical = 6.dp)) }
            for ((group, title, blurb) in CONNECTION_GROUPS) {
                val entries = ConnectionsCatalog.entries.filter { it.group == group }
                if (entries.isEmpty()) continue
                Text(title, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(start = 6.dp, top = 18.dp, bottom = 2.dp))
                Text(blurb, color = c.mut, fontSize = 12.sp, lineHeight = 16.sp, modifier = Modifier.padding(start = 6.dp, bottom = 8.dp))
                Column(Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(12.dp)).testTag("connectionsGroup_$group")) {
                    entries.forEachIndexed { i, e ->
                        if (i > 0) HorizontalDivider(thickness = 0.5.dp, color = c.line)
                        val on = store.state(e.id) == ConnState.On && e.way != ConnWay.Included
                        ConnectionRow(
                            entry = e,
                            store = store,
                            tag = null,
                            trailing = if (on) ({
                                SetupQuietButton("Disconnect") {
                                    scope.launch {
                                        runCatching { ConnectionsClient.disconnect(e.id, store.target(e.id)) }
                                            .onFailure { message = ConnectionsClient.describe(it) }
                                        store.load(relay)
                                    }
                                }
                            }) else null,
                            onConnect = { connect(e.id) },
                            onFallback = ::fallback,
                        )
                    }
                }
            }
        }
    }

    sheet?.let { s ->
        ConnectSheetHost(
            sheet = s,
            store = store,
            onConnected = { sheet = null; reload() },
            onFallback = ::fallback,
            onDismiss = { sheet = null; reload() },
        )
    }
}
