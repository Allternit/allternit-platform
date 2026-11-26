package com.allternit.feature.artifacts

import androidx.annotation.DrawableRes
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.design.R as DesignR
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/*
 * Artifacts v2 (iOS Core/API/ArtifactsV2Client.swift + ArtifactV2 models). Base is the cloud API
 * (`https://api.allternit.com/api/v2/...`) with the Clerk bearer, like iOS AppConfig.cloudAPIBaseURL.
 */

data class ArtifactOwner(val id: String, val name: String?, val imageUrl: String?)

data class ArtifactOrigin(
    val surface: String?,
    val sessionId: String?,
    val messageId: String?,
    /** `origin.legacy_source` ("sectioned" for Office records imported from the old store) and `legacy_id` (that record's id). */
    val legacySource: String? = null,
    val legacyId: String? = null,
    /** Plans: the project and chat the plan was saved from (`origin.project_title`, `session_title` / `chat_title`). */
    val projectTitle: String? = null,
    val sessionTitle: String? = null,
)

/** One row of `GET api/v2/artifacts` (iOS ArtifactV2Summary). */
data class ArtifactSummary(
    val id: String,
    val kind: String,
    val title: String,
    val owner: ArtifactOwner,
    val updatedAt: String,
    val currentVersion: Int,
    val visibility: String,
    val myAccess: String,
    val origin: ArtifactOrigin?,
    val thumbnailUrl: String?,
    /** `plan.status` on a `plan` artifact: open, approved or done. */
    val planStatus: String? = null,
) {
    val isMine: Boolean get() = myAccess == "owner"
}

/** `GET api/v2/artifacts/{id}` (iOS ArtifactV2 with its current version). */
data class ArtifactV2(
    val summary: ArtifactSummary,
    val version: Int,
    val body: String,
    val bodyFormat: String,
    val meta: JsonObject?,
) {
    val id get() = summary.id
    val kind get() = summary.kind
    val title get() = summary.title
    val canEdit: Boolean get() = summary.myAccess == "owner" || summary.myAccess == "edit"
}

data class ArtifactComment(
    val id: String,
    val parentId: String?,
    val authorId: String,
    val authorName: String?,
    val body: String,
    val resolvedAt: String?,
    val createdAt: String,
    val agentName: String?,
)

data class ArtifactListPage(val items: List<ArtifactSummary>, val nextCursor: String?, val archiveSupported: Boolean)

/** The 14 known kinds (iOS ArtifactV2Kind); unknown strings stay raw and fall back to the docs glyph. */
object ArtifactKinds {
    val filterKinds = listOf("doc", "slides", "sheet", "design", "dashboard", "motion", "page", "card", "diagram", "image", "code", "pdf")

    fun label(kind: String): String = when (kind) {
        "doc" -> "Docs"; "sheet" -> "Sheets"; "slides" -> "Slides"; "design" -> "Designs"; "dashboard" -> "Dashboards"
        "motion" -> "Motion"; "page" -> "Pages"; "card" -> "Cards"; "diagram" -> "Diagrams"; "image" -> "Images"
        "code" -> "Code"; "pdf" -> "PDFs"; "video" -> "Videos"; "ops" -> "Ops sheets"
        else -> kind.replaceFirstChar { it.uppercase() }
    }

    fun noun(kind: String): String = when (kind) {
        "doc" -> "Doc"; "sheet" -> "Sheet"; "slides" -> "Deck"; "design" -> "Design"; "dashboard" -> "Dashboard"
        "ops" -> "Ops sheet"; "motion" -> "Motion"; "page" -> "Page"; "card" -> "Card"; "diagram" -> "Diagram"
        "image" -> "Image"; "code" -> "Code"; "pdf" -> "PDF"; "video" -> "Video"
        else -> "Artifact"
    }

    @DrawableRes
    fun icon(kind: String): Int = when (kind) {
        "doc" -> DesignR.drawable.deck_docs; "sheet" -> DesignR.drawable.deck_sheets; "slides" -> DesignR.drawable.deck_slides
        "design" -> DesignR.drawable.deck_design; "dashboard" -> DesignR.drawable.deck_dashboard; "ops" -> DesignR.drawable.deck_ops
        "motion" -> DesignR.drawable.deck_motion; "page" -> DesignR.drawable.deck_page; "card" -> DesignR.drawable.deck_card
        "diagram" -> DesignR.drawable.deck_diagram; "image" -> DesignR.drawable.deck_image; "code" -> DesignR.drawable.deck_code
        "pdf" -> DesignR.drawable.deck_pdf; "video" -> DesignR.drawable.deck_video
        else -> DesignR.drawable.deck_docs
    }

    /** Kinds the editor handles on the phone (iOS: owner or editor, and doc, sheet or slides). */
    fun editable(kind: String) = kind == "doc" || kind == "sheet" || kind == "slides"

    /** Kinds with "PDF from template" (iOS export tab). */
    fun pdfTemplate(kind: String) = kind in setOf("doc", "sheet", "ops", "dashboard")

