package com.allternit.feature.shell.firstrun

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.time.Duration
import java.time.Instant
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// The connect sheets setup opens from a connect-kit row (iOS ConnectSheetView): an API key,
// a plan sign-in with a code, and the "runs on your computer" note.

/** What a row's Connect opens (iOS ConnSheet / ConnPage). */
internal sealed interface ConnSheet {
    /** "An API key" (all providers) or one provider's key. */
    data class Keys(val only: String?) : ConnSheet
    /** A plan sign-in (device code, link paste or OAuth) through `/connect`. */
    data class Login(val id: String) : ConnSheet
    /** Rows that run on the person's own computer. */
    data class OnComputer(val id: String) : ConnSheet
}

/** iOS ConnRoute.forConnect for the brain rows setup shows. */
internal fun connectRoute(id: String): ConnSheet = when {
    id == "api-key" -> ConnSheet.Keys(null)
    id == "gemini" -> ConnSheet.Keys("google-key")
    id.endsWith("-key") -> ConnSheet.Keys(id)
    id == "local" || id == "this-mac" -> ConnSheet.OnComputer(id)
    else -> ConnSheet.Login(id)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
internal fun ConnectSheetHost(
    sheet: ConnSheet,
    store: ConnectionsStore,
    onConnected: (String) -> Unit,
    onFallback: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val c = setupColors
    val state = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = state,
        containerColor = c.bg,
        shape = RoundedCornerShape(topStart = 28.dp, topEnd = 28.dp),
    ) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(bottom = 20.dp).navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            when (sheet) {
                is ConnSheet.Keys -> KeysSheet(sheet.only, store, onConnected)
                is ConnSheet.Login -> LoginSheet(sheet.id, store, onConnected, onFallback)
                is ConnSheet.OnComputer -> OnComputerSheet(sheet.id)
            }
        }
    }
}

@Composable
private fun SheetTitle(text: String) {
    Text(text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, color = setupColors.ink)
}

@Composable
private fun SheetNote(text: String) {
    Text(text, fontSize = 13.5.sp, color = setupColors.sub)
}

@Composable
private fun KeysSheet(only: String?, store: ConnectionsStore, onConnected: (String) -> Unit) {
    val providers = only?.let { listOfNotNull(ConnectionsCatalog.keyProvider(it)) } ?: ConnectionsCatalog.keyProviders
    var expanded by remember { mutableStateOf(only) }
    SheetTitle(if (only == null) "An API key" else "${providers.firstOrNull()?.name ?: "API"} key")
    // Real state only: one provider's own row, or "any row" when the sheet lists them all.
    val rows = providers.map { store.row(it.id).state }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        ConnectScene(
            when {
                ConnState.Connecting in rows -> ConnectSceneState.Connecting
                ConnState.On in rows -> ConnectSceneState.Connected
                else -> ConnectSceneState.Asleep
            },
        )
    }
    SheetNote("Open the key page, copy a key, then come back and paste it. We check it with the vendor before it's saved, and it works on any device.")
    providers.forEach { p ->
        KeyRow(p, store, expanded == p.id, onToggle = { expanded = if (expanded == p.id) null else p.id }, onConnected = onConnected)
    }
}

