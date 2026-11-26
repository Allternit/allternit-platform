package com.allternit.feature.bar

import android.content.Context
import com.allternit.core.api.FeatureFlags
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.intOrNull

// The island bar's data (Island bar + Team map, lane 36): the Android port of iOS IslandBarClient / BarWire types
// (platform PR #3037). Wire shapes: handoffs/island-bar-team-map-2026-11-25/CONTRACT.md section 2.

/** `bar.island`, off by default (iOS IslandBarFlag). */
object BarFlag {
    fun isOn(context: Context): Boolean = FeatureFlags.isOn(context, FeatureFlags.BAR_ISLAND, false)
}

/** A typed blank value (iOS BarValue). */
sealed interface BarValue {
    data class Text(val value: String) : BarValue
    data class Number(val value: Double) : BarValue
    data class Bool(val value: Boolean) : BarValue

    val display: String
        get() = when (this) {
            is Text -> value
            is Number -> if (value % 1.0 == 0.0 && kotlin.math.abs(value) < 1e15) value.toLong().toString() else value.toString()
            is Bool -> if (value) "Yes" else "No"
        }

    fun json(): JsonPrimitive = when (this) {
        is Text -> JsonPrimitive(value)
        is Number -> if (value % 1.0 == 0.0 && kotlin.math.abs(value) < 1e15) JsonPrimitive(value.toLong()) else JsonPrimitive(value)
        is Bool -> JsonPrimitive(value)
    }

    companion object {
        fun of(e: JsonElement?): BarValue? {
            val p = e as? JsonPrimitive ?: return null
            if (p is JsonNull) return null
            if (p.isString) return Text(p.content)
            p.booleanOrNull?.let { return Bool(it) }
            p.doubleOrNull?.let { return Number(it) }
            return null
        }
    }
}

/** `BarApp` (`GET /bar/apps`). [id] is `<kind>:<ref>` like iOS BarAppInfo.id and the deep link's `app=`. */
data class BarApp(val kind: String, val ref: String, val name: String, val level: String, val toolCount: Int) {
    val id: String get() = "$kind:$ref"
    /** Artifact apps are not runnable yet (CONTRACT section 8). */
    val isRunnable: Boolean get() = kind != "artifact"

    companion object {
        fun split(id: String): BarApp? {
            val i = id.indexOf(':')
            if (i <= 0 || i == id.length - 1) return null
            val ref = id.substring(i + 1)
            return BarApp(id.substring(0, i), ref, ref, "", 0)
        }
    }
}

data class BarTool(val name: String, val title: String, val hasUi: Boolean, val readOnly: Boolean)

data class BarOption(val value: String, val label: String)

data class BarRecent(val value: BarValue, val label: String)

data class BarBlank(
    val key: String,
    val label: String,
    val type: String,
    val required: Boolean,
    val unit: String?,
    val options: List<BarOption>,
    val pickerKind: String,
)

sealed interface SentencePart {
    data class Text(val text: String) : SentencePart
    data class Blank(val key: String) : SentencePart
}

/** `POST /bar/intent`. Stateless: [intentId] is resent with the values. */
data class BarIntent(
    val intentId: String,
    val app: BarApp,
    val toolName: String,
    val toolTitle: String,
    val parts: List<SentencePart>,
    val blanks: List<BarBlank>,
    val recents: Map<String, List<BarRecent>>,
)

data class BarReviewLine(val key: String, val label: String, val value: String)

/** `POST /bar/review`. */
data class BarReview(
    val jobId: String,
    val level: String,
    val ruleText: String?,
    val approverName: String?,
    val approverIsYou: Boolean,
    val approverRole: String?,
    val lines: List<BarReviewLine>,
)

/** A `Job` row reduced to what the bar, the notification and the widget show. */
data class BarJob(
    val id: String,
    val state: String,
    val title: String,
    val appName: String?,
    val botName: String?,
    val receipt: String?,
    val error: String?,
    val approverName: String?,
)

// MARK: - Parsing

internal fun JsonElement?.o(): JsonObject? = this as? JsonObject
internal fun JsonElement?.a(): List<JsonElement> = (this as? JsonArray).orEmpty()
internal fun JsonObject?.s(key: String): String? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content
internal fun JsonObject?.b(key: String): Boolean? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.booleanOrNull
internal fun JsonObject?.i(key: String): Int? = (this?.get(key) as? JsonPrimitive)?.takeIf { it !is JsonNull }?.intOrNull

