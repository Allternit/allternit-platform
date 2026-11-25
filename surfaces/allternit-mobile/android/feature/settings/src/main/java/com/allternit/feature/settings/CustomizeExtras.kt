package com.allternit.feature.settings

import android.content.Context
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import com.allternit.core.design.AllternitTheme
import com.allternit.core.design.GlassPill
import com.allternit.core.design.PolishGlass
import com.allternit.core.design.PolishHaptic
import com.allternit.core.design.R as DesignR
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/*
 * Customize > Sources and Publish (iOS DirectoryPages: CustomizeSourcesView, CustomizePublishView). Sources are kept
 * per device, like iOS (UserDefaults) and the web (localStorage `allternit:plugin-manager:personal-marketplaces:v1`).
 */

internal data class PersonalSource(val id: String, val type: String, val value: String, val label: String?)

internal object PersonalSources {
    private const val KEY = "allternit:plugin-manager:personal-marketplaces:v1"
    private val types = setOf("github", "url", "upload", "local")
    private fun prefs(c: Context) = c.applicationContext.getSharedPreferences("allternit_settings", Context.MODE_PRIVATE)

    fun load(c: Context): List<PersonalSource> {
        val raw = prefs(c).getString(KEY, null) ?: return emptyList()
        return runCatching {
            val arr = JSONArray(raw)
            (0 until arr.length()).map { arr.getJSONObject(it) }
                .map { PersonalSource(it.optString("id"), it.optString("type"), it.optString("value"), it.optString("label").ifBlank { null }) }
                .filter { it.type in types }
        }.getOrDefault(emptyList())
    }

    private fun save(c: Context, list: List<PersonalSource>) {
        val arr = JSONArray()
        list.forEach { arr.put(JSONObject().put("id", it.id).put("type", it.type).put("value", it.value).apply { it.label?.let { l -> put("label", l) } }.put("createdAt", java.time.Instant.now().toString())) }
        prefs(c).edit().putString(KEY, arr.toString()).apply()
    }

    /** handleAddPersonalMarketplaceSource: trimmed, no duplicates. */
    fun add(c: Context, type: String, value: String, label: String) {
        val v = value.trim()
        if (v.isEmpty()) return
        val list = load(c)
        if (list.any { it.type == type && it.value == v }) return
        save(c, list + PersonalSource("personal-$type-${System.currentTimeMillis()}-${UUID.randomUUID().toString().take(5).lowercase()}", type, v, label.trim().ifBlank { null }))
    }

    fun remove(c: Context, id: String) = save(c, load(c).filter { it.id != id })
}

@Composable
internal fun SourcesPage(onBack: () -> Unit) {
    val context = LocalContext.current
    var sources by remember { mutableStateOf(PersonalSources.load(context)) }
    var adding by remember { mutableStateOf(false) }
    var type by remember { mutableStateOf("github") }
    var value by remember { mutableStateOf("") }
    var label by remember { mutableStateOf("") }
    SPage("Sources", tag = "customizeSources", onBack = onBack) {
        SNote("Where skills and plugins come from: marketplaces, GitHub repos, URLs, and local folders you've added.")
        if (sources.isEmpty()) SNote("No sources added yet.", art = com.allternit.core.design.EmptyArtKind.Sheets) else SCard {
            sources.forEachIndexed { i, s ->
                if (i > 0) SDivider()
                SRow(s.label ?: s.value, "source_${s.id}", detail = "${s.type} · ${s.value}", chevron = false,
                    onClick = { PersonalSources.remove(context, s.id); sources = PersonalSources.load(context) })
            }
        }
        if (sources.isNotEmpty()) SNote("Tap a source to remove it.")
        if (adding) {
            SLabel("Add source")
            Row(Modifier.padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton(if (type == "github") "• GitHub repo" else "GitHub repo", "source_type_github") { type = "github" }
                SButton(if (type == "url") "• URL" else "URL", "source_type_url") { type = "url" }
            }
            SField(if (type == "github") "owner/repo" else "https://example.com/marketplace.json", value, "source_value", { value = it })
            SField("Name (optional)", label, "source_label", { label = it })
            SNote("Uploads and local folders are added from Allternit on your computer.")
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SButton("Add", "source_add", enabled = value.isNotBlank()) {
                    PersonalSources.add(context, type, value, label)
                    sources = PersonalSources.load(context)
                    value = ""; label = ""; adding = false
                }
                SButton("Cancel", "source_cancel") { adding = false }
            }
        } else {
            SCard(Modifier.padding(top = 18.dp)) { SRow("Add source", "source_new", chevron = false, onClick = { adding = true }) }
        }
    }
}

