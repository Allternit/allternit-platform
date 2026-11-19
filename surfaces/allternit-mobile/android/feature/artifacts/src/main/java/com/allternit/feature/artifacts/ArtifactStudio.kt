package com.allternit.feature.artifacts

import android.content.Context
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.delay
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject

/** A studio session (iOS ArtifactStudioSession): the agent session plus what its editor is bound to. */
data class StudioSession(
    val sessionId: String,
    val kind: StudioKind,
    val title: String,
    val artifactId: String? = null,
    val designProjectId: String? = null,
) {
    /** iOS editorRoute: Office editor for Docs/Sheets/Slides, the Design canvas, or the bound artifact's window. */
    fun editorUrl(dark: Boolean): String? = when {
        artifactId != null && kind in setOf(StudioKind.Docs, StudioKind.Sheets, StudioKind.Slides) -> ArtifactWeb.officeUrl(kind.raw, artifactId, dark)
        designProjectId != null -> ArtifactWeb.designUrl(designProjectId, title, fromProject = false, dark = dark)
        artifactId != null -> ArtifactWeb.windowUrl(artifactId, dark)
        else -> null
    }
}

/**
 * iOS ArtifactStudio.launch: Docs/Sheets/Slides start on a blank Office record (`POST artifacts`,
 * tag `office:<kind>`), Design on a new canvas id, then `POST agent-sessions` (cowork surface,
 * agent mode, the kind's mode id) and `PATCH agent-sessions/{id}` with `metadata.artifactSession`.
 * A failed record or patch still opens the session, unbound, as on iOS.
 */
object ArtifactStudioLauncher {
    private const val PREFS = "allternit_artifact_studio"

    suspend fun launch(context: Context, kind: StudioKind): Result<StudioSession> {
        val title = kind.newTitle
        var artifactId: String? = null
        if (kind in setOf(StudioKind.Docs, StudioKind.Sheets, StudioKind.Slides)) {
            val r = LibraryNet.send("POST", "artifacts", buildJsonObject {
                put("workspace_id", "default"); put("title", title); put("type", "document")
                putJsonArray("tags") { add(kotlinx.serialization.json.JsonPrimitive("office:${kind.raw}")) }
                putJsonArray("sections") {}
            })
            artifactId = (r as? ApiResult.Ok)?.value.obj.child("artifact").str("id")
        }
        val designProjectId = if (kind == StudioKind.Design) java.util.UUID.randomUUID().toString().lowercase() else null
        val created = LibraryNet.send("POST", "agent-sessions", buildJsonObject {
            put("name", title); put("origin_surface", "cowork"); put("session_mode", "agent")
            kind.modeId?.let { m -> putJsonObject("metadata") { put("agentModeId", m) } }
        })
        val sessionObj = (created as? ApiResult.Ok)?.value.obj
        val sessionId = sessionObj.str("id") ?: sessionObj.child("session").str("id")
            ?: return Result.failure(IllegalStateException(created.friendly("start a session")))
        LibraryNet.send("PATCH", "agent-sessions/${LibraryNet.escape(sessionId)}", buildJsonObject {
            putJsonObject("metadata") {
                putJsonObject("artifactSession") {
                    put("kind", kind.raw); put("title", title)
                    artifactId?.let { put("artifactId", it) }
                    designProjectId?.let { put("designProjectId", it) }
                }
            }
        })
        // iOS keeps the session's standing instructions on the device (artifactStudio.sessions.v1).
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(sessionId, buildJsonObject { put("kind", kind.raw); put("title", title); put("startedAt", System.currentTimeMillis()) }.toString())
            .apply()
        return Result.success(StudioSession(sessionId, kind, title, artifactId, designProjectId))
    }
}

private enum class StudioPane { Chat, Edit }

/**
 * iOS ArtifactStudioView: close, the serif title and kind, a Chat | Edit segment. Chat is the
 * session's conversation ([chatPane], the shell's chat screen); Edit is the bound editor in the
 * live web view, or the session's newest result, or the empty state until Allternit saves one.
 * Results refresh every 4 s while Edit is open and every 8 s otherwise.
 */
@Composable
fun ArtifactStudioScreen(
    session: StudioSession,
    chatPane: @Composable (sessionId: String) -> Unit,
    onClose: () -> Unit,
    onOpenArtifact: (String) -> Unit,
) {
    val c = AllternitTheme.colors
    val dark = c.bgPrimary.luminance() < 0.5f
    var pane by remember { mutableStateOf(if (session.editorUrl(dark) != null) StudioPane.Edit else StudioPane.Chat) }
    var results by remember { mutableStateOf<List<ArtifactSummary>>(emptyList()) }
    BackHandler(onBack = onClose)
    LaunchedEffect(session.sessionId, pane) {
        while (true) {
            val r = ArtifactsV2Client.list("mine", null, null, null, archived = false)
            if (r is ApiResult.Ok) results = r.value.items.filter { it.origin?.sessionId == session.sessionId }
            delay(if (pane == StudioPane.Edit) 4_000 else 8_000)
        }
    }
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("artifactStudio")) {
        LibraryHeader(
            title = session.title,
            subtitle = session.kind.label,
            onBack = null,
            leading = { LibraryIconButton(com.allternit.core.design.R.drawable.ph_x, "Close", "artifactStudioClose", onClick = onClose) },
        )
        PillRow(
            StudioPane.entries, pane,
            { if (it == StudioPane.Chat) "Chat" else if (results.isEmpty()) "Edit" else "Edit · ${results.size}" },
            { "artifactStudio${it.name}" },
            Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
        ) { pane = it }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (pane) {
                StudioPane.Chat -> chatPane(session.sessionId)
                StudioPane.Edit -> {
                    val url = session.editorUrl(dark) ?: results.firstOrNull()?.let { ArtifactWeb.windowUrl(it.id, dark) }
                    if (url != null) {
                        ArtifactLiveWebView(
                            LiveRoute(url, ArtifactKinds.noun(session.kind.studioKind).lowercase(), editor = true),
                            onClose = onClose,
                            onChat = { pane = StudioPane.Chat },
                        )
                    } else {
                        StudioEmpty(session.kind, onDescribe = { pane = StudioPane.Chat })
                    }
                }
            }
        }
    }
}

@Composable
private fun StudioEmpty(kind: StudioKind, onDescribe: () -> Unit) {
    val c = AllternitTheme.colors
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
    ) {
        Box(Modifier.size(96.dp).clip(RoundedCornerShape(20.dp)).background(c.fillNeutral), contentAlignment = Alignment.Center) {
            Icon(painterResource(kind.deck), null, tint = c.textPrimary, modifier = Modifier.size(40.dp))
        }
        Text(kind.emptyTitle, color = c.textPrimary, fontSize = 20.sp, fontFamily = FontFamily.Serif, textAlign = TextAlign.Center)
        Text(kind.studioHint, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
        LibraryRowButton("Describe it in the chat", com.allternit.core.design.R.drawable.ph_arrow_right, "artifactStudioDescribe", onClick = onDescribe)
    }
}
