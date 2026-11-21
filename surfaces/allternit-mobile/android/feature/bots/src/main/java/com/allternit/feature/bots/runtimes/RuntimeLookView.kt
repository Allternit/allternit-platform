package com.allternit.feature.bots.runtimes

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.invisibleToUser
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.feature.chat.MessageRecord
import com.allternit.feature.chat.ToolCallRecord
import com.allternit.feature.chat.ToolState
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/*
 * Runtime view (plan D3/D9; web `fromTokens` in pack-kit.tsx is the reference renderer): one
 * data-driven view that draws a runtime's registry look tokens. `kind: tui` is the terminal screen
 * (banner, user prompt glyph, step lines with {a}/{r} slots, answer glyph, the input style and the
 * footer); `kind: app` is the desktop app (header with status, a steps card, bubbles, the composer).
 * Light and dark follow the app theme: the native palette as drawn, the other from `light`/`dark`.
 */

/** One step of a reply: verb template, argument, result, status. */
data class LookStep(val verb: String, val a: String, val r: String, val status: String)

sealed interface LookItem {
    data class User(val id: String, val text: String) : LookItem
    data class Agent(val id: String, val text: String, val streaming: Boolean) : LookItem
    data class Steps(val id: String, val steps: List<LookStep>) : LookItem
}

private data class Pal(val bg: Color, val fg: Color, val mu: Color, val ac: Color, val ac2: Color, val ok: Color, val err: Color)

private fun hex(s: String?, fallback: Color): Color {
    val h = s?.trim()?.removePrefix("#") ?: return fallback
    val full = when (h.length) { 3 -> h.map { "$it$it" }.joinToString(""); 6 -> h; else -> return fallback }
    return full.toLongOrNull(16)?.let { Color(0xFF000000 or it) } ?: fallback
}

private val GENERIC_LIGHT = mapOf("bg" to "#fbfbfa", "fg" to "#1d1d1b", "mu" to "#74746f", "ok" to "#1a7f37", "err" to "#cf222e")
private val GENERIC_DARK = mapOf("bg" to "#16181d", "fg" to "#e5e7eb", "mu" to "#9ca3af", "ok" to "#3fb950", "err" to "#f85149")

/** web `tokenPalettes`: the native palette as drawn, the other from `light`/`dark` tokens or a neutral base. */
private fun palettes(tk: LookTokens): Pair<Pal, Pal> {
    val base = mapOf("bg" to tk.bg, "fg" to tk.fg, "mu" to tk.mu, "ac" to tk.ac, "ac2" to (tk.ac2 ?: tk.ac), "ok" to (tk.ok ?: "#3fb950"), "err" to (tk.err ?: "#f85149"))
    fun pal(m: Map<String, String>) = Pal(
        hex(m["bg"], Color.Black), hex(m["fg"], Color.White), hex(m["mu"], Color.Gray), hex(m["ac"], Color.White),
        hex(m["ac2"], hex(m["ac"], Color.White)), hex(m["ok"], Color(0xFF3FB950)), hex(m["err"], Color(0xFFF85149)),
    )
    // An accent that disappears on the other background falls back to the text color.
    fun fit(p: Pal): Pal {
        val lightBg = p.bg.luminance() > 0.5f
        fun fix(c: Color) = if (if (lightBg) c.luminance() > 0.6f else c.luminance() < 0.03f) p.fg else c
        return p.copy(ac = fix(p.ac), ac2 = fix(p.ac2))
    }
    val nativeDark = tk.darkFlag == true || (tk.darkFlag != false && tk.darkPalette.isEmpty() && hex(tk.bg, Color.Black).luminance() < 0.4f)
    return if (nativeDark) {
        val l = tk.light
        val light = base + GENERIC_LIGHT + l + mapOf("ac" to (l["ac"] ?: tk.ac), "ac2" to (l["ac2"] ?: if (tk.ac2 != null) tk.ac2 else l["ac"] ?: tk.ac))
        fit(pal(light)) to pal(base)
    } else {
        val d = tk.darkPalette
        val dark = base + GENERIC_DARK + d + mapOf("ac" to (d["ac"] ?: tk.ac), "ac2" to (d["ac2"] ?: if (tk.ac2 != null) tk.ac2 else d["ac"] ?: tk.ac))
        pal(base) to fit(pal(dark))
    }
}

