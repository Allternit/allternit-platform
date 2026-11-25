package com.allternit.feature.bots

import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.feature.artifacts.LibraryNet
import com.allternit.feature.artifacts.bool
import com.allternit.feature.artifacts.child
import com.allternit.feature.artifacts.list
import com.allternit.feature.artifacts.num
import com.allternit.feature.artifacts.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

// Team map (Island bar + Team map, lane 38). A port of iOS Features/Office/TeamMap/TeamMapModels.swift +
// TeamMapClient.swift (platform #3038): GET /api/v1/team/map and the five writes. Copy and rules match iOS
// word for word; do not add Android-only rows.

data class TeamAppRef(val kind: String, val ref: String, val name: String?) {
    val displayName: String get() = name?.takeIf { it.isNotEmpty() } ?: ref
    val key: String get() = "$kind:$ref"
}

data class TeamNode(
    val id: String, val kind: String, val name: String, val role: String?, val isMain: Boolean?,
    val brain: String?, val state: String, val asks: Int, val jobs: Int,
) {
    val isBot: Boolean get() = kind == "bot"
    val userId: String? get() = if (id.startsWith("p:")) id.removePrefix("p:") else null
    val subtitle: String
        get() {
            if (!isBot) return role?.takeIf { it.isNotEmpty() }?.replaceFirstChar { it.uppercase() } ?: "Member"
            val kindText = if (isMain == true) "Main bot" else if (id.startsWith("v:")) "Connected bot" else "Bot"
            return brain?.takeIf { it.isNotEmpty() }?.let { "$kindText · $it" } ?: kindText
        }
    val initials: String
        get() = name.split(" ").filter { it.isNotEmpty() }.take(2).map { it.first() }.joinToString("").uppercase().ifEmpty { "?" }
}

data class TeamEdge(val from: String, val to: String, val kind: String)

data class TeamGrant(val id: String, val bot: String, val app: TeamAppRef, val level: String, val ceiling: String?) {
    val botKind: String get() = if (bot.startsWith("v:")) "vendor" else "agent"
    val botId: String get() = bot.drop(2)
}

data class TeamRule(
    val id: String, val text: String, val enabled: Boolean, val scopeKind: String, val scopeRef: String?,
    val approverKind: String?, val approverUserId: String?, val locked: Boolean,
) {
    val scopeKey: String
        get() = when (scopeKind) {
            "person" -> scopeRef?.let { "p:$it" } ?: "org"
            "bot" -> scopeRef ?: "org"
            else -> "org"
        }
}

data class TeamResolve(val method: String, val path: String, val body: JsonObject?)

data class TeamAsk(
    val id: String, val store: String, val title: String, val summary: String?, val actor: String?,
    val approverUserId: String?, val ruleId: String?, val app: TeamAppRef?, val tool: String?, val risk: String?,
    val state: String, val createdAt: String?, val jobId: String?, val resolve: TeamResolve?,
)

data class TeamMapJob(val jobId: String, val actor: String?, val waitingOn: String?, val app: TeamAppRef?, val tool: String?, val title: String?, val state: String)

object TeamLevel {
    fun label(level: String): String = when (level) {
        "browser" -> "Browser"; "app" -> "App"; "full" -> "Full"
        else -> level.replaceFirstChar { it.uppercase() }
    }
    fun rank(level: String): Int = when (level) { "browser" -> 0; "app" -> 1; "full" -> 2; else -> -1 }
}

object TeamNodeState {
    fun label(state: String): String = when (state) {
        "working" -> "Working"; "needs_input" -> "Needs input"; "waiting_approval" -> "Waiting on approval"
        "offline" -> "Offline"; else -> "Idle"
    }
}

fun teamCount(n: Int, one: String, many: String? = null): String = "$n ${if (n == 1) one else (many ?: one + "s")}"

