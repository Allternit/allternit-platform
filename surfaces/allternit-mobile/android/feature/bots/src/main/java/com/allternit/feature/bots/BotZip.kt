package com.allternit.feature.bots

import com.allternit.core.api.ApiJson
import com.allternit.feature.artifacts.str
import com.allternit.feature.chat.ChatBackend
import com.allternit.feature.chat.RuntimeCall
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.put
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.nio.charset.CharacterCodingException
import java.nio.charset.CodingErrorAction
import java.time.Instant
import java.util.UUID
import java.util.zip.ZipException
import java.util.zip.ZipInputStream

// -----------------------------------------------------------------------------
// Bot .zip packages (iOS Features/Connect/BotZipImport.swift; allternit-ai
// src/lib/bots/bot-import.ts previewBotImport / importBotFromZip). A package
// holds a root bot.json that satisfies the web's botPackageSchema plus optional
// memory/, skills/, tasks/, docs/, identity/, connectors/ and secrets/ folders and
// root files, copied into the bot's workspace on the computer (agents/<id>/…).
// -----------------------------------------------------------------------------

class BotZipException(message: String) : Exception(message)

internal object BotZip {
    const val MAX_TOTAL_BYTES = 50 * 1024 * 1024
    val knownDirectories = listOf("memory", "skills", "tasks", "docs", "identity", "connectors", "secrets")

    data class Entry(val path: String, val data: ByteArray)

    /** What the web's import preview shows (BotImportPreview). */
    data class Preview(
        val name: String, val description: String, val category: String?,
        val hasMemory: Boolean, val hasSkills: Boolean, val hasTasks: Boolean, val hasDocs: Boolean,
        val hasIdentity: Boolean, val hasConnectors: Boolean, val hasSecrets: Boolean, val hasVmOperator: Boolean,
        val fileCount: Int, val warnings: List<String>,
    ) {
        val contents: List<String>
            get() = listOfNotNull(
                "Memory".takeIf { hasMemory }, "Skills".takeIf { hasSkills }, "Tasks".takeIf { hasTasks }, "Docs".takeIf { hasDocs },
                "Identity".takeIf { hasIdentity }, "Connectors".takeIf { hasConnectors }, "Secret slots".takeIf { hasSecrets }, "Computer use".takeIf { hasVmOperator },
            )
    }

    class Package(val bot: JsonObject, val files: List<Entry>, val preview: Preview) {
        /** The known folders, then root files; other folders are left out, like the web. */
        val workspaceFiles: List<Entry>
            get() = files.filter { f -> knownDirectories.any { f.path.startsWith("$it/") && f.path.length > it.length + 1 } } + files.filter { '/' !in it.path }
    }

    /** A zip starts with a local file header (PK\3\4) or, when empty, the end record (PK\5\6). */
    fun looksLikeZip(b: ByteArray) = b.size >= 4 && b[0] == 0x50.toByte() && b[1] == 0x4B.toByte() && (b[2] == 0x03.toByte() || b[2] == 0x05.toByte())

    fun entries(data: ByteArray): List<Entry> {
        val tooBig = BotZipException("This bot package is too big. Packages can hold at most 50 MB of files.")
        if (data.size > MAX_TOTAL_BYTES) throw tooBig
        if (!looksLikeZip(data)) throw BotZipException("This isn’t a zip file. Pick a bot package (.zip) exported from Allternit.")
        val out = mutableListOf<Entry>()
        var total = 0L
        try {
            ZipInputStream(ByteArrayInputStream(data)).use { zip ->
                while (true) {
                    val e = zip.nextEntry ?: break
                    val name = e.name.replace('\\', '/')
                    if (e.isDirectory || name.endsWith("/") || name == "__MACOSX" || name.startsWith("__MACOSX/")) continue
                    if (name.startsWith("/") || name.split('/').contains("..")) {
                        throw BotZipException("This zip has a file path that points outside the package ($name), so it can’t be imported.")
                    }
                    val buf = ByteArrayOutputStream()
                    val chunk = ByteArray(16 * 1024)
                    while (true) {
                        val n = zip.read(chunk)
                        if (n < 0) break
                        total += n
                        if (total > MAX_TOTAL_BYTES) throw tooBig
                        buf.write(chunk, 0, n)
                    }
                    out.add(Entry(name, buf.toByteArray()))
                }
            }
        } catch (e: BotZipException) {
            throw e
        } catch (e: ZipException) {
            val m = e.message.orEmpty().lowercase()
            throw BotZipException(
                when {
                    "encrypt" in m -> "This zip is password-protected. Export the bot again without a password."
                    "compression method" in m || "only deflated" in m -> "This zip uses a compression type the phone can’t read. Export it again from Allternit."
                    else -> "This zip file is damaged and can’t be read."
                },
            )
        } catch (e: java.io.IOException) {
            throw BotZipException("This zip file is damaged and can’t be read.")
        }
        return out
    }

