package com.allternit.feature.artifacts

import android.annotation.SuppressLint
import android.content.Intent
import android.print.PrintAttributes
import android.print.PrintManager
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.api.ApiResult
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import java.util.Locale
import kotlin.coroutines.resume
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject

// MARK: - Comments (iOS ArtifactCommentsSheet + ArtifactCollabRules)

@Composable
fun ArtifactCommentsSheet(a: ArtifactV2, onDismiss: () -> Unit, onOpenCount: (Int) -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var comments by remember { mutableStateOf<List<ArtifactComment>?>(null) }
    var failed by remember { mutableStateOf(false) }
    var reload by remember { mutableIntStateOf(0) }
    var showResolved by remember { mutableStateOf(false) }
    var draft by remember { mutableStateOf("") }
    var replyTo by remember { mutableStateOf<ArtifactComment?>(null) }
    var confirmDelete by remember { mutableStateOf<ArtifactComment?>(null) }
    val access = a.summary.myAccess
    val canComment = access == "comment" || access == "edit" || access == "owner"
    LaunchedEffect(reload) {
        when (val r = ArtifactsV2Client.comments(a.id)) {
            is ApiResult.Ok -> { comments = r.value; failed = false; onOpenCount(r.value.count { it.parentId == null && it.resolvedAt == null }) }
            else -> failed = true
        }
    }
    val roots = comments.orEmpty().filter { it.parentId == null }
    val resolvedCount = roots.count { it.resolvedAt != null }
    val shown = roots.filter { showResolved || it.resolvedAt == null }
    LibrarySheet(
        "Comments", "artifactComments", onDismiss = onDismiss,
        leading = LibrarySheetAction("Close", testTag = "artifactCommentsClose", onClick = onDismiss),
        trailing = if (resolvedCount > 0) LibrarySheetAction(if (showResolved) "Hide resolved" else "Resolved ($resolvedCount)") { showResolved = !showResolved } else null,
    ) {
        Column(Modifier.fillMaxWidth().heightIn(min = 320.dp).imePadding()) {
            Box(Modifier.weight(1f, fill = false)) {
                when {
                    failed -> FriendlyState(FriendlyStyle.Error, "Couldn't load comments", "Check your connection and try again.", actionTitle = "Retry", onAction = { reload += 1 }) {
                        Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
                    }
                    comments == null -> LibrarySpinner("Loading comments…")
                    shown.isEmpty() -> FriendlyState(
                        FriendlyStyle.Empty, if (roots.isEmpty()) "No comments yet" else "No open comments",
                        if (canComment) "Add a comment below." else "Only people who can comment can add one.",
                        art = com.allternit.core.design.EmptyArtKind.Envelope, icon = {},
                    )
                    else -> LazyColumn(Modifier.fillMaxWidth().heightIn(max = 460.dp).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        items(shown, key = { it.id }) { root ->
                            val thread = listOf(root) + comments.orEmpty().filter { it.parentId == root.id }
                            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(c.fillNeutral).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                thread.forEach { cm ->
                                    Text(author(cm, a), color = c.textPrimary, fontSize = 13.sp, fontWeight = FontWeight.SemiBold)
                                    Text(cm.body, color = c.textPrimary, fontSize = 14.sp)
                                }
                                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                    if (canComment) TextButton(onClick = { replyTo = root }) { Text("Reply") }
                                    if (access == "edit" || access == "owner") {
                                        TextButton(onClick = { scope.launch { ArtifactsV2Client.resolveComment(a.id, root.id, root.resolvedAt == null); reload += 1 } }) {
                                            Text(if (root.resolvedAt == null) "Resolve" else "Reopen")
                                        }
                                    }
                                    if (access == "owner") TextButton(onClick = { confirmDelete = root }) { Text("Delete comment", color = AllternitStatic.StatusError) }
                                }
                            }
                        }
                    }
                }
            }
            if (canComment) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    replyTo?.let { r -> Text("Replying to ${author(r, a)}", color = c.textSecondary, fontSize = 12.sp, modifier = Modifier.clickable { replyTo = null }) }
                    LibraryField("", draft, if (access == "edit" || access == "owner") "Add a comment, or @gizzi to ask for a change" else "Add a comment", "artifactCommentInput", singleLine = false) { draft = it.take(4000) }
                    TextButton(
                        enabled = draft.isNotBlank(),
                        modifier = Modifier.testTag("artifactCommentSend"),
                        onClick = {
                            val body = draft.trim()
                            val parent = replyTo?.id
                            draft = ""; replyTo = null
                            scope.launch { ArtifactsV2Client.addComment(a.id, body, parent); reload += 1 }
                        },
                    ) { Text(if (replyTo != null) "Reply" else "Send", fontWeight = FontWeight.SemiBold) }
                }
            }
        }
    }
    confirmDelete?.let { cm ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text("Delete this thread?") },
            text = { Text("Its replies are deleted too.") },
            confirmButton = { TextButton(onClick = { confirmDelete = null; scope.launch { ArtifactsV2Client.deleteComment(a.id, cm.id); reload += 1 } }) { Text("Delete", color = AllternitStatic.StatusError) } },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("Cancel") } },
        )
    }
}