@Composable
internal fun PublishPage(onBack: () -> Unit, onStart: () -> Unit, onPublished: () -> Unit) {
    SPage("Publish", tag = "customizePublish", onBack = onBack) {
        SNote("Share what you've built. Published artifacts from sessions live in Artifacts.")
        SCard {
            SRow("1  Package", "publish_step1", detail = "Bundle skills, commands, and connectors into a plugin.", chevron = false)
            SDivider()
            SRow("2  Validate", "publish_step2", detail = "Check the manifest, permissions, and file layout.", chevron = false)
            SDivider()
            SRow("3  Submit", "publish_step3", detail = "Publish to the Allternit marketplace or share a link.", chevron = false)
        }
        Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            SButton("Start publishing", "publish_start", onClick = onStart)
            SButton("Published artifacts", "publish_artifacts", onClick = onPublished)
        }
    }
}

/** A Customize card: the neutral fill at radius 20 (the polish pass; lists inside keep their rows). */
@Composable
internal fun CCard(modifier: Modifier = Modifier, radius: Dp = 20.dp, content: @Composable ColumnScope.() -> Unit) {
    Column(modifier.fillMaxWidth().clip(RoundedCornerShape(radius)).background(PolishGlass.track()), content = content)
}

/** Search on the neutral fill, no border. */
@Composable
internal fun CustomizeSearch(value: String, placeholder: String, onChange: (String) -> Unit, modifier: Modifier = Modifier) {
    val c = AllternitTheme.colors
    Row(
        modifier.height(40.dp).clip(RoundedCornerShape(14.dp)).background(PolishGlass.track()).padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(DesignR.drawable.ph_magnifying_glass), contentDescription = null, tint = c.textTertiary, modifier = Modifier.size(16.dp))
        Box(Modifier.weight(1f).padding(start = 7.dp), contentAlignment = Alignment.CenterStart) {
            if (value.isEmpty()) Text(placeholder, color = c.textTertiary, fontSize = 13.sp, maxLines = 1)
            BasicTextField(
                value = value, onValueChange = onChange, singleLine = true,
                textStyle = TextStyle(color = c.textPrimary, fontSize = 13.sp),
                cursorBrush = SolidColor(c.textPrimary),
                modifier = Modifier.fillMaxWidth().testTag("customize_search").semantics { contentDescription = placeholder },
            )
        }
    }
}

/**
 * Tabs with an underline that slides between them (its offset and width are animated), with a selection
 * haptic on a pick. [onSelect] null makes the row a picture (the intro hero). [carets] marks tabs with a ⌄.
 */
@Composable
internal fun SlidingTabRow(
    labels: List<String>,
    selected: Int,
    onSelect: ((Int) -> Unit)?,
    modifier: Modifier = Modifier,
    fontSize: TextUnit = 13.5.sp,
    gap: Dp = 18.dp,
    carets: Set<Int> = emptySet(),
) {
    val c = AllternitTheme.colors
    val view = LocalView.current
    val density = LocalDensity.current
    val frames = remember { mutableStateMapOf<Int, Pair<Float, Float>>() } // index -> x, width in px
    val target = frames[selected]
    val x by animateDpAsState(with(density) { (target?.first ?: 0f).toDp() }, spring(dampingRatio = 0.85f, stiffness = 420f), label = "tabX")
    val w by animateDpAsState(with(density) { (target?.second ?: 0f).toDp() }, spring(dampingRatio = 0.85f, stiffness = 420f), label = "tabW")
    Box(modifier.fillMaxWidth()) {
        Box(Modifier.align(Alignment.BottomStart).fillMaxWidth().height(1.dp).background(c.borderSubtle))
        Row(horizontalArrangement = Arrangement.spacedBy(gap)) {
            labels.forEachIndexed { i, label ->
                val picked = i == selected
                val color by animateColorAsState(if (picked) c.textPrimary else c.textSecondary, spring(stiffness = 420f), label = "tabText")
                Row(
                    Modifier
                        .onGloballyPositioned { frames[i] = it.positionInParent().x to it.size.width.toFloat() }
                        .semantics { this.selected = picked }
                        .then(
                            if (onSelect != null) Modifier.clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, role = Role.Tab) {
                                if (!picked) { PolishHaptic.perform(view, PolishHaptic.Kind.Selection); onSelect(i) }
                            } else Modifier,
                        )
                        .padding(bottom = 8.dp, top = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(3.dp),
                ) {
                    Text(label, color = color, fontSize = fontSize, fontWeight = if (picked) FontWeight.SemiBold else FontWeight.Normal)
                    if (i in carets) Icon(painterResource(DesignR.drawable.ph_caret_down), contentDescription = null, tint = color, modifier = Modifier.size(12.dp))
                }
            }
        }
        if (target != null) {
            Box(Modifier.align(Alignment.BottomStart).offset(x = x).width(w).height(2.dp).clip(RoundedCornerShape(2.dp)).background(c.textPrimary))
        }
    }
}