    /** Reads a zip and checks bot.json against the web's botPackageSchema. */
    fun load(data: ByteArray): Package {
        val all = entries(data)
        val botFile = all.firstOrNull { it.path == "bot.json" } ?: throw BotZipException("This zip has no bot.json at the top level. Pick a bot package exported from Allternit.")
        val bot = runCatching { ApiJson.default.parseToJsonElement(botFile.data.decodeToString()) as? JsonObject }.getOrNull()
            ?: throw BotZipException("The bot.json in this zip isn’t valid JSON.")
        val problems = validate(bot)
        if (problems.isNotEmpty()) {
            val shown = problems.take(4).joinToString("\n") { "• $it" }
            val more = if (problems.size > 4) "\n…and ${problems.size - 4} more" else ""
            throw BotZipException("This isn’t a valid bot package:\n$shown$more")
        }
        val files = all.filter { it.path != "bot.json" }
        fun hasDir(n: String) = files.any { it.path.startsWith("$n/") }
        fun nonEmptyArray(k: String) = (bot[k] as? JsonArray)?.isNotEmpty() == true
        val warnings = if (!hasDir("memory") && !hasDir("skills") && !hasDir("docs")) listOf("This package has no memory, skills or docs folders.") else emptyList()
        val profile = bot["botProfile"] as? JsonObject
        val preview = Preview(
            name = profile.str("displayName").orEmpty(), description = bot.str("description").orEmpty(), category = profile.str("botCategory"),
            hasMemory = hasDir("memory"), hasSkills = hasDir("skills"), hasTasks = hasDir("tasks"), hasDocs = hasDir("docs"),
            hasIdentity = hasDir("identity") || bot["identityChannels"] is JsonObject,
            hasConnectors = nonEmptyArray("connectorBindings"), hasSecrets = nonEmptyArray("secretRefs"),
            hasVmOperator = ((bot["vmOperator"] as? JsonObject)?.get("enabled") as? JsonPrimitive)?.booleanOrNull == true,
            fileCount = files.size, warnings = warnings,
        )
        return Package(bot, files, preview)
    }

    private val TYPES = listOf("orchestrator", "sub-agent", "worker", "specialist", "reviewer", "assistant", "bot")
    private val STATUSES = listOf("idle", "running", "paused", "error")
    private val CATEGORIES = listOf("research", "code", "writing", "data", "sales", "design", "ops", "custom")

