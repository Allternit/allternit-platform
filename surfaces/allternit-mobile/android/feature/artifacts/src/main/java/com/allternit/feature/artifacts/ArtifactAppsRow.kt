package com.allternit.feature.artifacts

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
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
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.layout.PaddingValues
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.R as DesignR

/** One row of `GET apps` `yours` (the same shelf the ACI Apps page reads, feature/computer AciAppsShelf). */
data class AppShelfRow(val kind: String, val slug: String, val name: String, val icon: String?, val link: String?, val installed: Boolean) {
    /** An artifact app's slug is the artifact id. */
    val artifactId: String? get() = slug.takeIf { kind == "artifact" && it.isNotBlank() }

    companion object {
        fun from(o: kotlinx.serialization.json.JsonObject): AppShelfRow {
            val slug = o.str("slug").orEmpty()
            return AppShelfRow(o.str("kind") ?: "artifact", slug, o.str("name") ?: slug, o.str("icon"), o.str("link"), o.bool("installed") ?: false)
        }
    }
}

/** Loads the person's own apps for the Artifacts page. `null` = not loaded yet, failure keeps the last rows. */
suspend fun loadAppShelf(): Result<List<AppShelfRow>> = when (val r = LibraryNet.get("apps")) {
    is ApiResult.Ok -> Result.success(r.value.obj.list("yours").map(AppShelfRow::from))
    else -> Result.failure(IllegalStateException(r.friendly("load your apps")))
}

/** "Updated 2 d ago · shared with a link" for an artifact app; vendor apps (or no list entry yet) keep "Your app". */
fun appSubtitle(row: AppShelfRow, artifact: ArtifactSummary?): String {
    if (artifact == null) return if (row.installed) "Installed" else "Your app"
    val share = when (artifact.visibility) { "link" -> "shared with a link"; "people" -> "shared"; "org" -> "shared with your org"; else -> "private" }
    return "Updated ${relativeTime(parseTime(artifact.updatedAt)).lowercase().ifEmpty { "recently" }} · $share"
}

/**
 * "Your apps" shelf (Artifacts redesign 6.5; web `YourAppsRow.tsx`): a dashed "Make an app" card first
 * (starts App mode), then one card per app on a horizontal shelf. States: loading placeholder, error line
 * with Retry, empty (just the Make card). The page hides it when signed out.
 */
@Composable
fun ArtifactAppsRow(
    artifacts: List<ArtifactSummary>,
    onMakeApp: () -> Unit,
    onOpen: (AppShelfRow) -> Unit,
    onRows: (List<AppShelfRow>) -> Unit,
    reloadKey: Any? = null,
) {
    val c = AllternitTheme.colors
    var rows by remember { mutableStateOf<List<AppShelfRow>?>(null) }
    var failed by remember { mutableStateOf(false) }
    var attempt by remember { mutableStateOf(0) }
    LaunchedEffect(attempt, reloadKey) {
        loadAppShelf().onSuccess { rows = it; failed = false; onRows(it) }.onFailure { failed = true }
    }
    Column(Modifier.fillMaxWidth().testTag("artifactsYourApps"), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Your apps", color = c.textTertiary, fontSize = 12.sp, modifier = Modifier.padding(top = 4.dp).semantics { contentDescription = "Your apps" })
        LazyRow(horizontalArrangement = Arrangement.spacedBy(10.dp), contentPadding = PaddingValues(end = 14.dp)) {
            item(key = "make") {
                val shape = RoundedCornerShape(14.dp)
                Row(
                    Modifier.width(220.dp).height(60.dp).clip(shape).border(BorderStroke(1.dp, c.borderSubtle), shape)
                        .testTag("artifactsMakeApp").clickable(onClickLabel = "Make an app", onClick = onMakeApp).padding(horizontal = 12.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(Modifier.size(32.dp).clip(RoundedCornerShape(8.dp)).background(c.fillNeutral), contentAlignment = Alignment.Center) {
                        Icon(painterResource(DesignR.drawable.ph_plus), null, tint = c.textPrimary, modifier = Modifier.size(16.dp))
                    }
                    Column {
                        Text("Make an app", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                        Text("Describe it, Gizzi builds it", color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
            if (rows == null && !failed) item(key = "loading") {
                Box(Modifier.width(220.dp).height(60.dp).clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).semantics { contentDescription = "Loading your apps" })
            }
            items(rows.orEmpty(), key = { it.kind + ":" + it.slug }) { app ->
                val shape = RoundedCornerShape(14.dp)
                val artifact = app.artifactId?.let { id -> artifacts.firstOrNull { it.id == id } }
                Row(
                    Modifier.width(240.dp).height(60.dp).clip(shape).background(c.bgPanel).border(BorderStroke(1.dp, c.borderSubtle), shape)
                        .testTag("artifactApp${app.slug}").clickable(onClickLabel = "Open ${app.name}") { onOpen(app) }.padding(horizontal = 12.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    val glyph = app.icon?.takeIf { it.isNotEmpty() && it.codePointCount(0, it.length) <= 2 } ?: app.name.take(1).uppercase()
                    Box(Modifier.size(32.dp).clip(RoundedCornerShape(8.dp)).background(c.fillNeutral), contentAlignment = Alignment.Center) {
                        Text(glyph, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                    }
                    Column(Modifier.weight(1f)) {
                        Text(app.name, color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(appSubtitle(app, artifact), color = c.textSecondary, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }
        if (failed) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text("Could not load your apps.", color = c.textSecondary, fontSize = 12.sp)
                Text("Retry", color = c.textPrimary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
                    modifier = Modifier.clip(RoundedCornerShape(6.dp)).testTag("artifactsAppsRetry").clickable { attempt++ }.padding(4.dp))
            }
        }
    }
}

/** Kinds that can become an app (they run as a page; web `canBecomeApp`). Docs, sheets and slides cannot. */
fun canBecomeApp(kind: String): Boolean = kind in setOf("page", "dashboard", "ops", "app", "build", "code", "html")