    /** Kinds with "Save or share file". */
    fun fileExport(kind: String) = kind == "pdf" || kind == "image" || kind == "video"
}

/** iOS ArtifactsV2Client error copy, by status. */
fun artifactError(r: ApiResult<*>): String = when (r) {
    is ApiResult.HttpError -> when (r.status) {
        401 -> "Sign in to see your artifacts."
        403 -> "You don’t have access to do that."
        404 -> "This artifact doesn’t exist or isn’t shared with you."
        409 -> "Someone saved a newer version."
        413 -> "This artifact is too large (16 MB max)."
        else -> "Something went wrong. Try again."
    }
    is ApiResult.Network -> "You’re offline. Check your connection and try again."
    else -> "Something went wrong. Try again."
}

object ArtifactsV2Client {
    private fun url(path: String, query: Map<String, String?> = emptyMap()): HttpUrl {
        val base = LibraryNet.config.cloudBaseUrl.toString().trimEnd('/')
        val b = ("$base/api/v2/$path").toHttpUrlOrNull()!!.newBuilder()
        query.forEach { (k, v) -> if (v != null) b.addQueryParameter(k, v) }
        return b.build()
    }

    private suspend fun call(method: String, path: String, body: JsonElement? = null, query: Map<String, String?> = emptyMap()): ApiResult<JsonElement> {
        val r = ApiCall.execute(
            LibraryNet.okHttp, ApiJson.default,
            ApiCall.request(method, url(path, query), body?.toString() ?: if (method == "GET" || method == "DELETE") null else "{}"),
            JsonElement.serializer(),
        )
        return if (r is ApiResult.Decode && method != "GET") ApiResult.Ok(JsonNull) else r
    }

    private inline fun <T> ApiResult<JsonElement>.map(f: (JsonElement) -> T): ApiResult<T> = when (this) {
        is ApiResult.Ok -> ApiResult.Ok(f(value))
        is ApiResult.HttpError -> this
        is ApiResult.Network -> this
        is ApiResult.Decode -> this
    }

    suspend fun list(scope: String, kind: String?, q: String?, cursor: String?, archived: Boolean): ApiResult<ArtifactListPage> =
        call(
            "GET", "artifacts",
            query = mapOf(
                "scope" to scope, "limit" to "50", "kind" to kind, "q" to q?.takeIf { it.isNotBlank() },
                "cursor" to cursor, "archived" to if (archived) "true" else null,
            ),
        ).map { el ->
            val o = el.obj
            ArtifactListPage(o.list("items").mapNotNull(::summary), o.str("next_cursor"), o.bool("archive_supported") ?: false)
        }

    /** `POST api/v2/artifacts` (iOS ArtifactV2CreateInput: kind, title, body, body_format, meta, origin, capabilities). */
    suspend fun create(input: JsonObject): ApiResult<ArtifactV2?> = call("POST", "artifacts", input).map(::full)

    suspend fun get(id: String): ApiResult<ArtifactV2?> = call("GET", "artifacts/${LibraryNet.escape(id)}").map(::full)

    /** POST versions: `{base_version, body, body_format, meta, author:"user"}`; 409 = someone saved first. */
    suspend fun saveVersion(a: ArtifactV2, body: String, bodyFormat: String): ApiResult<ArtifactV2?> =
        call(
            "POST", "artifacts/${LibraryNet.escape(a.id)}/versions",
            buildJsonObject {
                put("base_version", a.version)
                put("body", body)
                put("body_format", bodyFormat)
                put("meta", a.meta ?: JsonObject(emptyMap()))
                put("author", "user")
            },
        ).map(::full)

    suspend fun setArchived(id: String, archived: Boolean): ApiResult<JsonElement> =
        call("PUT", "artifacts/${LibraryNet.escape(id)}/library", buildJsonObject { put("archived", archived) })

    suspend fun delete(id: String): ApiResult<JsonElement> = call("DELETE", "artifacts/${LibraryNet.escape(id)}")

    /** `GET api/v2/artifacts/:id/app` -> `{app}`; 404 when the artifact is not an app yet. */
    suspend fun app(id: String): ApiResult<JsonElement> = call("GET", "artifacts/${LibraryNet.escape(id)}/app")

    /** `PUT api/v2/artifacts/:id/app {name, icon, tier, install_scope}` (web `createMakeAppApi.put`): Make it an app. */
    suspend fun putApp(id: String, name: String, tier: String, installScope: String): ApiResult<JsonElement> =
        call("PUT", "artifacts/${LibraryNet.escape(id)}/app", buildJsonObject {
            put("name", name); put("icon", kotlinx.serialization.json.JsonNull); put("tier", tier); put("install_scope", installScope)
        })