private fun author(cm: ArtifactComment, a: ArtifactV2): String = when {
    cm.agentName != null -> cm.agentName
    cm.authorId == a.summary.owner.id && a.summary.isMine -> "You"
    else -> cm.authorName ?: "Collaborator"
}

// MARK: - PDF from template (iOS PDFTemplateSheet + PDFTemplateRenderer)

internal enum class PdfTemplate(val raw: String, val label: String, val variants: List<String>) {
    Invoice("invoice", "Invoice", listOf("classic", "minimal", "modern")),
    Quote("quote", "Quote", listOf("classic", "minimal", "modern")),
    Contract("contract", "Contract", listOf("classic", "minimal")),
    Report("report", "Report", listOf("classic", "minimal", "modern")),
    Ticket("ticket", "Event ticket", listOf("classic", "minimal", "modern")),
    Minutes("minutes", "Meeting minutes", listOf("classic", "minimal")),
    Receipt("receipt", "Receipt", listOf("minimal", "classic", "modern"));

    /** The iOS PDFTemplate.sample, used when the artifact body isn't a JSON object. */
    fun sample(): JsonObject = when (this) {
        Invoice -> buildJsonObject {
            put("number", "INV-1042"); put("issued", "Nov 9, 2026"); put("due", "Dec 9, 2026"); put("taxRate", 8.25)
            putJsonObject("from") { put("name", "Northwind Studio"); putJsonArray("lines") { add(JsonPrimitive("410 Market Street")); add(JsonPrimitive("Austin, TX 78701")) } }
            putJsonObject("to") { put("name", "Harbor Coffee Co."); putJsonArray("lines") { add(JsonPrimitive("22 Pier Road")); add(JsonPrimitive("Galveston, TX 77550")) } }
            putJsonArray("items") {
                add(buildJsonObject { put("description", "Brand audit"); put("qty", 1); put("unitPrice", 1800) })
                add(buildJsonObject { put("description", "Menu design"); put("qty", 3); put("unitPrice", 240) })
            }
        }
        Quote -> buildJsonObject {
            put("number", "Q-2051"); put("issued", "Nov 26, 2026"); put("validUntil", "Dec 26, 2026"); put("currency", "USD"); put("taxRate", 8.25)
            putJsonObject("from") { put("name", "Northwind Studio"); putJsonArray("lines") { add(JsonPrimitive("410 Market Street")); add(JsonPrimitive("Austin, TX 78701")) } }
            putJsonObject("to") { put("name", "Harbor Coffee Co."); putJsonArray("lines") { add(JsonPrimitive("22 Pier Road")); add(JsonPrimitive("Galveston, TX 77550")) } }
            putJsonArray("items") {
                add(buildJsonObject { put("description", "Website design"); put("detail", "Five pages, two rounds of changes"); put("qty", 15); put("unitPrice", 100) })
                add(buildJsonObject { put("description", "Website development"); put("qty", 10); put("unitPrice", 100) })
                add(buildJsonObject { put("description", "Copywriting"); put("qty", 5); put("unitPrice", 50) })
            }
            put("scope", "Design and build of the new Harbor Coffee website, from wireframes to launch.")
            put("terms", "50% due on acceptance, 50% on launch. Work starts within five business days of acceptance.")
        }
        Contract -> buildJsonObject {
            put("title", "Services agreement"); put("number", "SA-118"); put("effective", "Dec 1, 2026"); put("term", "6 months")
            putJsonArray("parties") {
                add(buildJsonObject { put("id", "provider"); put("role", "Service provider"); put("name", "Northwind Studio"); putJsonArray("lines") { add(JsonPrimitive("410 Market Street")); add(JsonPrimitive("Austin, TX 78701")) } })
                add(buildJsonObject { put("id", "client"); put("role", "Client"); put("name", "Harbor Coffee Co."); putJsonArray("lines") { add(JsonPrimitive("22 Pier Road")); add(JsonPrimitive("Galveston, TX 77550")) } })
            }
            put("intro", "This agreement sets out how Northwind Studio will provide design services to Harbor Coffee Co.")
            putJsonArray("clauses") {
                add(buildJsonObject { put("heading", "Services"); put("body", "Northwind Studio will design and build the website described in quote Q-2051.") })
                add(buildJsonObject { put("heading", "Fees and payment"); put("body", "The client pays the fees in quote Q-2051. Invoices are due within 30 days.") })
                add(buildJsonObject { put("heading", "Ownership"); put("body", "The client owns the finished work once it is paid for in full.") })
                add(buildJsonObject { put("heading", "Ending the agreement"); put("body", "Either party may end this agreement with 14 days written notice. Work done until then is paid for.") })
            }
        }
        Report -> buildJsonObject {
            put("title", "Quarterly operations report"); put("subtitle", "Q3 2026"); put("org", "Northwind Studio")
            putJsonArray("metrics") {
                add(buildJsonObject { put("label", "Revenue"); put("value", "$184,200") })
                add(buildJsonObject { put("label", "Active clients"); put("value", "27") })
            }
            put("summary", "Revenue grew for the third quarter in a row.")
        }
        Ticket -> buildJsonObject { put("event", "Gulf Coast Makers Night"); put("holder", "Jordan Reyes"); put("date", "Sat, Dec 12, 2026"); put("venue", "The Pier Hall"); put("code", "HAC-88213-7F2K") }
        Minutes -> buildJsonObject {
            put("title", "Product sync"); put("date", "Nov 9, 2026")
            put("attendees", buildJsonArray { add(JsonPrimitive("Avery Park")); add(JsonPrimitive("Sam Okafor")) })
            putJsonArray("actions") { add(buildJsonObject { put("task", "Update the release calendar"); put("owner", "Sam"); put("due", "Nov 11") }) }
        }
        Receipt -> buildJsonObject {
            put("number", "R-30991"); put("date", "Nov 9, 2026"); put("merchant", "Harbor Coffee Co."); put("tax", 1.05)
            putJsonArray("items") { add(buildJsonObject { put("name", "Flat white"); put("qty", 2); put("unitPrice", 4.5) }) }
        }
    }
}