/** web `stepVerb`: which template a tool call uses, from its name. */
fun stepVerb(name: String): String {
    val n = name.lowercase()
    return when {
        Regex("edit|write|patch|replace|update|create|apply|insert|delete|move|rename").containsMatchIn(n) -> "Edit"
        Regex("read|view|cat|open|grep|glob|search|find|list|ls\\b|fetch|explore|look").containsMatchIn(n) -> "Read"
        else -> "Run"
    }
}

private fun oneLine(s: String?, max: Int = 80): String {
    val t = s?.lineSequence()?.firstOrNull { it.isNotBlank() }?.trim().orEmpty()
    return if (t.length > max) t.take(max - 1) + "…" else t
}

private fun JsonObject?.firstString(keys: List<String>): String? =
    keys.firstNotNullOfOrNull { k -> (this?.get(k) as? JsonPrimitive)?.contentOrNull?.takeIf { it.isNotEmpty() } }

/** web `toStep` for a chat tool call. */
fun ToolCallRecord.toLookStep(): LookStep {
    val status = when (state) { ToolState.Running -> "running"; ToolState.Failed -> "error"; ToolState.Done -> "success" }
    val a = oneLine(input.firstString(listOf("path", "file_path", "filePath", "file", "command", "cmd", "pattern", "query", "url")) ?: title)
    val r = if (status == "error") "failed" else oneLine(output)
    return LookStep(stepVerb(toolName), a.ifEmpty { toolName }, r.ifEmpty { if (status == "running") "" else "done" }, status)
}

/** A chat's messages as look items: user prompts, each reply's steps (before its answer), answers. */
fun lookItems(messages: List<MessageRecord>): List<LookItem> = buildList {
    for (m in messages) {
        if (m.role == "user") { add(LookItem.User(m.id, m.content)); continue }
        if (m.toolCalls.isNotEmpty()) add(LookItem.Steps("${m.id}-steps", m.toolCalls.map { it.toLookStep() }))
        if (m.content.isNotBlank() || m.isStreaming) add(LookItem.Agent(m.id, m.content, m.isStreaming))
    }
}

private fun fill(text: String, step: LookStep?, name: String) =
    text.replace("{a}", step?.a ?: "").replace("{r}", step?.r ?: "").replace("{name}", name)

/**
 * The runtime's look. [items] is the conversation; [onSend] sends from the look's own input.
 * [liveAppLabel] (a runtime with a live web app) adds the "open it on your computer" line, since
 * the phone has no frame for the app itself (plan D9).
 */