    /** `GET api/v2/org/artifact-settings` → which creation tiles the org allows (iOS allows(kind:)). */
    suspend fun allowedKinds(): (String) -> Boolean {
        val r = call("GET", "org/artifact-settings")
        val o = (r as? ApiResult.Ok)?.value.obj ?: return { true }
        val enabled = o.bool("enabled") ?: true
        val templates = o.child("templates")
        return { kind -> enabled && (templates.bool(kind) ?: true) }
    }

    suspend fun comments(id: String): ApiResult<List<ArtifactComment>> =
        call("GET", "artifacts/${LibraryNet.escape(id)}/comments").map { el -> el.listUnder("items").mapNotNull(::comment) }

    suspend fun addComment(id: String, body: String, parentId: String?): ApiResult<JsonElement> =
        call("POST", "artifacts/${LibraryNet.escape(id)}/comments", buildJsonObject { put("body", body); if (parentId != null) put("parent_id", parentId) })

    suspend fun resolveComment(id: String, commentId: String, resolved: Boolean): ApiResult<JsonElement> =
        call("PATCH", "artifacts/${LibraryNet.escape(id)}/comments/${LibraryNet.escape(commentId)}", buildJsonObject { put("resolved", resolved) })

    suspend fun deleteComment(id: String, commentId: String): ApiResult<JsonElement> =
        call("DELETE", "artifacts/${LibraryNet.escape(id)}/comments/${LibraryNet.escape(commentId)}")

    // MARK: decode

    fun summary(o: JsonObject?): ArtifactSummary? {
        o ?: return null
        val id = o.str("id") ?: return null
        val owner = o.child("owner")
        val origin = o.child("origin")
        return ArtifactSummary(
            id = id,
            kind = o.str("kind") ?: "doc",
            title = o.str("title")?.takeIf { it.isNotBlank() } ?: "Untitled",
            owner = ArtifactOwner(owner.str("id") ?: "", owner.str("name"), owner.str("image_url")),
            updatedAt = o.str("updated_at") ?: "",
            currentVersion = o.num("current_version")?.toInt() ?: 1,
            visibility = o.str("visibility") ?: "private",
            myAccess = o.str("my_access") ?: "view",
            origin = origin?.let { ArtifactOrigin(it.str("surface"), it.str("session_id"), it.str("message_id"), it.str("legacy_source"), it.str("legacy_id"), it.str("project_title"), it.str("session_title") ?: it.str("chat_title")) },
            thumbnailUrl = o.str("thumbnail_url"),
            planStatus = o.child("plan").str("status"),
        )
    }

    private fun full(el: JsonElement): ArtifactV2? {
        val o = el.obj ?: return null
        val s = summary(o) ?: return null
        val v = o.child("version")
        return ArtifactV2(
            summary = s,
            version = v.num("version")?.toInt() ?: s.currentVersion,
            body = v.str("body") ?: "",
            bodyFormat = v.str("body_format") ?: "text/plain",
            meta = v.child("meta"),
        )
    }

    private fun comment(o: JsonObject): ArtifactComment? {
        val id = o.str("id") ?: return null
        return ArtifactComment(
            id, o.str("parent_id"), o.str("author_id") ?: "", o.str("author_name"), o.str("body") ?: "",
            o.str("resolved_at"), o.str("created_at") ?: "", o.str("agent_name"),
        )
    }
}

/** The web app the live view and "Open in browser" use (iOS ArtifactLiveWebView.webOrigin). */
object ArtifactWeb {
    const val ORIGIN = "https://ai.allternit.com"

    fun viewerUrl(id: String): String = "$ORIGIN/a/${LibraryNet.escape(id)}"

    /** iOS ArtifactWebRoute.window: the artifact's own editor page. */
    fun windowUrl(id: String, dark: Boolean): String = "$ORIGIN/a/${LibraryNet.escape(id)}/edit?theme=${theme(dark)}"

    /** iOS ArtifactWebRoute.office. */
    fun officeUrl(product: String, artifactId: String, dark: Boolean): String =
        "$ORIGIN/editor/office/${LibraryNet.escape(product)}/${LibraryNet.escape(artifactId)}?theme=${theme(dark)}"

    /** iOS ArtifactWebRoute.design: the design canvas (`source=project` when opened from Projects). */
    fun designUrl(projectId: String, title: String, fromProject: Boolean, dark: Boolean): String =
        "$ORIGIN/editor/design/${LibraryNet.escape(projectId)}?theme=${theme(dark)}&title=${LibraryNet.escape(title)}" +
            if (fromProject) "&source=project" else ""

    /** The link Share copies (iOS ArtifactCollabRules: `origin/?artifact=<id>`). */
    fun shareLink(id: String): String = "$ORIGIN/?artifact=${LibraryNet.escape(id)}"

    private fun theme(dark: Boolean) = if (dark) "dark" else "light"

    /** iOS isAppPage: only these web-app paths may load in the live view's main frame. */
    fun isAppPage(url: android.net.Uri): Boolean =
        url.scheme == "https" && url.host == "ai.allternit.com" &&
            listOf("/a/", "/editor/", "/embed/editor/").any { (url.path ?: "").startsWith(it) }
}
