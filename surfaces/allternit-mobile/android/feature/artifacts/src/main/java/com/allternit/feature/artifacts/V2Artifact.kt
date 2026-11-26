package com.allternit.feature.artifacts

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.EmptyArtKind
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

// MARK: - Body decoders (iOS ArtifactBodyDecoder, the parts the phone draws natively)

/** A sheet body as rows of cell text: `[[..]]`, `{rows:[[..]|{..}]}` or `sheets[0].cells["r:c"]`. */
internal fun decodeSheet(body: String): List<List<String>>? {
    val root = runCatching { com.allternit.core.api.ApiJson.default.parseToJsonElement(body) }.getOrNull() ?: return null
    fun cell(e: JsonElement?): String = when (e) {
        null -> ""
        is JsonPrimitive -> e.contentOrNull ?: ""
        is JsonObject -> (e["v"] ?: e["value"] ?: e["text"])?.let { cell(it) } ?: ""
        is JsonArray -> e.joinToString(", ") { cell(it) }
    }
    fun rowsOf(a: JsonArray): List<List<String>> = a.map { r ->
        when (r) {
            is JsonArray -> r.map { cell(it) }
            is JsonObject -> r.values.map { cell(it) }
            else -> listOf(cell(r))
        }
    }
    (root as? JsonArray)?.let { return rowsOf(it) }
    val o = root as? JsonObject ?: return null
    (o["rows"] as? JsonArray)?.let { return rowsOf(it) }
    val sheet = (o["sheets"] as? JsonArray)?.firstOrNull() as? JsonObject ?: return null
    (sheet["rows"] as? JsonArray)?.takeIf { it.isNotEmpty() && it.first() is JsonArray }?.let { return rowsOf(it) }
    val cells = sheet["cells"] as? JsonObject ?: return null
    var maxR = -1; var maxC = -1
    val parsed = cells.mapNotNull { (k, v) ->
        val parts = k.split(":")
        val r = parts.getOrNull(0)?.toIntOrNull(); val c = parts.getOrNull(1)?.toIntOrNull()
        if (r == null || c == null) null else { maxR = maxOf(maxR, r); maxC = maxOf(maxC, c); Triple(r, c, cell(v)) }
    }
    if (maxR < 0) return emptyList()
    val grid = MutableList(maxR + 1) { MutableList(maxC + 1) { "" } }
    parsed.forEach { (r, c, v) -> grid[r][c] = v }
    return grid
}

/** One slide as title + lines (iOS SlidesListView input). */
internal data class SlideText(val title: String, val lines: List<String>)

internal fun decodeSlides(body: String): List<SlideText>? {
    val o = LibraryJson.parse(body) ?: return null
    val slides = o.list("slides").ifEmpty { o.child("deck").list("slides") }
    if (slides.isEmpty()) return null
    fun texts(el: JsonElement?): List<String> = when (el) {
        is JsonPrimitive -> listOfNotNull(el.contentOrNull?.takeIf { it.isNotBlank() })
        is JsonArray -> el.flatMap { texts(it) }
        is JsonObject -> listOf("text", "content", "items", "bullets").flatMap { texts(el[it]) }
        null -> emptyList()
    }
    return slides.map { s ->
        val lines = texts(s["bullets"]) + texts(s["body"]) + texts(s["blocks"]).filter { it != s.str("title") }
        SlideText(s.str("title") ?: lines.firstOrNull() ?: "", lines)
    }
}

/** What the viewer shows for a kind (iOS V2ArtifactView.render). */
private sealed interface Render {
    data class Page(val source: PageSource) : Render
    data class Live(val route: LiveRoute) : Render
    data class Sheet(val rows: List<List<String>>) : Render
    data class Slides(val slides: List<SlideText>) : Render
    data class Code(val text: String) : Render
    data class Unreadable(val label: String) : Render
}

private fun isMarkdown(format: String) = format == "text/markdown" || format == "text/plain" || format.contains("markdown")