@Composable
private fun KeyRow(p: ConnKeyProvider, store: ConnectionsStore, open: Boolean, onToggle: () -> Unit, onConnected: (String) -> Unit) {
    val c = setupColors
    val row = store.row(p.id)
    val uri = LocalUriHandler.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf("") }
    var checking by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val status = when (row.state) {
        ConnState.On -> row.found?.let { "Connected · $it" } ?: "Connected"
        ConnState.Needs -> "Needs you · ${row.reason ?: ConnectionsCatalog.reasonText(row.reasonCode, p.name) ?: "The key was rejected"}"
        ConnState.Connecting -> "Connecting…"
        ConnState.Off -> "Not connected"
    }
    fun submit(raw: String) {
        val key = raw.trim()
        if (key.isEmpty()) {
            error = "The clipboard has no text. Copy your ${p.name} key and try again."
            return
        }
        error = null
        checking = true
        store.setConnecting(p.id, true)
        scope.launch {
            try {
                val fresh = ConnectionsClient.submit(p.id, null, key)
                if (fresh != null) store.apply(fresh)
                when (fresh?.state) {
                    ConnState.On -> { draft = ""; onConnected(p.id) }
                    ConnState.Needs -> error = "${p.name} said no: ${(fresh.reason ?: ConnectionsCatalog.reasonText(fresh.reasonCode, p.name) ?: "the key was rejected").lowercase()}. Check the key and paste it again."
                    else -> {}
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = ConnectionsClient.describe(e)
            } finally {
                checking = false
                store.setConnecting(p.id, false)
            }
        }
    }
    Column(
        Modifier.fillMaxWidth().background(c.fill, RoundedCornerShape(14.dp)).testTag("conn-key-${p.id}"),
    ) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 52.dp).clickable(onClick = onToggle).padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text(p.name, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
                Text(status, fontSize = 12.5.sp, color = if (row.state == ConnState.Needs) c.errFg else c.sub)
            }
            Text(if (open) "▾" else "›", fontSize = 14.sp, color = c.mut)
        }
        if (open) {
            Column(Modifier.padding(start = 12.dp, end = 12.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (checking) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
                        Text("Checking your key with ${p.name}…", fontSize = 13.5.sp, color = c.sub)
                    }
                } else {
                    BasicTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        singleLine = true,
                        textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 14.sp, color = c.ink),
                        cursorBrush = SolidColor(c.ink),
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                        modifier = Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(10.dp))
                            .border(1.dp, c.line, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 12.dp),
                        decorationBox = { inner ->
                            Box {
                                if (draft.isEmpty()) Text("Paste your ${p.name} key", fontSize = 14.sp, color = c.mut)
                                inner()
                            }
                        },
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        if (draft.isBlank()) {
                            SetupQuietButton("Paste") {
                                val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                                submit(cm?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString().orEmpty())
                            }
                        } else {
                            SetupQuietButton("Save") { submit(draft) }
                        }
                        SetupQuietButton("Open the key page") { runCatching { uri.openUri(p.keyPage) } }
                    }
                }
                error?.let { Text(it, fontSize = 12.5.sp, color = c.errFg) }
            }
        }
    }
}

private sealed interface LoginPhase {
    data object Starting : LoginPhase
    data class Vendor(val login: ConnLogin) : LoginPhase
    data object Done : LoginPhase
    data object Expired : LoginPhase
    data class Failed(val message: String) : LoginPhase
    data class Disclosure(val disclosure: ConnDisclosure) : LoginPhase
}

