package com.allternit.feature.code

import android.util.Base64
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitStatic
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.artifacts.LibraryIconButton
import com.allternit.feature.chat.ChatBackend
import kotlinx.coroutines.launch
import com.allternit.core.design.R as DesignR

/*
 * The file browser (iOS FileBrowserView, FileBrowserDirectoryView, FileDetailView) and the editor
 * (iOS CodeEditorView, which is Runestone there). Browsing walks `v1/file/tree` one level at a
 * time; a file opens read-only with its uncommitted diff, and Edit opens the editor.
 */

/** The Files pane: the working folder's root, drilling into directories and files. */
@Composable
internal fun FileBrowserScreen(backend: ChatBackend, folder: CodeFolder?, onBack: () -> Unit) {
    val stack = remember { mutableStateListOf<String>() }
    var file by remember { mutableStateOf<FileNode?>(null) }
    var editing by remember { mutableStateOf<FileNode?>(null) }
    val open = file
    val edit = editing
    when {
        edit != null -> CodeEditorScreen(backend, edit, onBack = { editing = null })
        open != null -> FileDetailScreen(backend, open, onBack = { file = null }, onEdit = { editing = open })
        else -> {
            val path = stack.lastOrNull()
            val title = if (path == null) (folder?.name ?: "Files") else path.substringAfterLast('/')
            CodeScaffold(
                title = title,
                subtitle = if (path == null) folder?.branch?.let { "on $it" } else path,
                onBack = { if (stack.isNotEmpty()) stack.removeAt(stack.lastIndex) else onBack() },
                backTag = "codeFilesBack",
            ) {
                FileBrowserDirectory(backend, path, onDirectory = { stack.add(it.path) }, onFile = { file = it })
            }
        }
    }
}

/** One directory level (iOS FileBrowserDirectoryView). */
@Composable
internal fun FileBrowserDirectory(backend: ChatBackend, path: String?, onDirectory: (FileNode) -> Unit, onFile: (FileNode) -> Unit) {
    val c = AllternitTheme.colors
    CodeLoader(key = path ?: "", failedTitle = "Couldn't load files", load = { CodeApi.tree(backend, path) }) { nodes, reload ->
        if (nodes.isEmpty()) {
            EmptyState("Empty folder", "There is nothing in this folder yet.", art = com.allternit.core.design.EmptyArtKind.Sheets)
        } else {
            val sorted = nodes.sortedWith(compareByDescending<FileNode> { it.isDirectory }.thenBy { it.name.lowercase() })
            LazyColumn(Modifier.fillMaxSize().testTag("codeFileList")) {
                items(sorted, key = { it.path }) { node ->
                    FileNodeRow(node) { if (node.isDirectory) onDirectory(node) else onFile(node) }
                }
            }
        }
    }
}

/** iOS FileNodeRow: a folder (›) or file glyph, the name, ignored entries dimmed. */
@Composable
private fun FileNodeRow(node: FileNode, onClick: () -> Unit) {
    val c = AllternitTheme.colors
    CodeRow(
        title = node.name,
        testTag = "codeFileRow-${node.name}",
        mono = true,
        dim = node.ignored,
        leading = { Text(if (node.isDirectory) "▸" else "·", color = c.textSecondary, fontSize = 16.sp, modifier = Modifier.width(18.dp)) },
        trailing = if (node.isDirectory) ({ Text("›", color = c.textTertiary, fontSize = 18.sp) }) else null,
        onClick = onClick,
    )
}

/** A file's text with line numbers; a changed file offers its uncommitted diff (iOS FileDetailView). */
@Composable
internal fun FileDetailScreen(backend: ChatBackend, node: FileNode, onBack: () -> Unit, onEdit: () -> Unit) {
    val c = AllternitTheme.colors
    var showDiff by remember { mutableStateOf(false) }
    CodeScaffold(
        title = node.name,
        subtitle = node.path,
        onBack = onBack,
        backTag = "codeFileBack",
    ) {
        CodeLoader(key = node.path, failedTitle = "Couldn't load file", load = { CodeApi.read(backend, node.path) }) { read, _ ->
            Column(Modifier.fillMaxSize()) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (!read.diff.isNullOrEmpty()) {
                        CodePill(if (showDiff) "File" else "Modified", testTag = "codeFileModified") { showDiff = !showDiff }
                    }
                    if (!read.binary) CodePill("Edit", testTag = "codeFileEdit", onClick = onEdit)
                }
                when {
                    read.binary -> Column(Modifier.fillMaxSize().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically)) {
                        Text("Binary file", color = c.textPrimary, fontSize = 17.sp)
                        read.mimeType?.let { Text(it, color = c.textSecondary, fontSize = 13.sp, fontFamily = CodeMono) }
                    }
                    showDiff && read.diff != null -> DiffView(DiffLine.parse(read.diff), Modifier.weight(1f))
                    else -> {
                        val text = if (read.encoding == "base64") String(Base64.decode(read.content, Base64.DEFAULT)) else read.content
                        CodeText(text, node.path, Modifier.weight(1f))
                    }
                }
            }
        }
    }
}