private fun render(a: ArtifactV2, dark: Boolean): Render {
    val body = a.body
    val trimmed = body.trim()
    val live = Render.Live(LiveRoute(ArtifactWeb.viewerUrl(a.id), ArtifactKinds.noun(a.kind).lowercase(), seed = a))
    return when (a.kind) {
        "page" -> when {
            trimmed.startsWith("https://") && !trimmed.contains('\n') -> Render.Page(PageSource.Url(trimmed))
            isMarkdown(a.bodyFormat) && !trimmed.startsWith("<") -> Render.Page(PageSource.Html(ArtifactHTML.markdown(body)))
            a.bodyFormat.contains("jsx") || a.bodyFormat.contains("react") || body.contains("type=\"module\"") -> live
            else -> Render.Page(PageSource.Html(body))
        }
        "doc" -> when {
            isMarkdown(a.bodyFormat) -> Render.Page(PageSource.Html(ArtifactHTML.markdown(body)))
            else -> LibraryJson.parse(body).str("markdown")?.takeIf { it.isNotBlank() }?.let { Render.Page(PageSource.Html(ArtifactHTML.markdown(it))) } ?: live
        }
        "design" -> if (trimmed.startsWith("<")) Render.Page(PageSource.Html(body)) else live
        "diagram" -> if (trimmed.startsWith("<svg") || a.bodyFormat.contains("svg")) Render.Page(PageSource.Html(ArtifactHTML.svg(body))) else Render.Page(PageSource.Html(ArtifactHTML.mermaid(body)))
        "sheet" -> decodeSheet(body)?.let { Render.Sheet(it) } ?: Render.Unreadable(ArtifactKinds.noun(a.kind).lowercase())
        "slides" -> decodeSlides(body)?.let { Render.Slides(it) } ?: live
        "code" -> Render.Code(body)
        "image" -> if (trimmed.startsWith("https://") || trimmed.startsWith("data:image")) Render.Page(PageSource.Html(ArtifactHTML.image(trimmed))) else live
        "video" -> if (trimmed.startsWith("https://")) Render.Page(PageSource.Html(ArtifactHTML.video(trimmed))) else live
        else -> live
    }
}

private sealed interface V2State {
    data object Loading : V2State
    data object SignedOut : V2State
    data class Failed(val message: String) : V2State
    data class Ready(val artifact: ArtifactV2) : V2State
}

/**
 * The artifact viewer (iOS V2ArtifactView): back, the title, then comments (with its open count),
 * Share and ⋯. ⋯ opens a sheet (Go to chat, Open in browser, View comments, Edit, Export, Archive,
 * Delete), the same actions the iOS menu holds, reachable by tap. The body draws by kind.
 */
