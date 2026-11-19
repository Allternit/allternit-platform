package com.allternit.feature.chat

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme

/**
 * Markdown for assistant replies (iOS AssistantContent / MarkdownView): headings, paragraphs,
 * bullet and numbered lists, quotes, rules, fenced code, and inline bold, italic, code, links
 * and strikethrough. A small parser on purpose: no markdown library is in the catalog, and a
 * streaming reply re-renders up to ~20 times a second, so it stays linear and allocation-light.
 * An unclosed fence while streaming renders as code so far (iOS StreamingMarkdownReader).
 */
internal sealed interface MdBlock {
    data class Heading(val level: Int, val text: String) : MdBlock
    data class Paragraph(val text: String) : MdBlock
    data class Bullet(val items: List<String>, val ordered: Boolean, val start: Int) : MdBlock
    data class Quote(val text: String) : MdBlock
    data class Code(val lang: String, val code: String) : MdBlock
    data object Rule : MdBlock
}

internal object MarkdownParser {
    private val heading = Regex("""^(#{1,6})\s+(.*)$""")
    private val bullet = Regex("""^\s*[-*+]\s+(.*)$""")
    private val ordered = Regex("""^\s*(\d+)[.)]\s+(.*)$""")
    private val rule = Regex("""^\s*([-*_])(\s*\1){2,}\s*$""")

    fun parse(text: String): List<MdBlock> {
        val out = mutableListOf<MdBlock>()
        val lines = text.replace("\r\n", "\n").split('\n')
        var i = 0
        val para = StringBuilder()
        fun flushPara() {
            if (para.isNotBlank()) out += MdBlock.Paragraph(para.toString().trim())
            para.clear()
        }
        while (i < lines.size) {
            val line = lines[i]
            val trimmed = line.trimStart()
            when {
                trimmed.startsWith("```") -> {
                    flushPara()
                    val lang = trimmed.removePrefix("```").trim()
                    val code = StringBuilder()
                    i++
                    while (i < lines.size && !lines[i].trimStart().startsWith("```")) {
                        if (code.isNotEmpty()) code.append('\n')
                        code.append(lines[i])
                        i++
                    }
                    out += MdBlock.Code(lang, code.toString())
                }
                line.isBlank() -> flushPara()
                heading.matches(trimmed) -> {
                    flushPara()
                    val m = heading.find(trimmed)!!
                    out += MdBlock.Heading(m.groupValues[1].length, m.groupValues[2].trim().trimEnd('#').trim())
                }
                rule.matches(line) -> {
                    flushPara()
                    out += MdBlock.Rule
                }
                trimmed.startsWith(">") -> {
                    flushPara()
                    val quote = StringBuilder()
                    while (i < lines.size && lines[i].trimStart().startsWith(">")) {
                        if (quote.isNotEmpty()) quote.append('\n')
                        quote.append(lines[i].trimStart().removePrefix(">").trimStart())
                        i++
                    }
                    out += MdBlock.Quote(quote.toString())
                    continue
                }
                bullet.matches(line) || ordered.matches(line) -> {
                    flushPara()
                    val isOrdered = ordered.matches(line)
                    val start = if (isOrdered) ordered.find(line)!!.groupValues[1].toIntOrNull() ?: 1 else 1
                    val items = mutableListOf<String>()
                    while (i < lines.size) {
                        val l = lines[i]
                        val b = if (isOrdered) ordered.find(l)?.groupValues?.get(2) else bullet.find(l)?.groupValues?.get(1)
                        when {
                            b != null -> items += b
                            // A wrapped continuation line joins the item above.
                            l.isNotBlank() && l.startsWith("  ") && items.isNotEmpty() -> items[items.lastIndex] = items.last() + " " + l.trim()
                            else -> break
                        }
                        i++
                    }
                    out += MdBlock.Bullet(items, isOrdered, start)
                    continue
                }
                else -> {
                    if (para.isNotEmpty()) para.append('\n')
                    para.append(line)
                }
            }
            i++
        }
        flushPara()
        return out
    }

