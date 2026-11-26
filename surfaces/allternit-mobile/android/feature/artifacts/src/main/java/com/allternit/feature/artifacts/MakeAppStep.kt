package com.allternit.feature.artifacts

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** The saved app: its slug (the artifact id) and the link people open (`app.link`, else the share link). */
data class SavedApp(val slug: String, val link: String, val tier: InstallTier)

/** Where an app link points: its own `link`, else the artifact share link. */
fun appLinkFor(artifactId: String, link: String?): String = link?.takeIf { it.isNotBlank() } ?: ArtifactWeb.shareLink(artifactId)

/** Reads `{app}` from a GET/PUT `artifacts/:id/app` answer. */
fun savedAppFrom(artifactId: String, body: kotlinx.serialization.json.JsonElement?): SavedApp? {
    val app = body.obj.child("app") ?: return null
    return SavedApp(app.str("slug") ?: artifactId, appLinkFor(artifactId, app.str("link")), InstallTier.from(app.str("permission_tier") ?: app.str("tier")))
}

/** Same copy as web `describeMakeAppError`. */
fun makeAppError(r: ApiResult<*>): String = when {
    r is ApiResult.HttpError && r.code == "full_tier_not_allowed" -> "The Full level is only for you and your org. Pick Browser or App."
    r is ApiResult.HttpError && r.code == "kind_not_installable" -> "This kind of artifact can't be an app yet."
    r is ApiResult.HttpError && (r.code == "cap_reached" || r.status == 429) -> "You've reached the limit for app links. Remove one and try again."
    r is ApiResult.HttpError && r.status == 410 -> "This app was turned off by Allternit and can't be changed."
    else -> r.friendly("save this app")
}

/**
 * Installs an artifact app (`POST apps/artifact/:id/install`, the same call web `useInstallApp` makes).
 * Returns null when installed, an [InstallRequest] with `raised = true` when the server asks for more access,
 * or throws a message string wrapped in [IllegalStateException] on failure.
 */
suspend fun installArtifactApp(req: InstallRequest): InstallRequest? {
    val body = buildJsonObject { put("granted_tier", JsonPrimitive(req.tier.wire)); put("consent", JsonPrimitive(true)) }
    val res = LibraryNet.send("POST", "apps/artifact/${LibraryNet.escape(req.artifact.id)}/install", body)
    if (res !is ApiResult.Ok) throw IllegalStateException(res.friendly("install this app"))
    val o = res.value.obj
    if (o.bool("needs_consent") != true) return null
    val added = (o?.get("added") as? kotlinx.serialization.json.JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }.orEmpty()
    return InstallRequest(req.artifact, InstallTier.from(o.str("requested_tier") ?: req.tier.wire), added, true)
}

/** Android has no org context in the artifact list, so Full is scoped to the owner (web `inOrg = false`). */
private fun scopeFor(tier: InstallTier) = if (tier == InstallTier.Full) "owner" else "link"

/**
 * "Make it an app" (Artifacts redesign 6.8; web `MakeAppStep` and `AppPublishBar`): name, who can use it,
 * Save (`PUT api/v2/artifacts/:id/app`), then the app link with Copy app link and Install app.
 * States: editing, saving, error under the buttons, saved. [onChanged] fires once the app is saved or installed
 * so the Your apps shelf can reload.
 */