private val pdfThemes = listOf("professional", "minimal", "executive")

/**
 * iOS PDFTemplateSheet: Template, Look (theme, header variant, paper Letter / A4) and Make PDF.
 * The shared layout (pdf-templates.js, copied from the iOS bundle into this module's assets) runs
 * in an offscreen WebView instead of JavaScriptCore, and Android's print dialog makes the PDF
 * ("Save as PDF"), the Android form of the iOS share sheet. No server, works offline.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun PDFTemplateSheet(a: ArtifactV2, onDismiss: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var template by remember { mutableStateOf(PdfTemplate.Invoice) }
    var theme by remember { mutableStateOf("professional") }
    var variant by remember(template) { mutableStateOf(template.variants.first()) }
    var paper by remember { mutableStateOf(if (Locale.getDefault().country in setOf("US", "CA", "MX", "PH", "")) "letter" else "a4") }
    var making by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val data = remember(a.id, a.version) { LibraryJson.parse(a.body)?.takeIf { it.isNotEmpty() } }

    LibrarySheet(
        "PDF from template", "pdfTemplateSheet", onDismiss = onDismiss,
        leading = LibrarySheetAction("Close", testTag = "pdfTemplateClose", onClick = onDismiss),
        trailing = LibrarySheetAction(if (making) "Making…" else "Make PDF", enabled = !making, bold = true, testTag = "pdfTemplateMake") {
            making = true
            error = null
            scope.launch {
                val request = buildJsonObject {
                    put("template", template.raw); put("theme", theme); put("pageSize", paper); put("variant", variant)
                    put("data", data ?: template.sample())
                }
                val result = renderTemplateHtml(context, request.toString())
                making = false
                result.onSuccess { html -> printHtml(context, html, a.title.ifBlank { template.label }, paper) }
                    .onFailure { error = it.message ?: "Couldn't make the PDF. Try again." }
            }
        },
    ) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState())) {
            FilterLabel("Template")
            PillRow(PdfTemplate.entries, template, { it.label }, { "pdfTemplate${it.raw}" }) { template = it }
            FilterLabel("Look")
            PillRow(pdfThemes, theme, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "pdfTheme$it" }) { theme = it }
            Box(Modifier.padding(top = 8.dp)) { PillRow(template.variants, variant, { it.replaceFirstChar { ch -> ch.uppercase() } }, { "pdfVariant$it" }) { variant = it } }
            Box(Modifier.padding(top = 8.dp)) { PillRow(listOf("letter", "a4"), paper, { if (it == "letter") "Letter" else "A4" }, { "pdfPaper$it" }) { paper = it } }
            Text(
                if (data == null) "This artifact has no JSON data, so the template's sample is used. Edit the data on the web or Desktop." else "Uses the data in this artifact.",
                color = c.textSecondary, fontSize = 13.sp, modifier = Modifier.padding(top = 14.dp),
            )
            error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(top = 8.dp)) }
        }
    }
}

/** Runs `allternitPdfHtml(json)` from the bundled layout in an offscreen WebView; adds iOS's paged CSS. */
@SuppressLint("SetJavaScriptEnabled")
private suspend fun renderTemplateHtml(context: android.content.Context, requestJson: String): Result<String> {
    val source = withContext(Dispatchers.IO) { runCatching { context.assets.open("pdf-templates.js").bufferedReader().use { it.readText() } }.getOrNull() }
        ?: return Result.failure(IllegalStateException("The PDF templates are missing from this build."))
    return suspendCancellableCoroutine { cont ->
        val web = WebView(context)
        web.settings.javaScriptEnabled = true
        web.settings.allowFileAccess = false
        web.settings.allowContentAccess = false
        web.webViewClient = object : WebViewClient() {
            override fun onPageFinished(view: WebView, url: String?) {
                val script = "(function(){try{return {ok:true,html:allternitPdfHtml(${ArtifactHTML.jsString(requestJson)})};}catch(e){return {ok:false,error:String(e&&e.message||e)};}})()"
                view.evaluateJavascript(script) { raw ->
                    val o = LibraryJson.parse(raw ?: "")
                    view.destroy()
                    if (!cont.isActive) return@evaluateJavascript
                    val html = o.str("html")
                    if (o.bool("ok") == true && html != null) {
                        val css = "<style>.page{width:auto!important;min-height:0!important;padding:0!important}.mast-modern{margin:0 0 24pt!important;padding:24pt 20pt 18pt!important}</style>"
                        cont.resume(Result.success(html.replace("</head>", "$css</head>")))
                    } else {
                        cont.resume(Result.failure(IllegalStateException(o.str("error") ?: "Couldn't make the PDF. Try again.")))
                    }
                }
            }
        }
        web.loadDataWithBaseURL(null, "<!doctype html><html><head><script>$source</script></head><body></body></html>", "text/html", "utf-8", null)
        cont.invokeOnCancellation { web.post { web.destroy() } }
    }
}

