package com.allternit.feature.system

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.OpenableColumns
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.ThemeMode
import com.allternit.feature.chat.ChatBackends
import com.allternit.feature.chat.RuntimeCall
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * The Android share target (iOS `AllternitShare/ShareViewController`): text, links, images and files shared from any app
 * (ACTION_SEND and ACTION_SEND_MULTIPLE) open "Share to a bot". Pick a bot (the roster of the stored status snapshot, like
 * iOS), add a note, confirm once per bot, and it sends: files go through the chat uploads ([ChatBackend.upload], the same
 * call chat attachments use), then one message carries the note, the text or link and a link per file (iOS
 * `ShareOutboxDrainer.message`). Never replaces the "Save to computer" target of the Files drive (ShareToComputerActivity).
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        AllternitAuth.start(applicationContext)
        val shared = SharedItems.read(this, intent)
        setContent {
            AllternitTheme(themeMode = ThemeMode.Light) {
                ShareToBotScreen(shared, onFinish = { finish() }, onOpenApp = {
                    startActivity(packageManager.getLaunchIntentForPackage(packageName))
                    finish()
                })
            }
        }
    }
}

/** What the share carried (iOS `ShareModel` providers, loaded). */
internal class SharedItems(val text: String?, val url: String?, val streams: List<Uri>) {
    val isEmpty: Boolean get() = text.isNullOrBlank() && url.isNullOrBlank() && streams.isEmpty()

    companion object {
        @Suppress("DEPRECATION")
        fun read(context: Context, intent: Intent): SharedItems {
            val raw = intent.getStringExtra(Intent.EXTRA_TEXT)?.trim()?.takeIf { it.isNotEmpty() }
            val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)?.trim()?.takeIf { it.isNotEmpty() }
            val isLink = raw != null && (raw.startsWith("http://") || raw.startsWith("https://")) && !raw.contains(' ') && !raw.contains('\n')
            val streams = buildList {
                if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
                    val list = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                    else intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
                    list?.let { addAll(it) }
                } else {
                    val one = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                    else intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                    one?.let { add(it) }
                }
            }
            val text = if (isLink) subject else listOfNotNull(subject, raw).joinToString("\n\n").takeIf { it.isNotEmpty() }
            return SharedItems(text, if (isLink) raw else null, streams)
        }
    }
}

private sealed interface SharePhase {
    data object Picking : SharePhase
    data class Consent(val bot: StatusSnapshot.Bot) : SharePhase
    data object Saving : SharePhase
    data class Failed(val message: String) : SharePhase
    data object Sent : SharePhase
}