    /** Inline spans: `code`, **bold**, *italic* / _italic_, ~~strike~~, [text](url), bare https links. */
    fun inline(text: String, codeBg: Color, link: Color): AnnotatedString = buildAnnotatedString {
        var i = 0
        val n = text.length
        val plain = StringBuilder()
        fun flush() {
            if (plain.isNotEmpty()) append(plain.toString())
            plain.clear()
        }
        while (i < n) {
            val ch = text[i]
            if (ch == '`') {
                val end = text.indexOf('`', i + 1)
                if (end > i) {
                    flush()
                    withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = codeBg, fontSize = 14.sp)) { append(text.substring(i + 1, end)) }
                    i = end + 1
                    continue
                }
            }
            if ((ch == '*' || ch == '_') && i + 1 < n && text[i + 1] == ch) {
                val marker = "$ch$ch"
                val end = text.indexOf(marker, i + 2)
                if (end > i + 2) {
                    flush()
                    withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append(inline(text.substring(i + 2, end), codeBg, link)) }
                    i = end + 2
                    continue
                }
            }
            if (ch == '~' && i + 1 < n && text[i + 1] == '~') {
                val end = text.indexOf("~~", i + 2)
                if (end > i + 2) {
                    flush()
                    withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { append(text.substring(i + 2, end)) }
                    i = end + 2
                    continue
                }
            }
            if ((ch == '*' || ch == '_') && i + 1 < n && !text[i + 1].isWhitespace() && (i == 0 || !text[i - 1].isLetterOrDigit())) {
                val end = text.indexOf(ch, i + 1)
                if (end > i + 1 && !text[end - 1].isWhitespace()) {
                    flush()
                    withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { append(inline(text.substring(i + 1, end), codeBg, link)) }
                    i = end + 1
                    continue
                }
            }
            if (ch == '[') {
                val close = text.indexOf("](", i + 1)
                val paren = if (close > i) text.indexOf(')', close + 2) else -1
                if (close > i && paren > close) {
                    val label = text.substring(i + 1, close)
                    val url = text.substring(close + 2, paren).trim()
                    if (url.startsWith("http://") || url.startsWith("https://") || url.startsWith("mailto:")) {
                        flush()
                        withLink(LinkAnnotation.Url(url, TextLinkStyles(SpanStyle(color = link, textDecoration = TextDecoration.Underline)))) {
                            append(label)
                        }
                        i = paren + 1
                        continue
                    }
                }
            }
            if (ch == 'h' && text.startsWith("https://", i) && (i == 0 || text[i - 1].isWhitespace() || text[i - 1] == '(')) {
                var end = i
                while (end < n && !text[end].isWhitespace() && text[end] != ')') end++
                while (end > i && text[end - 1] in ".,;:!?") end--
                flush()
                val url = text.substring(i, end)
                withLink(LinkAnnotation.Url(url, TextLinkStyles(SpanStyle(color = link, textDecoration = TextDecoration.Underline)))) { append(url) }
                i = end
                continue
            }
            plain.append(ch)
            i++
        }
        flush()
    }
}

/** Renders assistant markdown at 15sp (iOS AssistantContent). Selectable, links open in the browser. */
@Composable
internal fun MarkdownText(text: String, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    val blocks = remember(text) { MarkdownParser.parse(text) }
    val codeBg = c.fillNeutral
    val link = c.accentPrimary
    SelectionContainer(modifier) {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            blocks.forEach { block ->
                when (block) {
                    is MdBlock.Heading -> Text(
                        MarkdownParser.inline(block.text, codeBg, link),
                        color = c.textPrimary,
                        fontWeight = FontWeight.SemiBold,
                        fontSize = when (block.level) { 1 -> 22.sp; 2 -> 19.sp; 3 -> 17.sp; else -> 15.sp },
                        lineHeight = when (block.level) { 1 -> 28.sp; 2 -> 25.sp; else -> 22.sp },
                    )
                    is MdBlock.Paragraph -> Text(MarkdownParser.inline(block.text, codeBg, link), color = c.textPrimary, fontSize = 15.sp, lineHeight = 22.sp)
                    is MdBlock.Bullet -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        block.items.forEachIndexed { index, item ->
                            Row {
                                Text(
                                    if (block.ordered) "${block.start + index}." else "•",
                                    color = c.textSecondary,
                                    fontSize = 15.sp,
                                    lineHeight = 22.sp,
                                    modifier = Modifier.width(if (block.ordered) 26.dp else 16.dp),
                                )
                                Text(MarkdownParser.inline(item, codeBg, link), color = c.textPrimary, fontSize = 15.sp, lineHeight = 22.sp)
                            }
                        }
                    }
                    is MdBlock.Quote -> Row {
                        Box(Modifier.width(3.dp).height(22.dp).background(c.borderStrong))
                        Text(
                            MarkdownParser.inline(block.text, codeBg, link),
                            color = c.textSecondary,
                            fontSize = 15.sp,
                            lineHeight = 22.sp,
                            modifier = Modifier.padding(start = 10.dp),
                        )
                    }
                    is MdBlock.Code -> Box(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(10.dp))
                            .background(c.fillNeutral)
                            .horizontalScroll(rememberScrollState())
                            .padding(12.dp),
                    ) {
                        Text(block.code, color = c.textPrimary, fontFamily = FontFamily.Monospace, fontSize = 13.sp, lineHeight = 19.sp, softWrap = false)
                    }
                    MdBlock.Rule -> Box(Modifier.fillMaxWidth().height(1.dp).background(c.borderSubtle))
                }
            }
        }
    }
}
