package com.allternit.feature.settings

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.listUnder
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull

/*
 * The Catalog (catalog v2, `GET /api/v1/connections/catalog` and `/catalog/:id`). Field names are the wire's snake_case,
 * the same ones iOS reads. One list for connectors and skills; the group decides which Add flow runs.
 */

/** One catalog entry. [group] is "skills" for a skill, anything else is a connector. */
internal data class CatEntry(
    val id: String,
    val name: String,
    val blurb: String?,
    val author: String?,
    val authorUrl: String?,
    val serverUrl: String?,
    val categories: List<String>,
    val interactive: Boolean,
    val screenPack: String?,
    val tier: String?,
    val iconUrl: String?,
    val available: Boolean,
    val unavailableReason: String?,
    val appId: String?,
    val group: String?,
    val verified: Boolean,
    val trending: Boolean,
    val isNew: Boolean,
    val popularity: Double,
) {
    val isSkill: Boolean get() = group == "skills"

    /** The connector id the existing connect routes use. */
    val connectorId: String get() = appId ?: id

    /** "Connector · By Google" / "Skill · By Anthropic". */
    val kindLine: String get() = (if (isSkill) "Skill" else "Connector") + (author?.let { " · By $it" } ?: "")

    companion object {
        fun parse(o: JsonObject): CatEntry? {
            val id = o.str("id") ?: return null
            fun strings(key: String) = (o[key] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.contentOrNull } ?: emptyList()
            val tier = o.str("tier")
            return CatEntry(
                id = id, name = o.str("name") ?: id, blurb = o.str("blurb"), author = o.str("author"), authorUrl = o.str("author_url"),
                serverUrl = o.str("server_url"), categories = strings("categories"), interactive = o.bool("interactive") ?: false,
                screenPack = o.str("screen_pack"), tier = tier, iconUrl = o.str("icon_url"), available = o.bool("available") ?: true,
                unavailableReason = o.str("unavailable_reason"), appId = o.str("app_id"), group = o.str("group"),
                verified = o.bool("verified") ?: (tier == "verified" || tier == "official"),
                trending = o.bool("trending") ?: false, isNew = o.bool("new") ?: o.bool("is_new") ?: false,
                popularity = (o["popularity"] as? JsonPrimitive)?.doubleOrNull ?: 0.0,
            )
        }
    }
}

internal object CatalogClient {
    /** The whole list in one call (sorted by use); sections, tabs and the Interactive filter read it. [q] and [category] go to the server. */
    suspend fun list(q: String = "", category: String? = null, interactive: Boolean = false): Load<List<CatEntry>> {
        val query = buildList {
            add("sort=popular"); add("limit=100")
            q.trim().takeIf { it.isNotEmpty() }?.let { add("q=" + LibraryNet.escape(it.take(64))) }
            if (category != null) add("category=" + LibraryNet.escape(category))
            if (interactive) add("interactive=true")
        }.joinToString("&")
        return when (val r = Net.cloud("GET", "api/v1/connections/catalog?$query")) {
            is ApiResult.Ok -> Load.Ready(r.value.listUnder("entries").mapNotNull(CatEntry::parse))
            else -> r.toLoadOf("load the Catalog")
        }
    }

    /** `/catalog/:id`: the full entry. Null when the server has no such entry. */
    suspend fun detail(id: String): Load<CatEntry?> {
        return when (val r = Net.cloud("GET", "api/v1/connections/catalog/${LibraryNet.escape(id)}")) {
            is ApiResult.Ok -> Load.Ready(r.value.obj?.let { (it["entry"] as? JsonObject ?: it).let(CatEntry::parse) })
            else -> r.toLoadOf("load this entry")
        }
    }

    private fun ApiResult<JsonElement>.toLoadOf(doing: String): Load<Nothing> = when (val l = toLoad(doing)) {
        is Load.Failed -> Load.Failed(l.message)
        Load.NoComputer -> Load.NoComputer
        else -> Load.Failed("Couldn't $doing.")
    }

    private val bitmaps = HashMap<String, Bitmap>()

    suspend fun icon(url: String): Bitmap? {
        bitmaps[url]?.let { return it }
        val bmp = withContext(Dispatchers.IO) { runCatching { java.net.URL(url).openStream().use { BitmapFactory.decodeStream(it) } }.getOrNull() }
        if (bmp != null) bitmaps[url] = bmp
        return bmp
    }
}

/** The entry's logo: `icon_url` when it loads, else the bundled logo or a monogram tile (AppLogo). */
@Composable
internal fun CatalogIcon(entry: CatEntry, size: Dp = 32.dp) {
    var bmp by remember(entry.iconUrl) { mutableStateOf<Bitmap?>(null) }
    LaunchedEffect(entry.iconUrl) { entry.iconUrl?.let { bmp = CatalogClient.icon(it) } }
    val shown = bmp
    if (shown != null) Box(Modifier.size(size).clip(RoundedCornerShape((size.value * 0.24f).dp))) {
        Image(shown.asImageBitmap(), contentDescription = null, contentScale = ContentScale.Fit, modifier = Modifier.size(size))
    } else com.allternit.core.design.AppLogo(entry.name, id = entry.appId ?: entry.id, size = size)
}
