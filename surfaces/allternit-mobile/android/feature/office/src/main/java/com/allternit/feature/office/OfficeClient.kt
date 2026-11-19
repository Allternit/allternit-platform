package com.allternit.feature.office

import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.arr
import com.allternit.feature.artifacts.friendly
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