/** Lays the page out in an offscreen WebView and hands it to the print dialog (Save as PDF). */
private fun printHtml(context: android.content.Context, html: String, title: String, paper: String) {
    val web = WebView(context)
    web.settings.javaScriptEnabled = false
    web.settings.allowFileAccess = false
    web.webViewClient = object : WebViewClient() {
        override fun onPageFinished(view: WebView, url: String?) {
            val pm = context.getSystemService(android.content.Context.PRINT_SERVICE) as? PrintManager ?: return
            val size = if (paper == "a4") PrintAttributes.MediaSize.ISO_A4 else PrintAttributes.MediaSize.NA_LETTER
            pm.print(title, view.createPrintDocumentAdapter(title), PrintAttributes.Builder().setMediaSize(size).build())
        }
    }
    web.loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
}

// MARK: - Chat-frame artifact details (iOS ArtifactDetailsView)

/** A chat card's artifact (iOS ArtifactRecord). */
data class ArtifactRecordInput(val id: String, val kind: String, val title: String, val url: String?, val content: String?)

private sealed interface DetailsLoad {
    data object Loading : DetailsLoad
    data class Ready(val text: String) : DetailsLoad
    data class Failed(val message: String) : DetailsLoad
}

/**
 * iOS ArtifactDetailsView. An `art_` id opens the V2 viewer; other chat-frame records load their
 * content (url → inline → the v2 body) and show Preview (the sandboxed web view, html / svg / css
 * / js) or Code, with Back, Share and More (Open in browser, Copy content).
 */