@Composable
fun MakeAppSheet(artifact: ArtifactSummary, onDismiss: () -> Unit, onChanged: () -> Unit, onNotice: (String) -> Unit = {}) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var name by remember(artifact.id) { mutableStateOf(artifact.title) }
    var tier by remember(artifact.id) { mutableStateOf(InstallTier.Browser) }
    var saving by remember(artifact.id) { mutableStateOf(false) }
    var error by remember(artifact.id) { mutableStateOf<String?>(null) }
    var saved by remember(artifact.id) { mutableStateOf<SavedApp?>(null) }
    var consent by remember(artifact.id) { mutableStateOf<InstallRequest?>(null) }
    var installing by remember(artifact.id) { mutableStateOf(false) }

    fun install(req: InstallRequest) {
        installing = true; error = null
        scope.launch {
            try {
                val again = installArtifactApp(req)
                if (again != null) consent = again else { consent = null; onChanged(); onNotice("${artifact.title} installed.") ; onDismiss() }
            } catch (e: IllegalStateException) { error = e.message }
            installing = false
        }
    }

    LibrarySheet("Make it an app", "artifactMakeAppSheet", onDismiss = onDismiss) {
        val done = saved
        val ask = consent
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            when {
                ask != null -> InstallConsent(ask, onTier = { consent = ask.copy(tier = it) }, onCancel = { consent = null }, onInstall = { install(ask) })
                done != null -> {
                    Text("Saved. It is in Your apps.", color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    Text(done.link, color = c.textSecondary, fontSize = 13.sp, maxLines = 2, modifier = Modifier.testTag("artifactMakeAppLink"))
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        SheetPill("Copy app link", false, Modifier.weight(1f), "artifactMakeAppCopy") { copyText(context, "App link", done.link); onNotice("App link copied.") }
                        SheetPill(if (installing) "Installing…" else "Install app", true, Modifier.weight(1f), "artifactMakeAppInstall", enabled = !installing) {
                            install(InstallRequest(artifact, done.tier))
                        }
                    }
                }
                else -> {
                    Text("Name", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    Box(Modifier.fillMaxWidth().heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).background(c.fillNeutral).padding(horizontal = 14.dp), contentAlignment = Alignment.CenterStart) {
                        BasicTextField(name, { name = it.take(80) }, singleLine = true, textStyle = TextStyle(color = c.textPrimary, fontSize = 16.sp),
                            cursorBrush = SolidColor(c.textPrimary), modifier = Modifier.fillMaxWidth().testTag("artifactMakeAppName"))
                    }
                    Text("Who can use it, and what it can do", color = c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        InstallTier.entries.forEach { t ->
                            val on = t == tier
                            Text(t.label, color = if (on) c.onAccent else c.textSecondary, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                                modifier = Modifier.heightIn(min = 44.dp).clip(CircleShape).background(if (on) c.accentPrimary else c.fillNeutral)
                                    .testTag("artifactMakeAppTier${t.label}").clickable(onClickLabel = "Use ${t.label}") { tier = t }.padding(horizontal = 14.dp, vertical = 12.dp))
                        }
                    }
                    Text(tier.detail, color = c.textSecondary, fontSize = 14.sp)
                    SheetPill(if (saving) "Saving…" else "Save app", true, Modifier.fillMaxWidth(), "artifactMakeAppSave", enabled = !saving && name.isNotBlank()) {
                        saving = true; error = null
                        scope.launch {
                            val r = ArtifactsV2Client.putApp(artifact.id, name.trim(), tier.wire, scopeFor(tier))
                            if (r is ApiResult.Ok) {
                                saved = savedAppFrom(artifact.id, r.value) ?: SavedApp(artifact.id, appLinkFor(artifact.id, null), tier)
                                onChanged()
                            } else error = makeAppError(r)
                            saving = false
                        }
                    }
                }
            }
            if (saving || installing) Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) { CircularProgressIndicator(Modifier.padding(2.dp), strokeWidth = 2.dp, color = c.textSecondary) }
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.testTag("artifactMakeAppError")) }
        }
    }
}

@Composable
private fun SheetPill(title: String, primary: Boolean, modifier: Modifier, tag: String, enabled: Boolean = true, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    Box(
        modifier.heightIn(min = 48.dp).clip(CircleShape).background(if (primary) c.accentPrimary else c.fillNeutral)
            .then(if (primary) Modifier else Modifier.border(1.dp, c.borderSubtle, CircleShape))
            .testTag(tag).clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(title, color = (if (primary) c.onAccent else c.textPrimary).copy(alpha = if (enabled) 1f else 0.4f), fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
    }
}

/** True when the artifact can become an app and the person owns it. */
fun canMakeAppHere(a: ArtifactSummary): Boolean = a.isMine && canBecomeApp(a.kind)
