package com.allternit.feature.office

import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.arr
import com.allternit.feature.artifacts.friendly
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.long
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.obj
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

/** One section of an artifact-service document (iOS OfficeArtifactSection). */
data class OfficeSection(val id: String, val heading: String?, val kind: String, val body: String, val position: Int)

/** The office editor family a record belongs to (iOS OfficeFamily). */
enum class OfficeFamily(val raw: String, val displayName: String) {
    Docs("docs", "Document"), Sheets("sheets", "Spreadsheet"), Slides("slides", "Presentation"), Pdf("pdf", "PDF");

    /** Platform editor route for the interactive web view. */
    val editorPath: String get() = "/$raw"

    companion object {
        fun of(sectionKind: String): OfficeFamily? = when {
            sectionKind.startsWith("docs-editor/") -> Docs
            sectionKind.startsWith("sheets-editor/") -> Sheets
            sectionKind.startsWith("slides-editor/") -> Slides
            sectionKind.startsWith("pdf-viewer/") -> Pdf
            else -> null
        }
    }
}

/** The artifact-service document the office viewer renders read-only (iOS OfficeArtifact). */
data class OfficeArtifact(val id: String, val title: String, val sections: List<OfficeSection>) {
    val ordered: List<OfficeSection> get() = sections.sortedBy { it.position }
    val family: OfficeFamily? get() = sections.firstOrNull()?.let { OfficeFamily.of(it.kind) }
}

/** What a load can come back as. [computeRequired] is the 428 / compute_required state (no computer). */
sealed interface OfficeLoad {
    data class Ok(val artifact: OfficeArtifact) : OfficeLoad
    data object SignedOut : OfficeLoad
    data object ComputeRequired : OfficeLoad
    data class Failed(val message: String) : OfficeLoad
}

/** iOS OfficeArtifactClient: `GET /api/v1/artifacts/:id` (`{"artifact": {...}}`). */
object OfficeClient {
    suspend fun getArtifact(id: String): OfficeLoad = when (val r = LibraryNet.get("artifacts/${LibraryNet.escape(id)}")) {
        is ApiResult.Ok -> parse(r.value)?.let { OfficeLoad.Ok(it) } ?: OfficeLoad.Failed("This document couldn't be read.")
        is ApiResult.HttpError -> when {
            r.status == 401 -> OfficeLoad.SignedOut
            r.status == 428 || r.code == "compute_required" -> OfficeLoad.ComputeRequired
            else -> OfficeLoad.Failed(r.friendly("open this document"))
        }
        else -> OfficeLoad.Failed(r.friendly("open this document"))
    }

    private fun parse(e: JsonElement): OfficeArtifact? {
        val o: JsonObject = e.obj?.let { it["artifact"].obj ?: it } ?: return null
        val id = o.str("id") ?: return null
        val sections = o["sections"].arr.orEmpty().mapNotNull { it.obj }.mapIndexedNotNull { i, s ->
            OfficeSection(s.str("id") ?: "s$i", s.str("heading"), s.str("kind") ?: return@mapIndexedNotNull null, s.str("body").orEmpty(), s.num("position")?.toInt() ?: i)
        }
        return OfficeArtifact(id, o.str("title") ?: "Document", sections)
    }
}

/** A pod (project, team or chief group) in `GET /floor/state` v2. */
data class FloorPod(val id: String, val name: String, val color: String, val kind: String)

/** Per bot v2 fields: its pod, counts of what needs the person, and its last result (epoch ms). */
data class FloorBot(val pod: String?, val approval: Int, val question: Int, val suggestion: Int, val resultText: String?, val resultAt: Long)

/** A handoff between bots from `GET /floor/events`, with a short label. */
data class FloorHandoff(val id: String, val from: String, val to: String, val label: String, val at: Long)

/** `GET /api/v1/floor/state` (v:2), the Workspace floor's source of pods, needs and results. */
data class FloorState(val v: Int, val mainBotId: String?, val pods: List<FloorPod>, val bots: Map<String, FloorBot>, val handoffs: List<FloorHandoff> = emptyList())

object FloorClient {
    private suspend fun getOk(path: String): JsonElement? = when (val r = LibraryNet.get(path)) { is ApiResult.Ok -> r.value; else -> null }

    /** Null on any failure (signed out, offline, old server); the floor then shows the v1 roster only. */
    suspend fun state(): FloorState? {
        val o = getOk("floor/state").obj ?: return null
        val v = o.num("v")?.toInt() ?: 1
        if (v < 2) return null
        val pods = o.list("pods").mapNotNull { p ->
            FloorPod(p.str("id") ?: return@mapNotNull null, p.str("name").orEmpty().take(40), p.str("color") ?: return@mapNotNull null, p.str("kind") ?: "project")
        }
        val bots = HashMap<String, FloorBot>()
        o.list("bots").forEach { b ->
            val id = b.str("id") ?: b.str("botId") ?: return@forEach
            val n = b.child("needs")
            val r = b.child("lastResult")
            bots[id] = FloorBot(
                b.str("pod"), n.num("approval")?.toInt() ?: 0, n.num("question")?.toInt() ?: 0, n.num("suggestion")?.toInt() ?: 0,
                r.str("text"), r.long("at") ?: 0L,
            )
        }
        return FloorState(v, o.str("mainBotId"), pods, bots, handoffs())
    }

    /** Recent handoffs (last 3 minutes); empty when events can't be read. */
    private suspend fun handoffs(): List<FloorHandoff> {
        val o = getOk("floor/events") ?: return emptyList()
        val cutoff = System.currentTimeMillis() - 3 * 60_000L
        val items = o.obj?.list("events") ?: o.arr.orEmpty().mapNotNull { it.obj }
        return items.mapNotNull { e ->
            if (e.str("kind") != "handoff") return@mapNotNull null
            val at = e.long("at") ?: 0L
            val from = e.str("botId") ?: e.str("from") ?: return@mapNotNull null
            val to = e.str("toBotId") ?: e.str("to") ?: return@mapNotNull null
            if (at < cutoff) return@mapNotNull null
            FloorHandoff(e.str("id") ?: "$from-$to-$at", from, to, (e.str("label") ?: e.str("text")).orEmpty().take(60), at)
        }
    }
}
