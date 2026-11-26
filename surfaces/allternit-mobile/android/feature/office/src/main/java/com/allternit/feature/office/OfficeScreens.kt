package com.allternit.feature.office

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.ArtifactSummary
import com.allternit.feature.artifacts.ArtifactsV2Client
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySpinner
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.parseTime
import com.allternit.feature.artifacts.relativeTime
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope

private val OFFICE_KINDS = listOf("doc", "sheet", "slides", "pdf")

private sealed interface DocsState {
    data object Loading : DocsState
    data class Failed(val message: String, val offline: Boolean) : DocsState
    data class Ready(val items: List<ArtifactSummary>) : DocsState
}

private fun kindIcon(kind: String): Int = when (kind) {
    "sheet" -> DesignR.drawable.deck_sheets
    "slides" -> DesignR.drawable.deck_slides
    "pdf" -> DesignR.drawable.deck_pdf
    else -> DesignR.drawable.deck_docs
}

/**
 * Office documents (iOS OfficeDocumentsView): the account's Docs, Sheets, Slides and PDFs from Artifacts v2
 * (`GET /api/v2/artifacts?kind=…`, one request per kind, merged newest first). A row opens the artifact viewer
 * ([openArtifact]); Sign PDF in the header opens a blank signing pad. [openArtifact] is :feature:shell's artifact page.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OfficeDocumentsScreen(onClose: () -> Unit, openArtifact: @Composable (id: String, title: String, onClose: () -> Unit) -> Unit) {
    val c = AllternitTheme.colors
    var state by remember { mutableStateOf<DocsState>(DocsState.Loading) }
    var reload by remember { mutableIntStateOf(0) }
    var refreshing by remember { mutableStateOf(false) }
    var signOpen by remember { mutableStateOf(false) }
    var open by remember { mutableStateOf<ArtifactSummary?>(null) }
    BackHandler(enabled = open == null && !signOpen, onBack = onClose)

    LaunchedEffect(reload) {
        val results = coroutineScope {
            OFFICE_KINDS.map { kind -> async { ArtifactsV2Client.list("all", kind, null, null, false) } }.map { it.await() }
        }
        val failure = results.firstOrNull { it !is ApiResult.Ok }
        state = if (failure != null) {
            DocsState.Failed(failure.friendly("load your documents"), failure is ApiResult.Network)
        } else {
            DocsState.Ready(results.flatMap { (it as ApiResult.Ok).value.items }.sortedByDescending { parseTime(it.updatedAt) })
        }
        refreshing = false
    }

    open?.let { s -> openArtifact(s.id, s.title, { open = null; reload += 1 }); return }
    if (signOpen) { PdfSignScreen(title = "Signed PDF", source = null, onClose = { signOpen = false }); return }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("officeDocumentsScreen")) {
        LibraryHeader(
            title = "Office documents", onBack = onClose, backTag = "officeDocumentsBack",
            trailing = { LibraryIconButton(DesignR.drawable.ph_identification_card, "Sign PDF", "officeSignPdf", iconSize = 20) { signOpen = true } },
        )
        when (val s = state) {
            DocsState.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LibrarySpinner("Loading documents…") }
            is DocsState.Failed -> FriendlyState(
                if (s.offline) FriendlyStyle.Offline else FriendlyStyle.Error, "Couldn't load documents", s.message,
                actionTitle = "Retry", onAction = { state = DocsState.Loading; reload += 1 },
            ) { Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary) }
            is DocsState.Ready -> if (s.items.isEmpty()) {
                FriendlyState(FriendlyStyle.Empty, "No documents yet", "Docs, sheets, decks and PDFs you make or are shared appear here.", art = EmptyArtKind.Sheets, icon = {})
            } else PullToRefreshBox(isRefreshing = refreshing, onRefresh = { refreshing = true; reload += 1 }, modifier = Modifier.fillMaxSize()) {
                LazyColumn(Modifier.fillMaxSize().testTag("officeDocumentsList"), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(s.items, key = { it.id }) { a ->
                        Row(
                            Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgSecondary).clickable { open = a }.padding(14.dp).testTag("officeDoc${a.id}"),
                            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Icon(painterResource(kindIcon(a.kind)), null, tint = c.accentPrimary, modifier = Modifier.size(24.dp))
                            Column(Modifier.weight(1f)) {
                                Text(a.title, color = c.textPrimary, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                                Text(relativeTime(parseTime(a.updatedAt)), color = c.textSecondary, fontSize = 12.sp)
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * The sectioned document screen (iOS OfficeDocumentView): reads `GET v1/artifacts/:id` and renders the section mapping
 * the web editors write (docs-editor blocks, slides-editor slides, sheets-editor TSV grids, pdf-viewer page text).
 * Read-only; the interactive editor is one tap away.
 */