@Composable
private fun LoginSheet(id: String, store: ConnectionsStore, onConnected: (String) -> Unit, onFallback: (String) -> Unit) {
    val c = setupColors
    val entry = ConnectionsCatalog.entry(id)
    // The Claude organization link signs in on Claude's own page (iOS ConnectSheetView vendor).
    val claudeOrg = id == "claude-org"
    val vendor = if (claudeOrg) "Claude" else entry?.name ?: id
    // Read before the sheet marks the row connecting: "Link again" for a row that needs you.
    val relinkReason = remember {
        val r = store.row(id)
        if (r.state == ConnState.Needs) r.reason ?: ConnectionsCatalog.reasonText(r.reasonCode, vendor) ?: "" else null
    }
    val uri = LocalUriHandler.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var phase by remember { mutableStateOf<LoginPhase>(LoginPhase.Starting) }
    var accepted by remember { mutableStateOf<String?>(null) }
    var attempt by remember { mutableIntStateOf(0) }
    var opened by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf("") }
    var submitting by remember { mutableStateOf(false) }
    var now by remember { mutableStateOf(Instant.now()) }

    LaunchedEffect(attempt) {
        phase = LoginPhase.Starting
        opened = false
        copied = false
        store.setConnecting(id, true)
        // Computer rows connect on that computer; the rest on cloud-api (iOS store.target(for:)).
        val target = store.target(id)
        try {
            val (state, login) = ConnectionsClient.connect(id, target, accepted, way = if (claudeOrg) "oauth" else "code")
            if (state == ConnState.On && login == null) {
                store.markOn(id)
                phase = LoginPhase.Done
                onConnected(id)
                return@LaunchedEffect
            }
            if (login == null) {
                phase = LoginPhase.Failed("Could not start. Try again")
                return@LaunchedEffect
            }
            phase = LoginPhase.Vendor(login)
            if (!login.poll) return@LaunchedEffect
            while (true) {
                delay(2000)
                now = Instant.now()
                // A poll can fail while the person is in the browser; keep polling until the code expires.
                val poll = try {
                    ConnectionsClient.login(id, target)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    if (login.expiry?.isBefore(Instant.now()) == true) { phase = LoginPhase.Expired; return@LaunchedEffect }
                    continue
                }
                when (poll.first) {
                    "pending" -> continue
                    "done" -> {
                        poll.second?.let(store::apply) ?: store.markOn(id)
                        phase = LoginPhase.Done
                        onConnected(id)
                        return@LaunchedEffect
                    }
                    "expired" -> { phase = LoginPhase.Expired; return@LaunchedEffect }
                    else -> { phase = LoginPhase.Failed("The sign-in didn't finish. Try again."); return@LaunchedEffect }
                }
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: DisclosureRequired) {
            phase = LoginPhase.Disclosure(e.disclosure)
        } catch (e: Exception) {
            phase = LoginPhase.Failed(ConnectionsClient.describe(e))
        } finally {
            if (phase != LoginPhase.Done) store.setConnecting(id, false)
        }
    }

    fun open(login: ConnLogin) {
        if (login.kind == "device_code" && login.urlWithCode == null && login.code != null) {
            copy(context, login.code)
            copied = true
        }
        login.openUrl?.let { runCatching { uri.openUri(it) } }
        opened = true
    }

    fun submitCode(raw: String) {
        val value = raw.trim()
        if (value.isEmpty()) return
        submitting = true
        scope.launch {
            try {
                val row = ConnectionsClient.submit(id, store.target(id), value)
                if (row != null) store.apply(row)
                if (row?.state == ConnState.On) { phase = LoginPhase.Done; onConnected(id) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: DisclosureRequired) {
                phase = LoginPhase.Disclosure(e.disclosure)
            } catch (e: Exception) {
                phase = LoginPhase.Failed(ConnectionsClient.describe(e))
            } finally {
                submitting = false
            }
        }
    }

    SheetTitle(if (claudeOrg) (if (relinkReason != null) "Link again" else "Link your Claude organization") else "Connect $vendor")
    if (claudeOrg) {
        relinkReason?.takeIf { it.isNotEmpty() }?.let { Text("$it.", fontSize = 13.sp, color = c.ink, modifier = Modifier.testTag("conn-relink-reason")) }
        if (phase !is LoginPhase.Disclosure) {
            SheetNote("Chats you send on this model are billed to your Claude organization, not to Allternit credits. ${ConnectionsCatalog.CLAUDE_ORG_NOTE} Allternit never sees your password.")
        }
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { ConnectScene(store.row(id).state.sceneState()) }
    when (val p = phase) {
        LoginPhase.Starting -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
            Text(if (claudeOrg) "Getting your link ready…" else "Getting your $vendor sign-in ready…", fontSize = 13.5.sp, color = c.sub)
        }
        is LoginPhase.Disclosure -> Column(Modifier.testTag("conn-disclosure"), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(p.disclosure.title, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, color = c.ink)
            Column(Modifier.fillMaxWidth().background(c.fill, RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                p.disclosure.body.forEach { line ->
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box(Modifier.padding(top = 8.dp).size(4.dp).background(c.mut, CircleShape))
                        Text(line, fontSize = 14.sp, color = c.ink)
                    }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SetupPrimaryButton(p.disclosure.acceptLabel, Modifier.testTag("conn-disclosure-accept")) {
                    accepted = p.disclosure.version
                    attempt += 1
                }
                SetupQuietButton(p.disclosure.declineLabel, Modifier.testTag("conn-disclosure-decline")) {
                    onFallback(ConnectionsCatalog.keyFallback(id))
                }
            }
        }
        is LoginPhase.Vendor -> {
            val login = p.login
            Column(Modifier.fillMaxWidth().background(c.fill, RoundedCornerShape(14.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                if (login.kind == "link_paste") {
                    if (!opened) {
                        SheetNote("$vendor opens in your browser. Approve, then tap Copy on the code $vendor shows. Come back here and paste it.")
                        SetupPrimaryButton("Open $vendor", enabled = login.openUrl != null) { open(login) }
                    } else if (submitting) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
                            Text("Checking the code…", fontSize = 13.5.sp, color = c.sub)
                        }
                    } else {
                        BasicTextField(
                            value = code, onValueChange = { code = it }, singleLine = true,
                            textStyle = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 14.sp, color = c.ink),
                            cursorBrush = SolidColor(c.ink),
                            modifier = Modifier.fillMaxWidth().background(c.card, RoundedCornerShape(10.dp))
                                .border(1.dp, c.line, RoundedCornerShape(10.dp)).padding(12.dp),
                            decorationBox = { inner -> Box { if (code.isEmpty()) Text("Paste the code", fontSize = 14.sp, color = c.mut); inner() } },
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            SetupQuietButton(if (code.isBlank()) "Paste" else "Done") {
                                if (code.isBlank()) {
                                    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                                    submitCode(cm?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString().orEmpty())
                                } else submitCode(code)
                            }
                            SetupQuietButton("Open $vendor again") { open(login) }
                        }
                    }
                } else {
                    val prefilled = login.urlWithCode != null
                    login.code?.let { value ->
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(value, fontFamily = FontFamily.Monospace, fontSize = 22.sp, fontWeight = FontWeight.SemiBold, color = c.ink,
                                letterSpacing = 3.sp, modifier = Modifier.background(c.bg, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 6.dp))
                            SetupQuietButton(if (copied) "Copied" else "Copy") { copy(context, value); copied = true }
                        }
                        SheetNote(if (prefilled) "$vendor opens with your code filled in. Check it shows $value and tap Confirm."
                        else "We copy the code when you tap Open. Paste it on $vendor's page.")
                    }
                    if (opened) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp, color = c.sub)
                            Text("Waiting for you on $vendor… this finishes by itself", fontSize = 13.sp, color = c.sub)
                        }
                        SetupQuietButton("Open $vendor again") { open(login) }
                    } else {
                        SetupPrimaryButton("Open $vendor", enabled = login.openUrl != null) { open(login) }
                    }
                }
                login.expiry?.let { expiry ->
                    val left = maxOf(0L, Duration.between(now, expiry).seconds)
                    Text("Code works for ${left / 60}:${"%02d".format(left % 60)}", fontSize = 12.sp, color = c.mut)
                }
            }
        }
        LoginPhase.Done -> Text("Connected.", fontSize = 14.sp, color = c.okFg)
        LoginPhase.Expired -> Retry("The code expired.", id, { attempt += 1 }, onFallback)
        is LoginPhase.Failed -> Retry(p.message, id, { attempt += 1 }, onFallback)
    }
}