@Composable
private fun ShareToBotScreen(shared: SharedItems, onFinish: () -> Unit, onOpenApp: () -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    val authState by AllternitAuth.state.collectAsState()
    var bots by remember { mutableStateOf(StatusSync.snapshot(context)?.bots.orEmpty()) }
    var selected by remember { mutableStateOf<StatusSnapshot.Bot?>(null) }
    var note by remember { mutableStateOf("") }
    var phase by remember { mutableStateOf<SharePhase>(SharePhase.Picking) }
    LaunchedEffect(authState) {
        if (authState is AuthState.SignedIn) StatusSync.refresh(context)?.let { bots = it.bots }
    }
    fun send(bot: StatusSnapshot.Bot) {
        phase = SharePhase.Saving
        scope.launch {
            phase = withContext(Dispatchers.IO) { deliver(context, bot, note, shared) }
        }
    }
    Box(Modifier.fillMaxSize().background(c.bgPrimary).padding(20.dp).testTag("share-root")) {
        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text("Share to a bot")
            Text(summary(shared), color = c.textSecondary)
            when {
                shared.isEmpty -> {
                    Text("Nothing to share here. Share text, a link, a photo or a file.", color = c.textSecondary)
                    TextButton(onClick = onFinish) { Text("Close") }
                }
                authState !is AuthState.SignedIn -> {
                    Text("Open Allternit to sign in, then share again.", color = c.textSecondary)
                    Button(onClick = onOpenApp) { Text("Open Allternit") }
                }
                else -> when (val p = phase) {
                    SharePhase.Picking -> {
                        if (bots.isEmpty()) {
                            Text("No bots yet. Open Allternit to set up your first bot.", color = c.textSecondary)
                            Button(onClick = onOpenApp) { Text("Open Allternit") }
                        } else {
                            LazyColumn(Modifier.fillMaxWidth().height(220.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                items(bots, key = { it.botId }) { bot ->
                                    Row(
                                        Modifier.fillMaxWidth().background(if (selected?.botId == bot.botId) c.bgTertiary else c.bgPrimary, RoundedCornerShape(12.dp))
                                            .clickable { selected = bot }.padding(12.dp).testTag("share-bot-${bot.botId}"),
                                        verticalAlignment = Alignment.CenterVertically,
                                    ) { Text(bot.name.ifBlank { "Bot" }) }
                                }
                            }
                            OutlinedTextField(note, { note = it }, Modifier.fillMaxWidth().testTag("share-note"), label = { Text("Add a note") })
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                TextButton(onClick = onFinish) { Text("Cancel") }
                                Button(
                                    enabled = selected != null,
                                    onClick = {
                                        val bot = selected ?: return@Button
                                        if (ShareConsent.has(context, bot.botId)) send(bot) else phase = SharePhase.Consent(bot)
                                    },
                                    modifier = Modifier.testTag("share-send"),
                                ) { Text("Send") }
                            }
                        }
                    }
                    is SharePhase.Consent -> {
                        Text("This goes to ${p.bot.name.ifBlank { "the bot" }} and the model it runs on. Send it?", color = c.textSecondary)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(onClick = { phase = SharePhase.Picking }) { Text("Back") }
                            Button(onClick = { ShareConsent.record(context, p.bot.botId); send(p.bot) }) { Text("Send") }
                        }
                    }
                    SharePhase.Saving -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        Text("Sending")
                    }
                    is SharePhase.Failed -> {
                        Text(p.message, color = c.textSecondary)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(onClick = onFinish) { Text("Close") }
                            Button(onClick = { phase = SharePhase.Picking }) { Text("Try again") }
                        }
                    }
                    SharePhase.Sent -> {
                        Text("Sent to ${selected?.name?.ifBlank { null } ?: "your bot"}.")
                        Button(onClick = {
                            val bot = selected
                            context.startActivity(SystemLinks.intent(context, if (bot != null) SystemLinks.botChat(bot.botId) else SystemLinks.ASK_MAIN))
                            onFinish()
                        }) { Text("Open chat") }
                        TextButton(onClick = onFinish) { Text("Done") }
                    }
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

private fun summary(shared: SharedItems): String {
    val parts = mutableListOf<String>()
    shared.url?.let { parts += it }
    shared.text?.let { parts += it.take(160) }
    if (shared.streams.isNotEmpty()) parts += if (shared.streams.size == 1) "1 file" else "${shared.streams.size} files"
    return parts.joinToString("\n")
}

/** One-time consent per bot (iOS `OutboxStore.hasConsent` / `recordConsent`). */
private object ShareConsent {
    private fun prefs(context: Context) = context.getSharedPreferences("allternit_share", Context.MODE_PRIVATE)
    fun has(context: Context, botId: String) = prefs(context).getBoolean("consent-$botId", false)
    fun record(context: Context, botId: String) = prefs(context).edit().putBoolean("consent-$botId", true).apply()
}

private const val MAX_FILE_BYTES = 25 * 1024 * 1024

/** Uploads the files, opens a chat with the bot and posts the message (iOS ShareOutboxDrainer.send + message). */
private suspend fun deliver(context: Context, bot: StatusSnapshot.Bot, note: String, shared: SharedItems): SharePhase {
    val app = context.applicationContext
    if (AllternitAuth.tokenProvider.bearerToken().isNullOrBlank()) return SharePhase.Failed("Open Allternit to sign in, then try again.")
    val backend = ChatBackends.create(app)
    val links = mutableListOf<String>()
    for (uri in shared.streams) {
        val resolver = app.contentResolver
        val name = displayName(resolver, uri)
        val bytes = runCatching { resolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            ?: return SharePhase.Failed("Couldn't read $name.")
        if (bytes.size > MAX_FILE_BYTES) return SharePhase.Failed("$name is larger than 25 MB.")
        when (val up = backend.upload(name, resolver.getType(uri) ?: "application/octet-stream", bytes)) {
            is ApiResult.Ok -> links += "[$name](${up.value})"
            else -> return SharePhase.Failed("Couldn't upload $name. Check the connection and try again.")
        }
    }
    val message = listOfNotNull(note.trim().takeIf { it.isNotEmpty() }, shared.text, shared.url, links.takeIf { it.isNotEmpty() }?.joinToString("\n"))
        .joinToString("\n\n")
    val session = when (val r = backend.createSession("Shared from phone", "chat", bot.botId, bot.name.ifBlank { null }, null, null, "agent")) {
        is ApiResult.Ok -> r.value.id
        else -> return SharePhase.Failed("Couldn't open a chat with ${bot.name.ifBlank { "the bot" }}.")
    }
    val body = JSONObject().put("text", message)
        .put("metadata", JSONObject().put("time_zone", java.util.TimeZone.getDefault().id).put("source", "share")).toString()
    return when (val r = backend.runtimeJson("POST", "/api/v1/agent-sessions/$session/messages", body)) {
        is RuntimeCall.Ok -> SharePhase.Sent
        RuntimeCall.NoComputer -> SharePhase.Failed("Pair a computer first. Your bots run on a computer.")
        is RuntimeCall.Http -> SharePhase.Failed("That didn't go through (${r.status}). Try again.")
        is RuntimeCall.Failed -> SharePhase.Failed(r.message)
    }
}

private fun displayName(resolver: ContentResolver, uri: Uri): String {
    val fromProvider = runCatching {
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull()
    return fromProvider?.takeIf { it.isNotBlank() } ?: uri.lastPathSegment ?: "file"
}