class TeamMap(
    val orgId: String?, val orgName: String?, val me: String,
    val nodes: List<TeamNode>, val edges: List<TeamEdge>, val grants: List<TeamGrant>,
    val rules: List<TeamRule>, val asks: List<TeamAsk>, val jobs: List<TeamMapJob>, val partial: List<String>,
) {
    val hasOrg: Boolean get() = orgId != null
    fun node(id: String?): TeamNode? = id?.let { i -> nodes.firstOrNull { it.id == i } }
    fun name(id: String?): String? = node(id)?.name
    val people: List<TeamNode> get() = nodes.filter { !it.isBot }
    val bots: List<TeamNode> get() = nodes.filter { it.isBot }
    fun owner(of: String): String? = edges.firstOrNull { it.kind == "owns" && it.to == of }?.from
    fun grants(of: String): List<TeamGrant> = grants.filter { it.bot == of }
    fun rule(id: String?): TeamRule? = id?.let { i -> rules.firstOrNull { it.id == i } }
    val partialNote: String? get() = if (partial.isEmpty()) null else "Some approvals or apps may be missing."

    fun requesterName(ask: TeamAsk): String? {
        val actor = ask.actor ?: return null
        return if (actor.startsWith("p:")) name(actor) else name(owner(actor))
    }
    fun approverName(ask: TeamAsk): String? = ask.approverUserId?.let { name("p:$it") }
    fun levelLine(ask: TeamAsk): String? {
        val actor = ask.actor ?: return null
        val app = ask.app ?: return null
        val g = grants.firstOrNull { it.bot == actor && it.app.kind == app.kind && it.app.ref == app.ref } ?: return null
        return "${g.app.displayName} at ${TeamLevel.label(g.level)}"
    }

    val viewer: TeamNode? get() = node(me)
    val viewerIsAdmin: Boolean get() = viewer?.role == "owner" || viewer?.role == "admin"

    fun parent(of: String): String? {
        val n = node(of) ?: return null
        if (!n.isBot) return edges.firstOrNull { it.kind == "reports_to" && it.from == of }?.to
        return edges.firstOrNull { it.kind == "parent" && it.to == of }?.from
            ?: edges.firstOrNull { it.kind == "owns" && it.to == of }?.from
    }
    private fun rank(n: TeamNode): Int = if (n.id == me) 0 else if (!n.isBot) 1 else if (n.isMain == true) 2 else 3
    fun ordered(list: List<TeamNode>): List<TeamNode> =
        list.sortedWith(compareBy<TeamNode> { rank(it) }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })
    val roots: List<TeamNode>
        get() = ordered(nodes.filter { n -> val p = parent(n.id); p == null || node(p) == null || createsCycle(n.id) })
    private fun createsCycle(id: String): Boolean {
        val seen = mutableSetOf(id)
        var cursor = parent(id)
        while (cursor != null && node(cursor) != null) {
            if (cursor in seen) return cursor == id
            seen += cursor
            cursor = parent(cursor)
        }
        return false
    }
    fun children(of: String): List<TeamNode> = ordered(nodes.filter { it.id != of && parent(it.id) == of && !createsCycle(it.id) })

    fun waitingOn(botId: String): String? {
        jobs.firstOrNull { it.actor == botId && it.waitingOn != null && it.state == "waiting_approval" }?.let { return it.waitingOn }
        if (node(botId)?.state != "waiting_approval") return null
        return asks.firstOrNull { it.actor == botId }?.approverUserId?.let { "p:$it" }
    }
    fun waitingOnMe(id: String): List<TeamNode> = bots.filter { waitingOn(it.id) == id }
    fun stateText(n: TeamNode): String = waitingOn(n.id)?.let { "Waiting on ${name(it) ?: "approval"}" } ?: TeamNodeState.label(n.state)

    fun asks(n: TeamNode): List<TeamAsk> {
        val uid = n.userId
        return asks.filter { it.actor == n.id || (uid != null && it.approverUserId == uid) }
    }
    fun rules(n: TeamNode): List<TeamRule> {
        val uid = n.userId
        val askRules = asks(n).mapNotNull { it.ruleId }.toSet()
        return rules.filter { r ->
            r.id in askRules || (r.scopeKind == "bot" && r.scopeRef == n.id) ||
                (r.scopeKind == "person" && uid != null && r.scopeRef == uid) || (uid != null && r.approverUserId == uid)
        }
    }
    fun rulesInScope(key: String): List<TeamRule> = rules.filter { it.scopeKind == "org" || it.scopeKey == key }
    fun canDecide(ask: TeamAsk): Boolean {
        if (ask.resolve == null) return false
        if (ask.approverUserId != null && me == "p:${ask.approverUserId}") return true
        return viewerIsAdmin
    }
    fun canEditLevels(botId: String): Boolean = viewerIsAdmin || owner(botId) == me
    val apps: List<TeamAppRef> get() = grants.map { it.app }.distinctBy { it.key }
    val currencyMark: String
        get() {
            for (a in asks) "${a.title} ${a.summary.orEmpty()}".firstOrNull { it in "€$£¥" }?.let { return it.toString() }
            return ""
        }
    val botCount: Int get() = bots.size
    val peopleCount: Int get() = people.size

    companion object {
        private fun app(o: JsonObject?): TeamAppRef? = o?.let { TeamAppRef(it.str("kind") ?: return null, it.str("ref") ?: return null, it.str("name")) }

        fun parse(e: JsonElement): TeamMap? {
            val o = e as? JsonObject ?: return null
            val me = o.str("me") ?: return null
            return TeamMap(
                orgId = o.child("org").str("id"), orgName = o.child("org").str("name"), me = me,
                nodes = o.list("nodes").mapNotNull { n ->
                    TeamNode(
                        n.str("id") ?: return@mapNotNull null, n.str("kind") ?: "person", n.str("name") ?: return@mapNotNull null,
                        n.str("role"), n.bool("is_main"), n.str("brain"), n.str("state") ?: "idle",
                        n.child("counts").num("asks")?.toInt() ?: 0, n.child("counts").num("jobs")?.toInt() ?: 0,
                    )
                },
                edges = o.list("edges").mapNotNull { TeamEdge(it.str("from") ?: return@mapNotNull null, it.str("to") ?: return@mapNotNull null, it.str("kind").orEmpty()) },
                grants = o.list("grants").mapNotNull { g ->
                    TeamGrant(g.str("id") ?: return@mapNotNull null, g.str("bot") ?: return@mapNotNull null, app(g.child("app")) ?: return@mapNotNull null, g.str("level") ?: "browser", g.str("ceiling"))
                },
                rules = o.list("rules").mapNotNull { r ->
                    TeamRule(
                        r.str("id") ?: return@mapNotNull null, r.str("text").orEmpty(), r.bool("enabled") ?: true,
                        r.child("scope").str("kind") ?: "org", r.child("scope").str("ref"),
                        r.child("approver").str("kind"), r.child("approver").str("user_id"), r.bool("locked") ?: false,
                    )
                },
                asks = o.list("asks").mapNotNull { a ->
                    TeamAsk(
                        a.str("id") ?: return@mapNotNull null, a.str("store").orEmpty(), a.str("title").orEmpty(), a.str("summary"), a.str("actor"),
                        a.str("approver_user_id"), a.str("rule_id"), app(a.child("app")), a.str("tool"), a.str("risk"), a.str("state") ?: "pending",
                        a.str("created_at"), a.str("job_id"),
                        a.child("resolve")?.let { r -> TeamResolve(r.str("method") ?: "POST", r.str("path") ?: return@let null, r.child("body")) },
                    )
                },
                jobs = o.list("jobs").mapNotNull { j ->
                    TeamMapJob(j.str("job_id") ?: return@mapNotNull null, j.str("actor"), j.str("waiting_on"), app(j.child("app")), j.str("tool"), j.str("title"), j.str("state").orEmpty())
                },
                partial = (o["partial"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content }.orEmpty(),
            )
        }
    }
}

