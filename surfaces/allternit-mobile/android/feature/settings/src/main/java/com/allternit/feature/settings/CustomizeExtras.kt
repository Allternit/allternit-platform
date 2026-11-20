package com.allternit.feature.settings

import android.content.Context
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
        if (sources.isEmpty()) SNote("No sources added yet.") else SCard {
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