@Composable
fun ArtifactDetailsScreen(record: ArtifactRecordInput, onClose: () -> Unit, onOpenChat: (String) -> Unit, onSignIn: () -> Unit) {
    if (record.id.startsWith("art_")) {
        V2ArtifactScreen(record.id, onClose = onClose, onOpenChat = onOpenChat, onSignIn = onSignIn)
        return
    }
    val c = AllternitTheme.colors
    val context = LocalContext.current
    var load by remember(record.id) { mutableStateOf<DetailsLoad>(DetailsLoad.Loading) }
    var retry by remember { mutableIntStateOf(0) }
    val kind = record.kind.lowercase()
    val previewable = kind.contains("html") || kind.contains("svg") || kind == "css" || kind.contains("javascript") || kind == "js"
    var preview by remember { mutableStateOf(previewable) }
    var moreOpen by remember { mutableStateOf(false) }
    BackHandler(onBack = onClose)
    LaunchedEffect(record.id, retry) {
        load = DetailsLoad.Loading
        load = loadRecordContent(record)
    }
    val https = record.url?.takeIf { it.startsWith("https://") }
    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("artifactDetails")) {
        LibraryHeader(
            title = record.title,
            onBack = onClose,
            backTag = "artifactDetailsBack",
            trailing = {
                LibraryIconButton(DesignR.drawable.ph_upload_simple, "Share", "artifactDetailsShare", iconSize = 20) {
                    val text = (load as? DetailsLoad.Ready)?.text ?: https ?: return@LibraryIconButton
                    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_SUBJECT, record.title).putExtra(Intent.EXTRA_TEXT, text)
                    context.startActivity(Intent.createChooser(send, "Share").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                }
                LibraryIconButton(DesignR.drawable.ph_dots_three_vertical, "More", "artifactDetailsMore", iconSize = 20) { moreOpen = true }
            },
        )
        if (previewable) {
            PillRow(listOf(true, false), preview, { if (it) "Preview" else "Code" }, { if (it) "artifactDetailsPreview" else "artifactDetailsCode" }, Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) { preview = it }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (val l = load) {
                DetailsLoad.Loading -> LibrarySpinner("Loading artifact…")
                is DetailsLoad.Failed -> FriendlyState(FriendlyStyle.Error, "Couldn't load this artifact", l.message, actionTitle = "Retry", onAction = { retry += 1 }) {
                    Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
                }
                is DetailsLoad.Ready -> when {
                    l.text.isBlank() -> FriendlyState(FriendlyStyle.Empty, "This artifact is empty", "Nothing was captured for this artifact.", art = com.allternit.core.design.EmptyArtKind.Sheets, icon = {})
                    previewable && preview -> SandboxedArtifactWebView(l.text, kind, Modifier.fillMaxSize())
                    else -> SelectionContainer(Modifier.fillMaxSize()) {
                        Text(
                            l.text, color = c.textPrimary, fontSize = 12.5.sp, lineHeight = 18.sp, fontFamily = FontFamily.Monospace,
                            modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState()).padding(16.dp),
                        )
                    }
                }
            }
        }
    }
    if (moreOpen) {
        LibrarySheet(record.title, "artifactDetailsMoreSheet", onDismiss = { moreOpen = false }) {
            Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (https != null) LibraryRowButton("Open in browser", DesignR.drawable.ph_globe, "artifactDetailsOpenBrowser") { moreOpen = false; openExternal(context, https) }
                LibraryRowButton("Copy content", DesignR.drawable.ph_copy, "artifactDetailsCopy", enabled = load is DetailsLoad.Ready) {
                    moreOpen = false
                    (load as? DetailsLoad.Ready)?.let { copyText(context, record.title, it.text) }
                }
            }
        }
    }
}