    /** The required fields and limits of allternit-ai botPackageSchema (iOS BotPackageValidator). */
    fun validate(bot: JsonObject): List<String> {
        val errors = mutableListOf<String>()
        fun present(o: JsonObject, k: String): JsonElement? = o[k]?.takeIf { it !is JsonNull }
        fun text(e: JsonElement?): String? = (e as? JsonPrimitive)?.takeIf { it.isString }?.content
        fun string(o: JsonObject, k: String, path: String, required: Boolean, min: Int = 0, max: Int? = null, trim: Boolean = false) {
            val v = present(o, k) ?: run { if (required) errors.add("$path: Required"); return }
            var s = text(v) ?: run { errors.add("$path: Expected text"); return }
            if (trim) s = s.trim()
            if (s.length < min) errors.add("$path: Can’t be empty")
            if (max != null && s.length > max) errors.add("$path: At most $max characters")
        }
        fun number(k: String, int: Boolean, lo: Double, hi: Double) {
            val v = present(bot, k) ?: run { errors.add("$k: Required"); return }
            val n = (v as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull ?: run { errors.add("$k: Expected a number"); return }
            if (int && n != Math.rint(n)) errors.add("$k: Expected a whole number")
            fun f(x: Double) = if (x == Math.rint(x)) x.toLong().toString() else x.toString()
            if (n < lo || n > hi) errors.add("$k: Must be from ${f(lo)} to ${f(hi)}")
        }
        fun stringArray(o: JsonObject, k: String, path: String, required: Boolean) {
            val v = present(o, k) ?: run { if (required) errors.add("$path: Required"); return }
            val items = v as? JsonArray ?: run { errors.add("$path: Expected a list"); return }
            if (items.any { text(it) == null }) errors.add("$path: Expected a list of text")
        }
        fun oneOf(o: JsonObject, k: String, path: String, options: List<String>, required: Boolean) {
            val v = present(o, k) ?: run { if (required) errors.add("$path: Required"); return }
            if (text(v) !in options) errors.add("$path: Must be one of ${options.joinToString(", ")}")
        }
        fun shape(k: String, array: Boolean) {
            val v = present(bot, k) ?: return
            if (array && v !is JsonArray || !array && v !is JsonObject) errors.add("$k: Expected ${if (array) "a list" else "an object"}")
        }
        string(bot, "id", "id", true, min = 1)
        string(bot, "name", "name", true, min = 1, max = 100)
        string(bot, "description", "description", true, max = 2000)
        oneOf(bot, "type", "type", TYPES, true)
        string(bot, "model", "model", true, min = 1)
        string(bot, "provider", "provider", true, min = 1, trim = true)
        stringArray(bot, "capabilities", "capabilities", true)
        string(bot, "systemPrompt", "systemPrompt", false, max = 50000)
        stringArray(bot, "tools", "tools", true)
        number("maxIterations", true, 1.0, 100.0)
        number("temperature", false, 0.0, 2.0)
        oneOf(bot, "status", "status", STATUSES, true)
        string(bot, "createdAt", "createdAt", true)
        string(bot, "updatedAt", "updatedAt", true)
        string(bot, "category", "category", false)
        stringArray(bot, "tags", "tags", false)
        shape("config", false)
        if ((bot["isBot"] as? JsonPrimitive)?.booleanOrNull != true) errors.add("isBot: Must be true")
        val profile = bot["botProfile"] as? JsonObject
        if (profile != null) {
            string(profile, "displayName", "botProfile.displayName", true, min = 1, max = 120)
            string(profile, "tagline", "botProfile.tagline", false, max = 240)
            string(profile, "welcomeMessage", "botProfile.welcomeMessage", false, max = 2000)
            present(profile, "starterPrompts")?.let { p ->
                val items = p as? JsonArray
                if (items == null) errors.add("botProfile.starterPrompts: Expected a list")
                else {
                    if (items.size > 12) errors.add("botProfile.starterPrompts: At most 12")
                    if (items.any { (text(it)?.length ?: 0) !in 1..500 }) errors.add("botProfile.starterPrompts: Each prompt must be 1 to 500 characters")
                }
            }
            present(profile, "accentColor")?.let { v ->
                if (!Regex("^(#[0-9A-Fa-f]{3,8}|rgb\\(.*\\)|hsl\\(.*\\))$").matches(text(v).orEmpty())) errors.add("botProfile.accentColor: accentColor must be a hex, rgb, or hsl color string")
            }
            present(profile, "groupChatEnabled")?.let { v -> if ((v as? JsonPrimitive)?.booleanOrNull == null) errors.add("botProfile.groupChatEnabled: Expected true or false") }
            string(profile, "defaultPresetId", "botProfile.defaultPresetId", false)
            oneOf(profile, "botCategory", "botProfile.botCategory", CATEGORIES, false)
        } else errors.add("botProfile: Required")
        shape("harness", false)
        string(bot, "brainId", "brainId", false)
        shape("brain", false)
        shape("connectorBindings", true)
        shape("secretRefs", true)
        shape("messagingConfig", false)
        shape("identityChannels", false)
        shape("vmOperator", false)
        return errors
    }

    private val PRIMITIVES = listOf(
        "connectorBindings" to "connector_bindings", "secretRefs" to "secret_refs", "messagingConfig" to "messaging_config",
        "identityChannels" to "identity_channels", "vmOperator" to "vm_operator",
    )

    /** bot-import buildCreateInput → agent.service createAgent's REST body. */
    fun createBody(bot: JsonObject, id: String, now: String, makeMain: Boolean): JsonObject {
        val profile = (bot["botProfile"] as? JsonObject).orEmptyObj().let { if (makeMain) JsonObject(it + ("chiefOfStaff" to JsonPrimitive(true))) else it }
        val displayName = profile.str("displayName") ?: bot.str("name") ?: "Imported bot"
        val config = (bot["config"] as? JsonObject).orEmptyObj().toMutableMap()
        config["importedAt"] = JsonPrimitive(now)
        config["importedFrom"] = JsonPrimitive("zip")
        config["agentSource"] = JsonPrimitive("organization")
        config["isBot"] = JsonPrimitive(true)
        config["botProfile"] = profile
        PRIMITIVES.forEach { (camel, _) -> bot[camel]?.takeIf { it !is JsonNull }?.let { config[camel] = it } }
        val body = mutableMapOf<String, JsonElement>(
            "id" to JsonPrimitive(id),
            "name" to JsonPrimitive(displayName),
            "description" to (bot["description"] ?: JsonPrimitive("")),
            "type" to (bot["type"] ?: JsonPrimitive("worker")),
            "model" to (bot["model"] ?: JsonPrimitive("")),
            "provider" to (bot["provider"] ?: JsonPrimitive("")),
            "capabilities" to (bot["capabilities"] ?: JsonArray(emptyList())),
            "tools" to (bot["tools"] ?: JsonArray(emptyList())),
            "max_iterations" to (bot["maxIterations"] ?: JsonPrimitive(10)),
            "temperature" to (bot["temperature"] ?: JsonPrimitive(0.7)),
            "config" to JsonObject(config),
            "trust_tier" to JsonPrimitive("standard"),
            "enabled_modes" to JsonArray(listOf("chat", "cowork", "code", "design", "browser").map { JsonPrimitive(it) }),
            "category" to (bot["category"] ?: JsonPrimitive("general")),
            "tags" to (bot["tags"] ?: JsonArray(emptyList())),
            "is_bot" to JsonPrimitive(true),
            "bot_profile" to profile,
        )
        bot["systemPrompt"]?.takeIf { it !is JsonNull }?.let { body["system_prompt"] = it }
        PRIMITIVES.forEach { (camel, snake) -> bot[camel]?.takeIf { it !is JsonNull }?.let { body[snake] = it } }
        return JsonObject(body)
    }

    private fun JsonObject?.orEmptyObj(): JsonObject = this ?: JsonObject(emptyMap())

    private fun utf8(data: ByteArray): String? = try {
        Charsets.UTF_8.newDecoder().onMalformedInput(CodingErrorAction.REPORT).onUnmappableCharacter(CodingErrorAction.REPORT).decode(java.nio.ByteBuffer.wrap(data)).toString()
    } catch (e: CharacterCodingException) { null }

    private suspend fun write(backend: ChatBackend, path: String, content: String): String? {
        val body = buildJsonObject { put("path", path); put("content", content) }.toString()
        return when (val r = backend.runtimeJson("POST", "/api/v1/files/write", body)) {
            is RuntimeCall.Ok -> null
            RuntimeCall.NoComputer -> "no computer is connected"
            is RuntimeCall.Http -> com.allternit.core.design.FriendlyErrorMessage.from(r.message ?: r.code ?: "HTTP ${r.status}")
            is RuntimeCall.Failed -> r.message
        }
    }

    /**
     * web importBotFromZip: a fresh id, the web's create body, then the package's folders and root
     * files copied under agents/<id>/ on the computer plus the workspace manifest. Per-file failures
     * become warnings; only the bot create failing fails. Returns the bot id and the warnings.
     */
    suspend fun importPackage(pkg: Package, backend: ChatBackend?, makeMain: Boolean): Pair<String, List<String>> {
        val files = pkg.workspaceFiles
        // The workspace lives on the computer: check one answers before making the bot.
        if (files.isNotEmpty()) {
            val noComputer = BotZipException("This package has files that go on your computer, and no computer is connected. Open Allternit Desktop or wake your cloud computer, then try again.")
            if (backend == null) throw noComputer
            if (backend.runtimeJson("GET", "/api/v1/files/list?path=agents&details=true") == RuntimeCall.NoComputer) throw noComputer
        }
        val sourceId = pkg.bot.str("id") ?: "bot"
        val freshId = "bot_${sourceId}_${UUID.randomUUID().toString().lowercase().take(8)}"
        val botId = when (val r = BotsApi.createAgent(createBody(pkg.bot, freshId, Instant.now().toString(), makeMain))) {
            is BotsResult.Ok -> r.value
            is BotsResult.Failed -> throw BotZipException("Couldn’t create the bot: ${r.message}")
        }
        val warnings = mutableListOf<String>()
        val root = "agents/$botId"
        if (backend != null && files.isNotEmpty()) {
            files.forEach { f ->
                val text = utf8(f.data)
                if (text == null) warnings.add("Skipped ${f.path}: it isn’t a text file.")
                else write(backend, "$root/${f.path}", text)?.let { warnings.add("Couldn’t copy ${f.path}: $it") }
            }
            val millis = JsonPrimitive(System.currentTimeMillis())
            val manifest = buildJsonObject {
                put("agentId", botId); put("agentName", (pkg.bot["botProfile"] as? JsonObject).str("displayName") ?: sourceId)
                put("createdAt", millis); put("files", JsonArray(files.map { JsonPrimitive(it.path) })); put("id", botId)
                put("lastModified", millis); put("template", "allternit-standard"); put("version", "1.0.0")
            }
            write(backend, "$root/.allternit/manifest.json", manifest.toString())?.let { warnings.add("Couldn’t write the workspace manifest: $it") }
        }
        return botId to warnings
    }
}

@Suppress("unused") private fun JsonElement.textOrNull(): String? = (this as? JsonPrimitive)?.contentOrNull