@Composable
private fun Retry(message: String, id: String, onRetry: () -> Unit, onFallback: (String) -> Unit) {
    Text(message, fontSize = 13.5.sp, color = setupColors.errFg)
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        SetupPrimaryButton("Try again", onClick = onRetry)
        if (id in ConnectionsCatalog.runnerIds || id == "chatgpt") {
            SetupQuietButton("Use a key instead") { onFallback(ConnectionsCatalog.keyFallback(id)) }
        } else if (id != "claude-org") {
            SetupQuietButton("Add a computer") { onFallback("this-mac") }
        }
    }
}

@Composable
private fun OnComputerSheet(id: String) {
    val uri = LocalUriHandler.current
    val name = ConnectionsCatalog.entry(id)?.name ?: id
    if (id == "this-mac") {
        SheetTitle("Add a computer")
        SheetNote("Only when a task needs one: clicking apps, using a browser. Chat works without it. Install Allternit Desktop on your Mac or PC and sign in with this account; it pairs by itself and shows here.")
    } else {
        SheetTitle(name)
        SheetNote("$name runs on your own computer. Open Allternit on your Mac or PC and set it up in Settings › Connections there. It shows here as soon as it's connected.")
    }
    SetupPrimaryButton("Get Allternit Desktop") { runCatching { uri.openUri("https://allternit.com/download") } }
}

private fun copy(context: Context, text: String) {
    val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager ?: return
    cm.setPrimaryClip(ClipData.newPlainText("code", text))
}

// Buttons (iOS SetupPrimaryButtonStyle, SetupGhostButtonStyle, ConnQuietButtonStyle)

@Composable
internal fun SetupPrimaryButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val c = setupColors
    Box(
        modifier.heightIn(min = 44.dp).background(if (enabled) c.ink else c.mut.copy(alpha = 0.6f), RoundedCornerShape(12.dp))
            .clickable(enabled = enabled, onClick = onClick).padding(horizontal = 18.dp),
        contentAlignment = Alignment.Center,
    ) { Text(text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, color = c.bg) }
}

@Composable
internal fun SetupGhostButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val c = setupColors
    Box(
        modifier.heightIn(min = 44.dp).clickable(enabled = enabled, onClick = onClick).padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) { Text(text, fontSize = 15.sp, fontWeight = FontWeight.Medium, color = if (enabled) c.sub else c.mut.copy(alpha = 0.5f)) }
}

@Composable
internal fun SetupQuietButton(text: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val c = setupColors
    Box(
        modifier.heightIn(min = 36.dp).background(c.fill2, CircleShape).clickable(onClick = onClick).padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) { Text(text, fontSize = 14.sp, fontWeight = FontWeight.Medium, color = c.ink, maxLines = 1) }
}