@Composable
fun V2ArtifactScreen(
    artifactId: String,
    onClose: () -> Unit,
    onOpenChat: (sessionId: String) -> Unit,
    onSignIn: () -> Unit,
) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val dark = c.bgPrimary.luminance() < 0.5f
    var state by remember(artifactId) { mutableStateOf<V2State>(V2State.Loading) }
    var reload by remember { mutableIntStateOf(0) }
    var moreOpen by remember { mutableStateOf(false) }
    var commentsOpen by remember { mutableStateOf(false) }
    var exportOpen by remember { mutableStateOf(false) }
    var pdfOpen by remember { mutableStateOf(false) }
    var signBytes by remember { mutableStateOf<ByteArray?>(null) }
    var legacyOpen by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var shareOpen by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    var makeAppOpen by remember { mutableStateOf(false) }
    var installReq by remember { mutableStateOf<InstallRequest?>(null) }
    var appLink by remember(artifactId) { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var openComments by remember { mutableIntStateOf(0) }
    BackHandler(enabled = !editing, onBack = onClose)

    LaunchedEffect(artifactId, reload) {
        state = V2State.Loading
        state = when (val r = ArtifactsV2Client.get(artifactId)) {
            is ApiResult.Ok -> r.value?.let { V2State.Ready(it) } ?: V2State.Failed("This artifact couldn't be read.")
            is ApiResult.HttpError -> if (r.status == 401) V2State.SignedOut else V2State.Failed(artifactError(r))
            else -> V2State.Failed(artifactError(r))
        }
        (state as? V2State.Ready)?.let { ready ->
            (ArtifactsV2Client.comments(ready.artifact.id) as? ApiResult.Ok)?.value?.let { list ->
                openComments = list.count { it.parentId == null && it.resolvedAt == null }
            }
        }
    }

    val ready = (state as? V2State.Ready)?.artifact
    if (editing && ready != null) {
        V2ArtifactEditorScreen(ready, onDone = { saved -> editing = false; if (saved != null) state = V2State.Ready(saved) })
        return
    }

    // iOS: the PDF's Sign sheet (PDFSignView) and the sectioned Office record's interactive screen (OfficeDocumentView).
    ArtifactOfficeHooks.signPdf?.let { sign ->
        val bytes = signBytes
        if (bytes != null) { sign(ready?.title ?: "Document", bytes, { signBytes = null }); return }
    }
    ArtifactOfficeHooks.legacyDocument?.let { legacy ->
        val legacyId = ready?.summary?.origin?.legacyId
        if (legacyOpen && legacyId != null) { legacy(legacyId, { legacyOpen = false }); return }
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("v2ArtifactView")) {
        LibraryHeader(
            title = ready?.title ?: "",
            subtitle = ready?.let { ArtifactKinds.noun(it.kind) },
            onBack = onClose,
            backTag = "v2ArtifactBack",
            trailing = {
                if (ready != null) {
                    Box {
                        LibraryIconButton(DesignR.drawable.ph_identification_card, if (openComments > 0) "Comments, $openComments open" else "Comments", "v2ArtifactComments", iconSize = 20) { commentsOpen = true }
                        if (openComments > 0) {
                            Text(
                                if (openComments > 9) "9+" else "$openComments", color = c.onAccent, fontSize = 9.sp, fontWeight = FontWeight.Bold,
                                modifier = Modifier.align(Alignment.TopEnd).padding(top = 6.dp, end = 4.dp).clip(CircleShape).background(c.accentPrimary).padding(horizontal = 4.dp),
                            )
                        }
                    }
                    LibraryIconButton(DesignR.drawable.ph_dots_three_vertical, "More", "v2ArtifactMore", iconSize = 20) { moreOpen = true }
                }
            },
        )
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (val s = state) {
                V2State.Loading -> LibrarySpinner("Loading artifact…")
                V2State.SignedOut -> FriendlyState(FriendlyStyle.Empty, "Sign in to open this artifact", "Artifacts are saved to your Allternit account, so they open once you're signed in.", actionTitle = "Sign in", onAction = onSignIn) {
                    Icon(painterResource(DesignR.drawable.deck_artifact), null, tint = c.textSecondary)
                }
                is V2State.Failed -> FriendlyState(FriendlyStyle.Error, "Couldn't open this artifact", s.message, actionTitle = "Retry", onAction = { reload += 1 }, modifier = Modifier.testTag("v2ArtifactError")) {
                    Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
                }
                is V2State.Ready -> {
                    val art = s.artifact
                    val canLegacy = art.summary.origin?.legacySource == "sectioned" && art.summary.origin?.legacyId != null &&
                        art.kind in setOf("doc", "sheet", "slides", "pdf") && ArtifactOfficeHooks.legacyDocument != null
                    val canSign = art.kind == "pdf" && ArtifactOfficeHooks.signPdf != null
                    Column(Modifier.fillMaxSize()) {
                        // iOS legacyEditorBanner: a visible bar above an imported Office record.
                        if (canLegacy) Row(
                            Modifier.fillMaxWidth().background(c.fillNeutral).testTag("v2ArtifactLegacyEditor")
                                .clickable(role = androidx.compose.ui.semantics.Role.Button) { legacyOpen = true }.padding(horizontal = 16.dp, vertical = 14.dp),
                            horizontalArrangement = Arrangement.SpaceBetween,
                        ) {
                            Text("Open in editor", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
                            Text("›", color = c.textSecondary, fontSize = 14.sp)
                        }
                        Box(Modifier.weight(1f).fillMaxWidth()) { ArtifactBody(art, dark) }
                        // iOS "Sign this PDF": a visible bar under the PDF.
                        if (canSign) Box(
                            Modifier.fillMaxWidth().background(c.fillNeutral).testTag("v2ArtifactSignPdf")
                                .clickable(role = androidx.compose.ui.semantics.Role.Button) {
                                    scope.launch {
                                        val bytes = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { pdfBytes(art) }
                                        if (bytes == null) actionError = "The PDF couldn't be read. Try again." else signBytes = bytes
                                    }
                                }.padding(vertical = 14.dp),
                            contentAlignment = Alignment.Center,
                        ) { Text("Sign this PDF", color = c.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold) }
                    }
                }
            }
        }
    }

    if (moreOpen && ready != null) {
        LibrarySheet(ready.title, "v2ArtifactMoreSheet", onDismiss = { moreOpen = false }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibraryRowButton("Share", DesignR.drawable.ph_upload_simple, "v2ArtifactShare") {
                    moreOpen = false
                    // An app-capable artifact opens the share sheet (Install app, Copy app link); others go straight to the chooser.
                    if (canMakeAppHere(ready.summary)) shareOpen = true else shareLink(context, ready)
                }
                val session = ready.summary.origin?.sessionId
                val surface = ready.summary.origin?.surface
                if (session != null && (surface == null || surface == "chat" || surface == "cowork")) {
                    LibraryRowButton("Go to chat", DesignR.drawable.ph_arrow_right, "v2ArtifactGoToChat") { moreOpen = false; onOpenChat(session) }
                }
                LibraryRowButton("Open in browser", DesignR.drawable.ph_globe, "v2ArtifactOpenBrowser") { moreOpen = false; openExternal(context, ArtifactWeb.shareLink(ready.id)) }
                LibraryRowButton("View comments", DesignR.drawable.ph_identification_card, "v2ArtifactViewComments") { moreOpen = false; commentsOpen = true }
                if (ready.canEdit && ArtifactKinds.editable(ready.kind)) {
                    LibraryRowButton("Edit", DesignR.drawable.ph_sliders_horizontal, "v2ArtifactEdit") { moreOpen = false; editing = true }
                }
                LibraryRowButton("Export", DesignR.drawable.ph_arrow_down, "v2ArtifactExport") { moreOpen = false; exportOpen = true }
                LibraryRowButton("Archive", DesignR.drawable.ph_floppy_disk, "v2ArtifactArchive") {
                    moreOpen = false
                    scope.launch {
                        val r = ArtifactsV2Client.setArchived(ready.id, true)
                        if (r is ApiResult.Ok) onClose() else actionError = artifactError(r)
                    }
                }
                if (ready.summary.isMine) LibraryRowButton("Delete", DesignR.drawable.ph_trash, "v2ArtifactDelete", destructive = true) { moreOpen = false; confirmDelete = true }
            }
        }
    }

    // The app link and whether this artifact is already an app (404 = not yet); only for the owner of an app-capable kind.
    androidx.compose.runtime.LaunchedEffect(ready?.id, makeAppOpen) {
        val a = ready?.summary ?: return@LaunchedEffect
        if (!canMakeAppHere(a)) return@LaunchedEffect
        appLink = (ArtifactsV2Client.app(a.id) as? ApiResult.Ok)?.let { savedAppFrom(a.id, it.value)?.link }
    }
    if (shareOpen && ready != null) {
        LibrarySheet("Share", "v2ArtifactShareSheet", onDismiss = { shareOpen = false }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                LibraryRowButton("Share link", DesignR.drawable.ph_upload_simple, "v2ArtifactShareLink") { shareOpen = false; shareLink(context, ready) }
                if (appLink != null) {
                    LibraryRowButton("Install app", DesignR.drawable.ph_download_simple, "v2ArtifactInstallApp") { shareOpen = false; installReq = InstallRequest(ready.summary, InstallTier.App) }
                    LibraryRowButton("Copy app link", DesignR.drawable.ph_link_simple, "v2ArtifactCopyAppLink") {
                        shareOpen = false
                        copyText(context, "App link", appLink.orEmpty())
                        notice = "App link copied."
                    }
                } else {
                    LibraryRowButton("Make it an app", DesignR.drawable.ph_plus_circle, "v2ArtifactMakeApp") { shareOpen = false; makeAppOpen = true }
                }
            }
        }
    }
    if (makeAppOpen && ready != null) {
        MakeAppSheet(ready.summary, onDismiss = { makeAppOpen = false }, onChanged = { }, onNotice = { notice = it })
    }
    installReq?.let { req ->
        LibrarySheet("Install ${req.artifact.title}?", "v2ArtifactInstallConsent", onDismiss = { installReq = null }) {
            InstallConsent(
                request = req,
                onTier = { installReq = req.copy(tier = it) },
                onCancel = { installReq = null },
                onInstall = {
                    installReq = null
                    scope.launch {
                        try {
                            val again = installArtifactApp(req)
                            if (again != null) installReq = again else notice = "${req.artifact.title} installed."
                        } catch (e: IllegalStateException) { actionError = e.message }
                    }
                },
            )
        }
    }

    if (exportOpen && ready != null) {
        ExportSheet(ready, onPdf = { exportOpen = false; pdfOpen = true }, onDismiss = { exportOpen = false })
    }
    if (pdfOpen && ready != null) {
        PDFTemplateSheet(ready, onDismiss = { pdfOpen = false })
    }
    if (commentsOpen && ready != null) {
        ArtifactCommentsSheet(ready, onDismiss = { commentsOpen = false }, onOpenCount = { openComments = it })
    }
    if (confirmDelete && ready != null) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete this artifact for everyone?") },
            text = { Text("People it's shared with lose access too. This can't be undone.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    scope.launch {
                        val r = ArtifactsV2Client.delete(ready.id)
                        if (r is ApiResult.Ok || (r is ApiResult.HttpError && r.status == 404)) onClose() else actionError = artifactError(r)
                    }
                }, modifier = Modifier.testTag("v2ArtifactDeleteConfirm")) { Text("Delete", color = AllternitStatic.StatusError) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }
    notice?.let { msg ->
        AlertDialog(onDismissRequest = { notice = null }, title = { Text(msg) }, confirmButton = { TextButton(onClick = { notice = null }) { Text("OK") } })
    }
    actionError?.let { msg ->
        AlertDialog(
            onDismissRequest = { actionError = null },
            title = { Text("Couldn't do that") },
            text = { Text(msg) },
            confirmButton = { TextButton(onClick = { actionError = null }) { Text("OK") } },
        )
    }
}