internal object BarParse {
    fun appRef(a: JsonObject?): BarApp? {
        val kind = a.s("kind") ?: return null
        val ref = a.s("ref") ?: return null
        return BarApp(kind, ref, a.s("name")?.takeIf { it.isNotBlank() } ?: ref, "", 0)
    }

    fun app(row: JsonObject?): BarApp? =
        appRef(row?.get("app").o())?.copy(level = row.s("level").orEmpty(), toolCount = row.i("tool_count") ?: 0)

    fun tool(t: JsonObject?): BarTool? {
        val name = t.s("name") ?: return null
        return BarTool(name, t.s("title")?.takeIf { it.isNotBlank() } ?: name, t.b("has_ui") == true, t.b("read_only") == true)
    }

    fun blank(b: JsonObject?): BarBlank? {
        val key = b.s("key") ?: return null
        val options = b?.get("options").a().mapNotNull { o ->
            val v = BarValue.of(o.o()?.get("value"))?.display ?: return@mapNotNull null
            BarOption(v, o.o().s("label") ?: v)
        }
        return BarBlank(
            key = key,
            label = b.s("label") ?: key,
            type = b.s("type") ?: "text",
            required = b.b("required") == true,
            unit = b.s("unit")?.takeIf { it.isNotBlank() },
            options = options,
            pickerKind = b?.get("picker").o().s("kind") ?: "standard",
        )
    }

    fun intent(json: JsonObject, picked: BarApp, tool: String): BarIntent? {
        val intentId = json.s("intent_id") ?: return null
        val rows = json["blanks"].a().mapNotNull { it.o() }
        val blanks = rows.mapNotNull(::blank)
        val recents = rows.associate { row ->
            row.s("key").orEmpty() to row["recent"].a().mapNotNull { r ->
                val v = BarValue.of(r.o()?.get("value")) ?: return@mapNotNull null
                BarRecent(v, r.o().s("label") ?: v.display)
            }
        }
        val parts = json["sentence"].a().mapNotNull { p ->
            val o = p.o()
            when {
                o.s("type") == "blank" && o.s("key") != null -> SentencePart.Blank(o.s("key")!!)
                o.s("text") != null -> SentencePart.Text(o.s("text")!!)
                else -> null
            }
        }.toMutableList()
        // A sentence with no blank parts still shows every blank (iOS IslandBarClient.intent).
        val inSentence = parts.filterIsInstance<SentencePart.Blank>().map { it.key }.toSet()
        blanks.filter { it.key !in inSentence }.forEach { parts += SentencePart.Text(" "); parts += SentencePart.Blank(it.key) }
        val toolObj = json["tool"].o()
        val app = appRef(json["app"].o())?.let { a ->
            a.copy(name = if (a.name == a.ref) picked.name else a.name, level = picked.level, toolCount = picked.toolCount)
        } ?: picked
        return BarIntent(intentId, app, toolObj.s("name") ?: tool, toolObj.s("title") ?: tool, parts, blanks, recents)
    }

    fun review(json: JsonObject): BarReview? {
        val jobId = json.s("job_id") ?: return null
        val approver = json["approver"].o()
        return BarReview(
            jobId = jobId,
            level = json.s("level").orEmpty(),
            ruleText = json["rule"].o().s("text")?.trim()?.takeIf { it.isNotEmpty() },
            approverName = approver.s("name")?.takeIf { it.isNotBlank() },
            approverIsYou = approver.b("is_you") == true,
            approverRole = approver.s("role"),
            lines = json["lines"].a().mapNotNull { l ->
                val o = l.o() ?: return@mapNotNull null
                BarReviewLine(o.s("key").orEmpty(), o.s("label") ?: o.s("key").orEmpty(), BarValue.of(o["value"])?.display ?: o.s("value").orEmpty())
            },
        )
    }

    fun job(job: JsonObject?, approverName: String?): BarJob? {
        val id = job.s("id") ?: return null
        val state = job.s("state")?.takeIf { it.isNotEmpty() } ?: return null
        val err = job?.get("error")
        val error = (err as? JsonPrimitive)?.takeIf { it !is JsonNull }?.content ?: err.o().s("message")
        val approver = job?.get("result").o()?.get("approver").o().s("name") ?: approverName
        return BarJob(
            id = id,
            state = state,
            title = job.s("title")?.takeIf { it.isNotBlank() } ?: job.s("tool").orEmpty(),
            appName = job?.get("app").o().s("name") ?: job?.get("app").o().s("ref"),
            botName = job?.get("bot").o().s("name"),
            receipt = if (state == "completed") BarWording.receipt(job) else null,
            error = error?.takeIf { it.isNotBlank() },
            approverName = approver,
        )
    }
}