/** The new-rule body (iOS TeamRuleInput). */
data class TeamRuleInput(
    val scopeKind: String, val scopeRef: String?, val appKind: String?, val appRef: String?, val tool: String,
    val field: String?, val op: String, val value: JsonElement?, val unitLabel: String?, val approverKind: String, val approverUserId: String?,
) {
    fun json(): JsonObject = buildJsonObject {
        put("scope_kind", scopeKind); scopeRef?.let { put("scope_ref", it) }
        appKind?.let { put("app_kind", it) }; appRef?.let { put("app_ref", it) }
        put("tool", tool); field?.let { put("field", it) }; put("op", op)
        value?.let { put("value", it) }; unitLabel?.let { put("unit_label", it) }
        put("approver_kind", approverKind); approverUserId?.let { put("approver_user_id", it) }
    }
}

/** iOS TeamMapError: every failure is one plain sentence. */
sealed class TeamMapError(val text: String) {
    object NoRuntime : TeamMapError("No Allternit computer is reachable. Connect one and try again.")
    object SignedOut : TeamMapError("You're signed out. Sign in again to see your team.")
    object StoreUnavailable : TeamMapError("The team list isn't available right now. Try again in a moment.")
    object CannotChangeNow : TeamMapError("Can't change this right now. Try again in a moment.")
    object NotAllowed : TeamMapError("Only the owner or an admin can change this.")
    object ReadOnly : TeamMapError("This ask can only be decided on the platform.")
    class Refused(text: String) : TeamMapError(text)