private fun shareLink(context: Context, a: ArtifactV2) {
    val send = Intent(Intent.ACTION_SEND).setType("text/plain")
        .putExtra(Intent.EXTRA_SUBJECT, a.title)
        .putExtra(Intent.EXTRA_TEXT, ArtifactWeb.shareLink(a.id))
    context.startActivity(Intent.createChooser(send, "Share").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
}

internal fun copyText(context: Context, label: String, text: String) {
    (context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager)?.setPrimaryClip(ClipData.newPlainText(label, text))
}

@Composable
private fun ArtifactBody(a: ArtifactV2, dark: Boolean) {
    val c = AllternitTheme.colors
    // Downloads, option B: a stored picture loads with its poster sharpening by the real bytes in, then shows.
    if (a.kind == "image" && a.body.trim().startsWith("https://")) { ByteLoadedImage(a); return }
    when (val r = remember(a.id, a.version, dark) { render(a, dark) }) {
        is Render.Page -> ArtifactPageWebView(r.source, Modifier.fillMaxSize())
        is Render.Live -> ArtifactLiveWebView(r.route, Modifier.fillMaxSize())
        is Render.Sheet -> SheetGrid(r.rows)
        is Render.Slides -> LazyColumn(Modifier.fillMaxSize().testTag("v2Slides"), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            itemsIndexed(r.slides) { i, s ->
                Column(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.bgPanel).border(1.dp, c.borderSubtle, RoundedCornerShape(14.dp)).padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text("${i + 1} / ${r.slides.size}", color = c.textTertiary, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                    Text(s.title.ifEmpty { "No text on this slide" }, color = c.textPrimary, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    s.lines.filter { it != s.title }.forEach { Text("• $it", color = c.textSecondary, fontSize = 14.sp) }
                }
            }
        }
        is Render.Code -> SelectionContainer(Modifier.fillMaxSize()) {
            Text(
                r.text, color = c.textPrimary, fontSize = 12.5.sp, lineHeight = 18.sp, fontFamily = FontFamily.Monospace,
                modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(16.dp),
            )
        }
        is Render.Unreadable -> FriendlyState(FriendlyStyle.Empty, "This ${r.label} couldn't be read", "Open it on the web or Desktop to export the source.") {
            Icon(painterResource(ArtifactKinds.icon(a.kind)), null, tint = c.textSecondary)
        }
    }
}

/** iOS SheetGridView: row/column headers, cells 110 wide, horizontal and vertical scroll. */
@Composable
private fun SheetGrid(rows: List<List<String>>) {
    val c = AllternitTheme.colors
    if (rows.isEmpty() || rows.all { it.isEmpty() }) {
        FriendlyState(FriendlyStyle.Empty, "No rows yet", "This sheet is empty.", art = EmptyArtKind.Chart)
        return
    }
    val cols = rows.maxOf { it.size }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(12.dp).testTag("v2SheetGrid")) {
        Row {
            Box(Modifier.width(36.dp).height(28.dp))
            repeat(cols) { j -> HeaderCell(columnName(j), 110) }
        }
        rows.forEachIndexed { i, row ->
            Row {
                HeaderCell("${i + 1}", 36)
                repeat(cols) { j ->
                    Text(
                        row.getOrNull(j).orEmpty(), color = c.textPrimary, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.width(110.dp).height(30.dp).border(0.5.dp, c.borderSubtle).padding(horizontal = 6.dp, vertical = 6.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun HeaderCell(text: String, width: Int) {
    val c = AllternitTheme.colors
    Box(Modifier.width(width.dp).height(28.dp).background(c.fillNeutral).border(0.5.dp, c.borderSubtle), contentAlignment = Alignment.Center) {
        Text(text, color = c.textSecondary, fontSize = 11.sp, fontWeight = FontWeight.Medium)
    }
}

internal fun columnName(index: Int): String {
    var n = index
    val sb = StringBuilder()
    do { sb.insert(0, ('A' + n % 26)); n = n / 26 - 1 } while (n >= 0)
    return sb.toString()
}

/**
 * The Export part of iOS ArtifactSharePanel: Save or share file (pdf, image, video: saved where
 * the person picks), PDF from template (doc, sheet, ops, dashboard) and Copy content (text kinds).
 */
@Composable
private fun ExportSheet(a: ArtifactV2, onPdf: () -> Unit, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var status by remember { mutableStateOf<String?>(null) }
    var bytes by remember { mutableStateOf<ByteArray?>(null) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("*/*")) { uri ->
        val data = bytes
        bytes = null
        if (uri != null && data != null) scope.launch {
            val ok = withContext(Dispatchers.IO) { runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(data) } != null }.getOrDefault(false) }
            status = if (ok) null else "The file couldn't be saved on this phone."
        }
    }
    val textKind = a.kind in setOf("doc", "page", "code", "diagram", "design", "sheet", "slides", "card", "dashboard", "ops", "motion")
    LibrarySheet("Export", "v2ArtifactExportSheet", onDismiss = onDismiss) {
        Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            if (ArtifactKinds.fileExport(a.kind)) {
                LibraryRowButton("Save or share file", DesignR.drawable.ph_arrow_down, "v2ArtifactSaveFile") {
                    status = "Preparing file…"
                    scope.launch {
                        // Downloads, option B: the status line carries real bytes, percent and ETA while the file is fetched.
                        val data = withContext(Dispatchers.IO) { fetchBytesWithProgress(a.body.trim()) { p -> status = ByteProgressText.status("Saving", p) } }
                        if (data == null) {
                            status = "The file couldn't be read. Try the export again."
                        } else {
                            status = null
                            bytes = data
                            val ext = when (a.kind) { "pdf" -> "pdf"; "video" -> "mp4"; else -> "png" }
                            save.launch(safeFileName("${a.title}.$ext"))
                        }
                    }
                }
            }
            if (ArtifactKinds.pdfTemplate(a.kind)) LibraryRowButton("PDF from template", DesignR.drawable.deck_pdf, "v2ArtifactPdfTemplate", onClick = onPdf)
            if (textKind) LibraryRowButton("Copy content", DesignR.drawable.ph_copy, "v2ArtifactCopy") { copyText(context, a.title, a.body); onDismiss() }
            if (!ArtifactKinds.fileExport(a.kind) && !ArtifactKinds.pdfTemplate(a.kind) && !textKind) {
                Text("Nothing to export from the phone for this one. Open it on Desktop or the web to export.", color = c.textSecondary, fontSize = 14.sp, modifier = Modifier.padding(8.dp))
            }
            status?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 8.dp)) }
        }
    }
}

/** Reads a file body: https (no bearer), `/api/...` paths (with the bearer), or a data: URL. */
internal fun fetchBytes(ref: String): ByteArray? = runCatching {
    when {
        ref.startsWith("data:") -> android.util.Base64.decode(ref.substringAfter(','), android.util.Base64.DEFAULT)
        ref.startsWith("https://") -> okhttp3.OkHttpClient().newCall(okhttp3.Request.Builder().url(ref).build()).execute().use { if (it.isSuccessful) it.body?.bytes() else null }
        ref.startsWith("/api/") -> LibraryNet.okHttp.newCall(okhttp3.Request.Builder().url(LibraryNet.absolute(ref)).build()).execute().use { if (it.isSuccessful) it.body?.bytes() else null }
        else -> null
    }
}.getOrNull()?.takeIf { it.isNotEmpty() && it.size <= 512 * 1024 * 1024 }

/** The PDF's bytes: a base64 body (`application/pdf`) or an https / `/api/` / data: reference. */
internal fun pdfBytes(a: ArtifactV2): ByteArray? {
    val b = a.body.trim()
    return runCatching {
        if (b.startsWith("data:") || b.startsWith("https://") || b.startsWith("/api/")) fetchBytes(b)
        else android.util.Base64.decode(b, android.util.Base64.DEFAULT)
    }.getOrNull()?.takeIf { it.isNotEmpty() }
}

/** A stored picture over https: [ByteLoadingView] while its bytes arrive, then the picture itself (inline, so it does not download twice). */
@Composable
private fun ByteLoadedImage(a: ArtifactV2) {
    val url = a.body.trim()
    var progress by remember(url) { androidx.compose.runtime.mutableStateOf<ByteProgress?>(null) }
    val loaded by androidx.compose.runtime.produceState<LoadedImage>(LoadedImage.Loading, url) {
        value = withContext(Dispatchers.IO) {
            val bytes = fetchBytesWithProgress(url) { p -> progress = p }
            // Over 12 MB, or unreadable: let the page fetch it itself rather than hold it in memory twice.
            if (bytes == null || bytes.size > 12 * 1024 * 1024) LoadedImage.Direct
            else LoadedImage.Data("data:${sniffImageMime(bytes)};base64," + android.util.Base64.encodeToString(bytes, android.util.Base64.NO_WRAP),
                android.graphics.BitmapFactory.decodeByteArray(bytes, 0, bytes.size))
        }
    }
    // The halftone poster the download showed: the finished image crossfades from it; with no poster the cells bloom in.
    var poster by remember(url) { androidx.compose.runtime.mutableStateOf<android.graphics.Bitmap?>(null) }
    var revealed by remember(url) { androidx.compose.runtime.mutableStateOf(false) }
    when (val l = loaded) {
        is LoadedImage.Data -> if (l.bitmap != null && !revealed) {
            com.allternit.core.design.DotField(
                com.allternit.core.design.DotFieldMode.Reveal(
                    l.bitmap,
                    if (poster != null) com.allternit.core.design.DotRevealStyle.Crossfade else com.allternit.core.design.DotRevealStyle.Cells,
                    progress?.fraction?.toFloat()?.coerceIn(0f, 0.8f) ?: 0.8f,
                ),
                Modifier.fillMaxSize().testTag("v2ImageReveal"),
                onRevealed = { revealed = true },
            )
        } else if (l.url.startsWith("data:image/gif;base64,")) {
            // GIF: play it with ImageDecoder + AnimatedImageDrawable (first frame below API 28).
            val gifBytes = remember(l.url) { runCatching { android.util.Base64.decode(l.url.substringAfter(','), android.util.Base64.DEFAULT) }.getOrNull() }
            if (gifBytes != null) com.allternit.core.design.AnimatedGif(gifBytes, Modifier.fillMaxSize(), "Animated image")
            else ArtifactPageWebView(PageSource.Html(ArtifactHTML.image(l.url)), Modifier.fillMaxSize())
        } else ArtifactPageWebView(PageSource.Html(ArtifactHTML.image(l.url)), Modifier.fillMaxSize())
        else -> Unit
    }
    when (val l = loaded) {
        LoadedImage.Loading -> ByteLoadingView(ByteLoading.poster(a.meta), progress, Modifier.fillMaxSize().testTag("v2ImageLoading"), onPoster = { poster = it })
        LoadedImage.Direct -> ArtifactPageWebView(PageSource.Html(ArtifactHTML.image(url)), Modifier.fillMaxSize())
        is LoadedImage.Data -> Unit
    }
}

private sealed interface LoadedImage {
    data object Loading : LoadedImage
    data object Direct : LoadedImage
    data class Data(val url: String, val bitmap: android.graphics.Bitmap? = null) : LoadedImage
}

internal fun sniffImageMime(b: ByteArray): String = when {
    b.size > 3 && b[0] == 0x89.toByte() && b[1] == 'P'.code.toByte() -> "image/png"
    b.size > 2 && b[0] == 0xFF.toByte() && b[1] == 0xD8.toByte() -> "image/jpeg"
    b.size > 3 && b[0] == 'G'.code.toByte() && b[1] == 'I'.code.toByte() -> "image/gif"
    b.size > 11 && b[8] == 'W'.code.toByte() && b[9] == 'E'.code.toByte() -> "image/webp"
    else -> "image/png"
}
