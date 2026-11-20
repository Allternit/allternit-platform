package com.allternit.feature.office

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Color as AndroidColor
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.FriendlyState
import com.allternit.core.design.FriendlyStyle
import com.allternit.core.design.R as DesignR
import com.allternit.feature.artifacts.LibraryHeader
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.artifacts.LibrarySpinner
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private enum class LocalType { Pdf, Markdown, PlainText, Word, Excel, Unknown }

private fun displayName(context: Context, uri: Uri): String =
    runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull() ?: uri.lastPathSegment?.substringAfterLast('/') ?: "Document"

private fun typeOf(context: Context, uri: Uri, name: String): LocalType {
    val ext = name.substringAfterLast('.', "").lowercase().ifEmpty { MimeTypeMap.getSingleton().getExtensionFromMimeType(context.contentResolver.getType(uri)).orEmpty() }
    return when (ext) {
        "pdf" -> LocalType.Pdf
        "md", "markdown" -> LocalType.Markdown
        "txt", "text" -> LocalType.PlainText
        "doc", "docx" -> LocalType.Word
        "xls", "xlsx", "csv" -> LocalType.Excel
        else -> LocalType.Unknown
    }
}

/** Every page of a PDF at the given width in px (iOS PDFView, a scrolling page list). */
private fun renderPdf(context: Context, uri: Uri, widthPx: Int): List<Bitmap>? {
    val file = File.createTempFile("local-doc", ".pdf", context.cacheDir)
    return try {
        context.contentResolver.openInputStream(uri)?.use { input -> file.outputStream().use { input.copyTo(it) } } ?: return null
        ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY).use { fd ->
            PdfRenderer(fd).use { r ->
                (0 until r.pageCount.coerceAtMost(60)).map { i ->
                    r.openPage(i).use { p ->
                        val h = (widthPx.toFloat() / p.width * p.height).toInt().coerceAtLeast(1)
                        Bitmap.createBitmap(widthPx, h, Bitmap.Config.ARGB_8888).also { it.eraseColor(AndroidColor.WHITE); p.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY) }
                    }
                }
            }
        }
    } catch (_: Exception) {
        null
    } finally {
        file.delete()
    }
}

private sealed interface LocalState {
    data object Loading : LocalState
    data class Failed(val message: String) : LocalState
    data class Pdf(val pages: List<Bitmap>) : LocalState
    data class Text(val text: String) : LocalState
    data object External : LocalState
}

/**
 * A locally-shared office file opened inside the Allternit app (iOS LocalDocumentView): PDFs render page by page,
 * Markdown and text as selectable text, Word and Excel hand off to the system viewer (iOS QuickLook). Share in the header.
 */
@Composable
fun LocalDocumentScreen(uri: Uri, onClose: () -> Unit) {
    val c = AllternitTheme.colors
    val context = LocalContext.current
    val name = remember(uri) { displayName(context, uri) }
    val type = remember(uri) { typeOf(context, uri, name) }
    var state by remember(uri) { mutableStateOf<LocalState>(LocalState.Loading) }
    var attempt by remember { mutableIntStateOf(0) }
    BackHandler(onBack = onClose)

    LaunchedEffect(uri, attempt) {
        state = LocalState.Loading
        state = withContext(Dispatchers.IO) {
            when (type) {
                LocalType.Pdf -> renderPdf(context, uri, 1080)?.takeIf { it.isNotEmpty() }?.let { LocalState.Pdf(it) } ?: LocalState.Failed("The file could not be rendered.")
                LocalType.Markdown, LocalType.PlainText ->
                    runCatching { context.contentResolver.openInputStream(uri)?.use { String(it.readBytes(), Charsets.UTF_8) } }.getOrNull()?.let { LocalState.Text(it) }
                        ?: LocalState.Failed("The file could not be read.")
                else -> LocalState.External
            }
        }
    }

    Column(Modifier.fillMaxSize().background(c.bgPrimary).statusBarsPadding().testTag("localDocumentScreen")) {
        LibraryHeader(
            title = name, onBack = onClose, backTag = "localDocumentClose",
            trailing = {
                LibraryIconButton(DesignR.drawable.ph_upload_simple, "Share", "localDocumentShare", iconSize = 20) {
                    val send = Intent(Intent.ACTION_SEND).setType(context.contentResolver.getType(uri) ?: "*/*")
                        .putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    context.startActivity(Intent.createChooser(send, "Share").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                }
            },
        )
        when (val s = state) {
            LocalState.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LibrarySpinner("Opening…") }
            is LocalState.Failed -> FriendlyState(FriendlyStyle.Error, "Couldn't open document", s.message, actionTitle = "Try Again", onAction = { attempt += 1 }) {
                Icon(painterResource(DesignR.drawable.ph_warning), null, tint = c.textSecondary)
            }
            is LocalState.Pdf -> LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                itemsIndexed(s.pages) { i, bmp -> Image(bmp.asImageBitmap(), "Page ${i + 1}", Modifier.fillMaxWidth(), contentScale = ContentScale.FillWidth) }
            }
            is LocalState.Text -> SelectionContainer {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(20.dp)) { Text(s.text, color = c.textPrimary, fontSize = 16.sp) }
            }
            LocalState.External -> FriendlyState(
                FriendlyStyle.Empty, "Open in another app", "Word and Excel files open in your phone's document viewer.",
                actionTitle = "Open",
                onAction = {
                    val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, context.contentResolver.getType(uri) ?: "*/*").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                    runCatching { context.startActivity(Intent.createChooser(view, "Open with").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
                },
            ) { Icon(painterResource(DesignR.drawable.deck_docs), null, tint = c.accentPrimary) }
        }
    }
}