@Composable
fun RuntimeLookView(
    runtime: RuntimeEntry,
    look: String?,
    items: List<LookItem>,
    isStreaming: Boolean,
    onSend: (String) -> Unit,
    onStop: () -> Unit,
    modifier: Modifier = Modifier,
    botName: String? = null,
) {
    val tk = runtime.tokens(look)
    val appDark = AllternitTheme.colors.bgPrimary.luminance() < 0.4f
    if (tk == null) {
        // The native runtime has no look of its own: Runtime view is the Allternit chat.
        Box(modifier.fillMaxSize().padding(24.dp), contentAlignment = Alignment.Center) {
            Text("${runtime.name} has no separate look. Its work shows in the Allternit view.", color = AllternitTheme.colors.textSecondary, fontSize = 14.sp)
        }
        return
    }
    val (light, dark) = remember(tk) { palettes(tk) }
    val p = if (appDark) dark else light
    val app = tk.isApp
    val font = if (app) FontFamily.Default else FontFamily.Monospace
    val base = TextStyle(color = p.fg, fontSize = if (app) 14.sp else 13.sp, fontFamily = font, lineHeight = if (app) 21.sp else 19.sp)
    fun role(r: String?): Color = when (r) { "mu" -> p.mu; "ac" -> p.ac; "ac2" -> p.ac2; "ok" -> p.ok; "err" -> p.err; else -> p.fg }
    val shade = p.fg.copy(alpha = 0.07f)
    val line = p.mu.copy(alpha = 0.4f)

    fun annotated(l: LookLine, step: LookStep? = null, swapOkToErr: Boolean = false): AnnotatedString = buildAnnotatedString {
        l.forEach { seg ->
            val r = if (swapOkToErr && seg.role == "ok") "err" else seg.role
            val s = fill(seg.text, step, runtime.name)
            if (r == "pill") withStyle(SpanStyle(background = role(tk.pillRole ?: "ac"), color = p.bg, fontWeight = FontWeight.Bold)) { append(s) }
            else withStyle(SpanStyle(color = role(r), fontWeight = if (r == "b") FontWeight.Bold else FontWeight.Normal)) { append(s) }
        }
    }

    @Composable
    fun Ln(l: LookLine, step: LookStep? = null, swap: Boolean = false) = Text(annotated(l, step, swap), style = base, modifier = Modifier.heightIn(min = 18.dp))

    @Composable
    fun Banner() {
        if (app) {
            Row(
                Modifier.fillMaxWidth().drawBehind { drawLine(line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx()) }.padding(bottom = 10.dp)
                    .semantics(mergeDescendants = true) { heading() },
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Box(Modifier.size(22.dp).clip(RoundedCornerShape(6.dp)).background(p.ac), contentAlignment = Alignment.Center) {
                    Text(runtime.name.take(1).uppercase(), color = p.bg, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                }
                Text(runtime.name, style = base.copy(fontWeight = FontWeight.Bold))
                tk.header?.let { Text(it, style = base.copy(color = p.mu, fontSize = 12.sp)) }
                Spacer(Modifier.weight(1f))
                tk.status?.let {
                    Box(Modifier.size(7.dp).clip(CircleShape).background(p.ac2))
                    Text(it, style = base.copy(color = p.mu, fontSize = 12.sp))
                }
            }
            return
        }
        if (tk.bigText == null && tk.bigParts.isEmpty() && tk.banner.isEmpty()) return
        val content: @Composable () -> Unit = {
            Column {
                if (tk.bigText != null || tk.bigParts.isNotEmpty()) {
                    val bigStyle = base.copy(fontSize = 28.sp, fontWeight = FontWeight.ExtraBold, letterSpacing = 2.sp, lineHeight = 31.sp)
                    val grad = tk.bigGrad.map { hex(it, p.ac) }
                    if (tk.bigParts.isNotEmpty()) Text(annotated(tk.bigParts), style = bigStyle, modifier = Modifier.semantics { invisibleToUser() })
                    else Text(tk.bigText.orEmpty(), style = bigStyle.copy(color = grad.firstOrNull() ?: p.fg), modifier = Modifier.semantics { invisibleToUser() })
                }
                tk.banner.forEach { Ln(it) }
            }
        }
        if (tk.bannerBox) Box(Modifier.border(1.dp, role(tk.boxRole ?: "mu"), RoundedCornerShape(6.dp)).padding(horizontal = 12.dp, vertical = 8.dp)) { content() }
        else content()
    }

    @Composable
    fun StepsBlock(steps: List<LookStep>) {
        if (app) {
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).border(1.dp, line, RoundedCornerShape(10.dp))) {
                steps.forEachIndexed { i, s ->
                    if (i > 0) Box(Modifier.fillMaxWidth().padding(0.dp).background(line).heightIn(min = 1.dp, max = 1.dp))
                    val label = (tk.steps[s.verb]?.firstOrNull() ?: emptyList()).joinToString("") { fill(it.text, s, runtime.name) }
                    Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(when (s.status) { "error" -> "✕"; "running" -> "○"; else -> "✓" }, style = base.copy(color = if (s.status == "error") p.err else p.ok, fontSize = 12.sp), modifier = Modifier.semantics { invisibleToUser() })
                        Text(label, style = base.copy(fontFamily = FontFamily.Monospace, fontSize = 11.5.sp), modifier = Modifier.weight(1f))
                        if (s.status != "running") Text(s.r, style = base.copy(color = p.mu, fontSize = 12.sp), maxLines = 1)
                    }
                }
            }
            return
        }
        val body: @Composable () -> Unit = {
            Column {
                steps.forEach { s ->
                    val lines = tk.steps[s.verb].orEmpty()
                    (if (s.status == "running") lines.take(1) else lines).forEach { Ln(it, s, swap = s.status == "error") }
                }
            }
        }
        if (tk.stepBox) Box(Modifier.fillMaxWidth().border(1.dp, line, RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 6.dp)) { body() }
        else body()
    }

    @Composable
    fun UserRow(text: String) {
        val g = if (tk.prompt.isNotEmpty()) "${tk.prompt} " else ""
        if (app || tk.userStyle == "bubble") {
            val fill = tk.userFill?.let { hex(it, p.ac.copy(alpha = 0.15f)) }?.takeIf { (it.luminance() < 0.4f) == appDark } ?: p.ac.copy(alpha = 0.15f)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                Text(text, style = base, modifier = Modifier.widthIn(max = 300.dp).clip(RoundedCornerShape(14.dp)).background(fill).padding(horizontal = 12.dp, vertical = 8.dp))
            }
            return
        }
        when (tk.userStyle) {
            "shade" -> Box(Modifier.fillMaxWidth().background(shade).padding(horizontal = 8.dp, vertical = 4.dp)) { Ln(listOf(LineSeg(g, "mu"), LineSeg(text, "fg"))) }
            "bar" -> Box(Modifier.fillMaxWidth().drawBehind { drawLine(p.ac2, Offset(0f, 0f), Offset(0f, size.height), 2.dp.toPx()) }.background(shade).padding(horizontal = 10.dp, vertical = 4.dp)) { Ln(listOf(LineSeg(text, "fg"))) }
            "color" -> Ln(listOf(LineSeg(g, "ac"), LineSeg(text, "ac")))
            else -> Ln(listOf(LineSeg(g, "mu"), LineSeg(text, "mu")))
        }
    }

    @Composable
    fun AgentRow(text: String, streaming: Boolean) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
            if (!app && tk.answerGlyph != null) Text(tk.answerGlyph, style = base.copy(color = role(tk.answerGlyphRole)), modifier = Modifier.semantics { invisibleToUser() })
            Text(
                buildAnnotatedString {
                    withStyle(SpanStyle(color = role(tk.answerRole))) { append(text) }
                    if (streaming) withStyle(SpanStyle(color = p.ac)) { append(" ▍") }
                },
                style = base, modifier = Modifier.weight(1f),
            )
        }
    }

    val listState = rememberLazyListState()
    LaunchedEffect(items.size, items.lastOrNull()) { if (items.isNotEmpty()) listState.animateScrollToItem(items.size) }

    Column(modifier.fillMaxSize().background(p.bg).testTag("runtimeLookView").semantics { contentDescription = "${runtime.name} view" }) {
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = listState, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(if (app) 12.dp else 8.dp)) {
            item(key = "banner") { Banner() }
            items.forEach { entry ->
                when (entry) {
                    is LookItem.User -> item(key = entry.id) { UserRow(entry.text) }
                    is LookItem.Agent -> item(key = entry.id) { AgentRow(entry.text, entry.streaming) }
                    is LookItem.Steps -> item(key = entry.id) {
                        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            if (isStreaming && entry.steps.any { s -> s.status == "running" }) tk.pre.forEach { l -> Ln(l) }
                            StepsBlock(entry.steps)
                        }
                    }
                }
            }
            runtime.liveUi?.let { _ ->
                item(key = "live") {
                    Text(
                        "Open ${runtime.name}'s own app on your computer in Allternit Desktop.",
                        style = base.copy(color = p.mu, fontSize = 12.sp, fontFamily = FontFamily.Default),
                        modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(shade).padding(10.dp).testTag("runtimeLiveAppNote"),
                    )
                }
            }
        }
        LookComposer(tk, p, base, line, shade, isStreaming, onSend, onStop, label = "Message ${botName ?: runtime.name}", ::annotated)
    }
}