@Composable
fun OfficeDocumentScreen(artifactId: String, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    var load by remember(artifactId) { mutableStateOf<OfficeLoad?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    var editing by remember { mutableStateOf(false) }
    LaunchedEffect(artifactId, reload) { load = null; load = OfficeClient.getArtifact(artifactId) }
    BackHandler(onBack = { if (editing) editing = false else onClose() })

    val artifact = (load as? OfficeLoad.Ok)?.artifact
    val family = artifact?.family
    if (editing && artifact != null && family != null) {
        Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding()) {
            LibraryHeader(title = artifact.title, subtitle = family.displayName, onBack = { editing = false }, backTag = "officeEditorBack")
            OfficeEditorWebView(family, artifactId, Modifier.weight(1f))
        }
        return
    }
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("officeDocumentScreen")) {
        LibraryHeader(
            title = artifact?.title ?: "Document", onBack = onClose, backTag = "officeDocumentBack",
            trailing = {
                if (family != null) LibraryIconButton(DesignR.drawable.ph_arrow_right, "Interactive", "officeInteractive", iconSize = 20) { editing = true }
            },
        )
        when (val l = load) {
            null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LibrarySpinner("Loading document…") }
            OfficeLoad.SignedOut -> FriendlyState(FriendlyStyle.Empty, "Sign in to open this document", "Documents are saved to your Allternit account, so they open once you're signed in.") {
                Icon(painterResource(DesignR.drawable.deck_docs), null, tint = c.accentPrimary)
            }
            OfficeLoad.ComputeRequired -> FriendlyState(FriendlyStyle.Empty, "No computer yet", "This document opens on your Allternit computer. Add one in Settings, then come back.", art = EmptyArtKind.ConnectScene, icon = {})
            is OfficeLoad.Failed -> FriendlyState(FriendlyStyle.Offline, "Couldn't load document", l.message, actionTitle = "Retry", onAction = { reload += 1 }) {
                Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
            }
            is OfficeLoad.Ok -> if (l.artifact.sections.isEmpty()) {
                FriendlyState(FriendlyStyle.Empty, "No office content", "This artifact has no office-editor sections.") {
                    Icon(painterResource(DesignR.drawable.deck_docs), null, tint = c.accentPrimary)
                }
            } else SelectionContainer {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    items(l.artifact.ordered.withIndex().toList(), key = { it.value.id }) { (index, section) ->
                        when (l.artifact.family) {
                            OfficeFamily.Slides -> SlideCard(section, index + 1)
                            OfficeFamily.Sheets -> SheetCard(section)
                            OfficeFamily.Pdf -> PageCard(section, index + 1)
                            else -> DocBlock(section)
                        }
                    }
                }
            }
        }
    }
}

/** docs-editor: one paragraph/heading block per section. */
@Composable
private fun DocBlock(s: OfficeSection) {
    val heading = s.kind.endsWith("/heading")
    Text(
        s.body, color = AllternitTheme.colors.textPrimary, fontSize = if (heading) 20.sp else 16.sp,
        fontWeight = if (heading) FontWeight.Bold else FontWeight.Normal, modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun CardColumn(label: String, content: @Composable () -> Unit) {
    val c = AllternitTheme.colors
    Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(c.bgSecondary).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, color = c.accentPrimary, fontSize = 12.sp, fontWeight = FontWeight.Bold)
        content()
    }
}

/** slides-editor: one card per slide, one line per paragraph. */
@Composable
private fun SlideCard(s: OfficeSection, number: Int) = CardColumn("Slide $number") {
    s.body.split("\n").filter { it.isNotEmpty() }.forEach { Text(it, color = AllternitTheme.colors.textPrimary, fontSize = 16.sp) }
}

/** pdf-viewer: one extracted-text page per section. */
@Composable
private fun PageCard(s: OfficeSection, number: Int) = CardColumn("Page $number") {
    Text(s.body.ifEmpty { "(empty page)" }, color = AllternitTheme.colors.textPrimary, fontSize = 16.sp, modifier = Modifier.fillMaxWidth())
}

/** sheets-editor: TSV body rendered as a monospaced grid. */
@Composable
private fun SheetCard(s: OfficeSection) {
    val c = AllternitTheme.colors
    val rows = s.body.split("\n").map { it.split("\t") }
    CardColumn(s.heading?.replace("Sheet: ", "") ?: "Sheet") {
        Column(Modifier.horizontalScroll(rememberScrollState())) {
            rows.forEach { row ->
                Row {
                    row.forEach { cell ->
                        Text(
                            cell, color = c.textPrimary, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
                            modifier = Modifier.widthIn(min = 72.dp).border(BorderStroke(0.5.dp, c.borderSubtle)).padding(horizontal = 8.dp, vertical = 4.dp),
                        )
                    }
                }
            }
        }
    }
}