    companion object {
        private val CODE = Regex("^[a-z0-9_.-]+$")
        private val HTTP = Regex("^HTTP \\d+")
        private fun plain(text: String?): String? = text?.trim()?.takeIf { it.isNotEmpty() && !CODE.matches(it) && !HTTP.containsMatchIn(it) }

        fun from(status: Int, code: String?, message: String?): TeamMapError {
            when (code?.lowercase()) {
                "no_runtime", "runtime_unavailable", "runtime_offline" -> return NoRuntime
                "tier_check_unavailable" -> return CannotChangeNow
                "team_rules_unavailable", "team_map_failed", "team_approvals_failed", "team_access_failed",
                "team_write_failed", "team_grants_failed", "team_rules_failed" -> return StoreUnavailable
                "owner_or_admin_required", "bot_owner_or_admin_required" -> return NotAllowed
                "reports_to_cycle" -> return Refused("That would make a loop: someone would report to their own report.")
                "manager_not_member", "not_member" -> return Refused("That person isn't on this team.")
                "no_org" -> return Refused("Make a team first to set who reports to whom.")
                "above_ceiling" -> return Refused("That level is above what this app allows here. Pick a lower level.")
                "app_not_installed" -> return Refused("This app isn't installed for the bot's owner, so its level can't change.")
                "bot_not_found" -> return Refused("This bot isn't on the team anymore. Reload the map.")
                "rule_not_found" -> return Refused("This rule was deleted. Reload the map.")
                "invalid_rule" -> return Refused(plain(message) ?: "That rule doesn't work. Check each part and try again.")
            }
            return when {
                status == 401 -> SignedOut
                status == 403 -> NotAllowed
                status == 409 || status == 428 -> NoRuntime
                status >= 500 -> StoreUnavailable
                else -> Refused(plain(message) ?: "That didn't work. Try again.")
            }
        }
    }
}

sealed interface TeamResult<out T> {
    data class Ok<T>(val value: T) : TeamResult<T>
    data class Failed(val error: TeamMapError) : TeamResult<Nothing>
}

/** iOS TeamMapClient: allternit-api `/api/v1/team/*` with the app's bearer, through [LibraryNet]. */
object TeamMapClient {
    private fun <T> wrap(r: ApiResult<JsonElement>, map: (JsonElement) -> T?): TeamResult<T> = when (r) {
        is ApiResult.Ok -> map(r.value)?.let { TeamResult.Ok(it) } ?: TeamResult.Failed(TeamMapError.StoreUnavailable)
        is ApiResult.HttpError -> TeamResult.Failed(TeamMapError.from(r.status, r.code, r.message))
        // No answer at all: the runtime (computer) is not reachable.
        is ApiResult.Network -> TeamResult.Failed(TeamMapError.NoRuntime)
        is ApiResult.Decode -> TeamResult.Failed(TeamMapError.StoreUnavailable)
    }
    private fun unit(r: ApiResult<JsonElement>): TeamResult<Unit> = wrap(r) { Unit }

    suspend fun map(): TeamResult<TeamMap> = wrap(LibraryNet.get("team/map")) { TeamMap.parse(it) }

    suspend fun setLevel(grant: TeamGrant, level: String): TeamResult<Unit> = unit(LibraryNet.send("PUT", "team/grants", buildJsonObject {
        put("bot_kind", grant.botKind); put("bot_id", grant.botId); put("app_kind", grant.app.kind); put("app_ref", grant.app.ref); put("level", level)
    }))

    suspend fun setReportsTo(userId: String, managerUserId: String?): TeamResult<Unit> =
        unit(LibraryNet.send("PUT", "team/people/${LibraryNet.escape(userId)}", JsonObject(mapOf("reports_to_user_id" to (managerUserId?.let { JsonPrimitive(it) } ?: JsonNull)))))

    suspend fun createRule(input: TeamRuleInput): TeamResult<Unit> = unit(LibraryNet.send("POST", "team/rules", input.json()))

    suspend fun deleteRule(id: String): TeamResult<Unit> = unit(LibraryNet.send("DELETE", "team/rules/${LibraryNet.escape(id)}"))

    /** Each store's own route from `ask.resolve`, with `decision` added (no new write path). */
    suspend fun resolve(ask: TeamAsk, approve: Boolean): TeamResult<Unit> {
        val resolve = ask.resolve ?: return TeamResult.Failed(TeamMapError.ReadOnly)
        val method = resolve.method.uppercase()
        val url = (if (resolve.path.startsWith("/")) LibraryNet.absolute(resolve.path) else LibraryNet.url(resolve.path).toString()).toHttpUrlOrNull()
            ?: return TeamResult.Failed(TeamMapError.StoreUnavailable)
        val body = JsonObject((resolve.body ?: JsonObject(emptyMap())) + ("decision" to JsonPrimitive(if (approve) "approve" else "deny")))
        val request = ApiCall.request(method, url, if (method == "GET") null else body.toString())
        return when (val r = ApiCall.execute(LibraryNet.okHttp, ApiJson.default, request, JsonElement.serializer())) {
            is ApiResult.Decode -> TeamResult.Ok(Unit)
            else -> unit(r)
        }
    }
}