@Composable
private fun LookComposer(
    tk: LookTokens, p: Pal, base: TextStyle, line: Color, shade: Color, busy: Boolean,
    onSend: (String) -> Unit, onStop: () -> Unit, label: String,
    annotated: (LookLine, LookStep?, Boolean) -> AnnotatedString,
) {
    var draft by rememberSaveable { mutableStateOf("") }
    val send = { val s = draft.trim(); if (s.isNotEmpty()) { onSend(s); draft = "" } }
    fun role(r: String?): Color = when (r) { "mu" -> p.mu; "ac" -> p.ac; "ac2" -> p.ac2; "ok" -> p.ok; "err" -> p.err; else -> p.fg }
    val field: @Composable (Modifier) -> Unit = { m ->
        Box(m) {
            if (draft.isEmpty()) Text(tk.placeholder.ifEmpty { if (tk.isApp) label else "" }, style = base.copy(color = p.mu), maxLines = 1)
            BasicTextField(
                value = draft, onValueChange = { draft = it }, textStyle = base, cursorBrush = SolidColor(p.ac),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send), keyboardActions = KeyboardActions(onSend = { send() }),
                modifier = Modifier.fillMaxWidth().testTag("runtimeLookInput").semantics { contentDescription = label },
            )
        }
    }
    val action: @Composable () -> Unit = {
        val (text, desc, onClick) = if (busy) Triple(if (tk.isApp) "■" else "esc to interrupt", "Stop", onStop) else Triple(if (tk.isApp) "↑" else "↵", "Send message", send)
        Box(
            Modifier.size(48.dp).clip(RoundedCornerShape(if (tk.sendSquare) 8.dp else 24.dp)).clickable(enabled = busy || draft.isNotBlank(), role = Role.Button, onClick = onClick)
                .semantics { contentDescription = desc }.testTag(if (busy) "runtimeLookStop" else "runtimeLookSend"),
            contentAlignment = Alignment.Center,
        ) {
            if (tk.isApp) Box(Modifier.size(30.dp).clip(RoundedCornerShape(if (tk.sendSquare) 8.dp else 15.dp)).background(p.ac.copy(alpha = if (busy || draft.isNotBlank()) 1f else 0.45f)), contentAlignment = Alignment.Center) {
                Text(text, color = p.bg, fontWeight = FontWeight.Bold, fontSize = 14.sp)
            } else Text(text, style = base.copy(color = if (busy) p.mu else p.ac, fontSize = if (busy) 11.5.sp else 13.sp), maxLines = 1)
        }
    }
    if (tk.isApp) {
        Box(Modifier.fillMaxWidth().background(p.bg).padding(start = 14.dp, end = 14.dp, bottom = 14.dp)) {
            Row(
                Modifier.fillMaxWidth().border(BorderStroke(1.dp, line), RoundedCornerShape((tk.radius ?: 14).dp)).padding(start = 14.dp, end = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) { field(Modifier.weight(1f)); action() }
        }
        return
    }
    val glyph = tk.inputGlyph ?: tk.prompt
    val row: @Composable () -> Unit = {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (glyph.isNotEmpty()) Text(glyph, style = base.copy(color = role(tk.inputGlyphRole ?: "mu")), modifier = Modifier.semantics { invisibleToUser() })
            field(Modifier.weight(1f))
            action()
        }
    }
    Column(Modifier.fillMaxWidth().background(p.bg).padding(start = 16.dp, end = 10.dp, bottom = 12.dp, top = 6.dp)) {
        when (tk.input) {
            "round" -> Box(Modifier.fillMaxWidth().border(1.dp, role(tk.inputBorderRole ?: "mu"), RoundedCornerShape(8.dp)).padding(start = 10.dp)) { row() }
            "shade" -> Box(Modifier.fillMaxWidth().background(shade).padding(start = 10.dp)) { row() }
            "bar" -> Box(Modifier.fillMaxWidth().drawBehind { drawLine(role(tk.inputBorderRole ?: "ac"), Offset(0f, 0f), Offset(0f, size.height), 3.dp.toPx()) }.background(shade).padding(start = 10.dp)) { row() }
            "line" -> Box(Modifier.fillMaxWidth().drawBehind {
                drawLine(line, Offset(0f, 0f), Offset(size.width, 0f), 1.dp.toPx()); drawLine(line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx())
            }) { row() }
            "rule" -> Column {
                Text(annotated(listOf(LineSeg("━".repeat(14), "ac"), LineSeg(if (busy) " working" else "", "mu")), null, false), style = base)
                row()
            }
            else -> row()
        }
        val f = listOfNotNull(tk.footerL, tk.footerM, tk.footerR)
        if (f.isNotEmpty()) {
            Row(Modifier.fillMaxWidth().padding(top = 6.dp, end = 6.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                Text(tk.footerL?.let { annotated(it, null, false) } ?: AnnotatedString(""), style = base.copy(fontSize = 11.5.sp))
                tk.footerM?.let { Text(annotated(it, null, false), style = base.copy(fontSize = 11.5.sp)) }
                tk.footerR?.let { Text(annotated(it, null, false), style = base.copy(fontSize = 11.5.sp)) }
            }
        }
    }
}