/** iOS ArtifactContentLoader: url (http(s) without the bearer, data:, a gateway path with it), then inline, then the v2 body. */
private suspend fun loadRecordContent(r: ArtifactRecordInput): DetailsLoad = withContext(Dispatchers.IO) {
    val url = r.url
    if (url != null) {
        val text = runCatching {
            when {
                url.startsWith("data:") -> {
                    val payload = url.substringAfter(',')
                    if (url.substringBefore(',').endsWith(";base64")) String(android.util.Base64.decode(payload, android.util.Base64.DEFAULT)) else java.net.URLDecoder.decode(payload, "UTF-8")
                }
                url.startsWith("https://") || url.startsWith("http://") ->
                    okhttp3.OkHttpClient().newCall(okhttp3.Request.Builder().url(url).build()).execute().use { if (it.isSuccessful) it.body?.string() else null }
                url.startsWith("/api/") ->
                    LibraryNet.okHttp.newCall(okhttp3.Request.Builder().url(LibraryNet.absolute(url)).build()).execute().use { if (it.isSuccessful) it.body?.string() else null }
                else -> null
            }
        }.getOrNull()
        if (text != null) return@withContext DetailsLoad.Ready(text)
    }
    r.content?.let { return@withContext DetailsLoad.Ready(it) }
    if (r.id.startsWith("art_")) {
        val got = ArtifactsV2Client.get(r.id)
        (got as? ApiResult.Ok)?.value?.let { return@withContext DetailsLoad.Ready(it.body) }
    }
    if (url != null) DetailsLoad.Failed("The artifact URL is invalid.") else DetailsLoad.Failed("The artifact event carried no content, URL, or fetchable id.")
}