/** Read-only code: a line-number gutter and the highlighted text, scrolling both ways. */
@Composable
internal fun CodeText(text: String, path: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val lines = remember(text) { text.split("\n").size }
    val styled = remember(text, path) {
        highlight(text, languageOf(path), c.accentCode, AllternitStatic.StatusSuccess, c.textTertiary, AllternitStatic.StatusWarning)
    }
    Row(modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(top = 4.dp, bottom = 24.dp).testTag("codeFileText")) {
        Text(
            (1..lines).joinToString("\n"), color = c.textTertiary, fontSize = 12.sp, fontFamily = CodeMono, lineHeight = 18.sp,
            textAlign = androidx.compose.ui.text.style.TextAlign.End, modifier = Modifier.width(44.dp).padding(end = 8.dp),
        )
        Text(styled, color = c.textPrimary, fontSize = 12.sp, fontFamily = CodeMono, lineHeight = 18.sp, softWrap = false, modifier = Modifier.horizontalScroll(rememberScrollState()))
    }
}

/** The editor (iOS CodeEditorView): a line-numbered, highlighted text field with Save. */
@Composable
internal fun CodeEditorScreen(backend: ChatBackend, node: FileNode, onBack: () -> Unit) {
    val c = AllternitTheme.colors
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf<String?>(null) }
    var original by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var noComputer by remember { mutableStateOf(false) }
    val language = remember(node.path) { languageOf(node.path) }
    val dirty = text != null && text != original
    CodeScaffold(
        title = node.name,
        subtitle = if (dirty) "Edited" else node.path,
        onBack = onBack,
        backTag = "codeEditorBack",
        trailing = {
            CodePill("Save", testTag = "codeEditorSave", filled = true, enabled = dirty && !saving) {
                val t = text ?: return@CodePill
                scope.launch {
                    saving = true
                    error = null
                    when (val r = CodeApi.write(backend, node.absolute.ifEmpty { node.path }, t)) {
                        is CodeResult.Ok -> original = t
                        CodeResult.NoComputer -> noComputer = true
                        is CodeResult.Failed -> error = r.message
                    }
                    saving = false
                }
            }
        },
    ) {
        CodeLoader(key = node.path, failedTitle = "Couldn't load file", load = { CodeApi.read(backend, node.path) }) { read, _ ->
            if (text == null) {
                val loaded = if (read.encoding == "base64") String(Base64.decode(read.content, Base64.DEFAULT)) else read.content
                text = loaded
                original = loaded
            }
            val current = text.orEmpty()
            val accent = c.accentCode
            val transform = remember(language, accent) {
                VisualTransformation { raw ->
                    TransformedText(highlight(raw.text, language, accent, AllternitStatic.StatusSuccess, c.textTertiary, AllternitStatic.StatusWarning), OffsetMapping.Identity)
                }
            }
            Column(Modifier.fillMaxSize().imePadding()) {
                if (noComputer) Text("Connect a computer to save.", color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
                error?.let { Text(it, color = AllternitStatic.StatusError, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp).testTag("codeEditorError")) }
                Row(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
                    Text(
                        (1..current.split("\n").size).joinToString("\n"), color = c.textTertiary, fontSize = 12.sp, fontFamily = CodeMono, lineHeight = 18.sp,
                        textAlign = androidx.compose.ui.text.style.TextAlign.End, modifier = Modifier.width(44.dp).padding(end = 8.dp),
                    )
                    BasicTextField(
                        value = current,
                        onValueChange = { text = it; noComputer = false },
                        textStyle = TextStyle(color = c.textPrimary, fontSize = 12.sp, fontFamily = CodeMono, lineHeight = 18.sp),
                        cursorBrush = SolidColor(c.accentCode),
                        visualTransformation = transform,
                        modifier = Modifier.fillMaxWidth().testTag("codeEditorField"),
                    )
                }
            }
        }
    }
}
