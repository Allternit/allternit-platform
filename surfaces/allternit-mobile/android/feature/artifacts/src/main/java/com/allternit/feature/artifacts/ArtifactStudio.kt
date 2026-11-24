package com.allternit.feature.artifacts

import com.allternit.core.design.AllternitSerif
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

/**
 * The studio session's standing instructions, sent as agent-chat `systemPrompt` on every turn
 * (iOS ArtifactStudioKind.systemPrompt(title:): the mode contract, the "## Artifact session" note,
 * then OWN_TOOLS_NOTE, word for word). The shell wires [systemPrompt] into ChatViewModel.
 */
object ArtifactStudioPrompts {
    private const val PREFS = "allternit_artifact_studio"

    fun systemPrompt(context: Context, sessionId: String): String? {
        val raw = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(sessionId, null) ?: return null
        val o = LibraryJson.parse(raw) ?: return null
        val kind = StudioKind.entries.firstOrNull { it.raw == o.str("kind") } ?: return null
        return build(kind, o.str("title") ?: kind.newTitle)
    }

    fun build(kind: StudioKind, title: String): String {
        val n = ArtifactKinds.noun(kind.studioKind).lowercase().let { if (kind == StudioKind.Research) "research report" else if (kind == StudioKind.Build) "build" else it }
        val note = when (kind) {
            StudioKind.Image -> "## Artifact session\n\nThis session makes images, starting with \"$title\". Make each one with the media_generate tool; the result lands in the session's Outputs and opens beside this chat. Check the rendered PNG it returns and fix it before you call the image done."
            StudioKind.Video -> "## Artifact session\n\nThis session makes videos, starting with \"$title\". Make each one with the media_generate tool (kind \"video\"); the MP4 lands in the session's Outputs and opens beside this chat. Check the contact sheet it returns and fix the scene before you call the video done."
            StudioKind.Build -> "## Artifact session\n\nThis session makes one build, titled \"$title\". Deliver it as one HTML artifact the user can open beside this chat, and make it look finished: web libraries load in the preview (Tailwind or other CSS/JS from a CDN, web fonts, icon sets, https images). Then look at it: call pane_artifact with action \"call\", tool \"preview_output\" to get a picture of the rendered page (describe lists the other tools), compare it with what the user asked for, and deliver a corrected version when something is off. Check the phone width too (width 390) when the site should work on phones. Before you call it done, run artifact_check on the saved artifact: it pictures phone, tablet and laptop widths, lists script errors and sideways scroll, and compares with the user's picture when you pass reference \"attachment\"."
            StudioKind.Research -> "## Artifact session\n\nThis session makes one $n, titled \"$title\". Deliver it as an artifact the user can open beside this chat."
            else -> "## Artifact session\n\nThis session makes one $n, titled \"$title\". Make it with `artifact_create` (kind \"${kind.studioKind}\"); it opens in the pane beside this chat as soon as it is saved. For every later change, call `artifact_read` then `artifact_update` on that same artifact instead of creating another one, unless the user asks for a new one."
        }
        return listOfNotNull(modeContract(kind.modeId), note, OWN_TOOLS_NOTE).joinToString("\n\n")
    }

    private fun modeContract(modeId: String?): String? {
        val (brief, artifact, caps) = when (modeId) {
            "research" -> Triple("You are executing Deep Research mode. Search multiple authoritative sources, distinguish evidence from inference, attach citations to claims, reconcile conflicting sources, and create a structured research-report artifact. Do not answer from memory alone.", "research-report", "web_search, browser, file_write")
            "website" -> Triple("You are executing Build mode. Produce a runnable website or app artifact with complete HTML/CSS/JS or framework files, responsive behavior, accessible interaction, and no placeholder sections. Validate the artifact structure before completion.", "website", "artifact_create, file_write")
            "docs" -> Triple("You are executing Docs mode. Produce an editable structured document artifact\u2014not only chat markdown\u2014with headings, semantic sections, consistent styles, and the requested professional format. Validate completeness and exportability.", "document", "document_create, file_write")
            "data" -> Triple("You are executing Sheets mode. Produce an editable spreadsheet artifact with typed cells, formulas where appropriate, source-data preservation, and charts or analysis requested by the user. Validate formulas, ranges, and missing-data behavior.", "spreadsheet", "spreadsheet_create, spreadsheet_formula, file_write")
            "slides" -> Triple("You are executing Slides mode. Produce an editable presentation artifact with a coherent narrative, slide-specific layouts, visual hierarchy, and speaker-ready content. Validate slide count, overflow, and required sections.", "presentation", "presentation_create, file_write")
            "image" -> Triple("You are executing Image mode. Make the image with the media_generate tool (kind \"image\"; its description explains the lanes and how to write a native image as svg, html or canvas code), check the rendered PNG it returns, and fix it if needed. Match the requested subject, composition, dimensions, and constraints. A textual image description is not completion.", "image", "media_generate")
            "video" -> Triple("You are executing Video mode. Make the video with the media_generate tool (kind \"video\"; its description explains how to write a native video as a canvas scene with an optional score), check the contact sheet it returns, and fix the scene if needed. Return a playable video artifact with the requested duration and format. A storyboard, EDL, or prose description alone is not completion.", "video", "media_generate")
            else -> return null
        }
        return "<!--allternit-mode-contract-->\n$brief\n$COMPLETION_LAW\n\nMODE: $modeId\nREQUIRED ARTIFACT: $artifact\nREQUIRED CAPABILITIES: $caps\n<!--/allternit-mode-contract-->"
    }

    private const val COMPLETION_LAW = "MODE EXECUTION LAW:\n- Do not claim completion from prose alone.\n- Produce the required artifact and use the required capabilities.\n- If a required capability or provider is unavailable, return MODE_EXECUTION_BLOCKED with the missing capability. Never simulate a successful artifact.\n- Preserve the user's requested format and validate the deliverable before reporting success.\n- End successful work with MODE_EXECUTION_COMPLETE and a concise artifact manifest."

    const val OWN_TOOLS_NOTE = "The session tools named here (pane_artifact, media_generate, pane_browser) may be listed with a prefix, such as mcp__allternit__pane_artifact; they are the same tools. Do this session's work with them, in Allternit. Don't create or edit documents, images or files through outside connectors or services (claude.ai Docs, Google Drive, Notion, email and the like), and don't write a local file instead of changing what the user has open, unless the user explicitly asks for that."
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
        Text(kind.emptyTitle, color = c.textPrimary, fontSize = 20.sp, fontFamily = AllternitSerif, textAlign = TextAlign.Center)
        Text(kind.studioHint, color = c.textSecondary, fontSize = 14.sp, textAlign = TextAlign.Center)
        LibraryRowButton("Describe it in the chat", com.allternit.core.design.R.drawable.ph_arrow_right, "artifactStudioDescribe", onClick = onDescribe)
    }
}
