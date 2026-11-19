package com.allternit.feature.shell

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.api.ApiCall
import com.allternit.core.api.ApiConfig
import com.allternit.core.api.ApiJson
import com.allternit.core.api.ApiResult
import com.allternit.core.api.gen.ModelThinking
import com.allternit.core.api.gen.RuntimeModel
import com.allternit.core.design.FriendlyErrorMessage
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull

/**
 * One row the "Pick a brain" sheet offers: a computer catalog model (`GET /api/v1/models`) or,
 * with no computer, an Allternit Cloud model from the public `{cloud}/v1/models` catalog as
 * `allternit/<alias>` (iOS ModelStore.cloudChatModels).
 */
internal data class PickerModel(
    val id: String,
    val name: String,
    val provider: String,
    val description: String?,
    val isDefault: Boolean,
    val hidden: Boolean,
    val rank: Long?,
    val thinking: ModelThinking?,
)

private fun RuntimeModel.toPicker() = PickerModel(
    id = id, name = name, provider = provider, description = description,
    isDefault = default == true, hidden = hidden == true, rank = rank, thinking = thinking,
)

/**
 * The composer's model selection (iOS ModelStore): the catalog plus the persisted choice and
 * favourites. One per shell; step 8's chat reads [selectedId] (key `allternit-runtime-model-id`,
 * the iOS UserDefaults key) for `runtimeModelId`. Never picks a model for the person: no
 * selection shows "Pick a brain".
 */
internal class ModelStore(context: Context) {
    private val prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    var models by mutableStateOf<List<PickerModel>>(emptyList())
        private set
    var cloudModels by mutableStateOf<List<PickerModel>>(emptyList())
        private set
    var isLoading by mutableStateOf(false)
        private set
    var loadError by mutableStateOf<String?>(null)
        private set
    var selectedId by mutableStateOf(prefs.getString(KEY_SELECTED, null))
        private set
    var effort by mutableStateOf(prefs.getString(KEY_EFFORT, null) ?: "medium")
        private set
    var favorites by mutableStateOf(prefs.getString(KEY_FAVORITES, null)?.split('\n')?.filter { it.isNotEmpty() } ?: emptyList())
        private set

    /** The computer's catalog, or with no computer the Allternit Cloud catalog (iOS chatModels). */
    val chatModels: List<PickerModel> get() = models.ifEmpty { cloudModels }

    val selected: PickerModel? get() = chatModels.firstOrNull { it.id == selectedId }

    /** The chip's text (iOS ComposerModelLabel): the name without a trailing "(…)", else the id's tail. */
    val chipLabel: String
        get() {
            val id = selectedId?.takeIf { it.isNotEmpty() } ?: return "Pick a brain"
            selected?.name?.takeIf { it.isNotEmpty() }?.let { name ->
                val trimmed = name.replace(Regex("""\s*\([^)]*\)\s*$"""), "")
                return trimmed.ifEmpty { name }
            }
            return id.substringAfter('/', id)
        }

    fun select(id: String?) {
        selectedId = id
        prefs.edit().putString(KEY_SELECTED, id).apply()
    }

    fun setEffort(level: String) {
        effort = level
        prefs.edit().putString(KEY_EFFORT, level).apply()
    }

    fun toggleFavorite(id: String) {
        favorites = if (id in favorites) favorites - id else favorites + id
        prefs.edit().putString(KEY_FAVORITES, favorites.joinToString("\n")).apply()
    }

    /** The level shown for [model]: the picked one when it takes it, else its default. */
    fun level(model: PickerModel): String? = model.thinking?.let { t ->
        if (t.levels.any { it.id == effort }) effort else t.default
    }

    /**
     * Loads both catalogs (iOS fetchModelsIfNeeded + fetchCloudCatalogIfNeeded). A computer
     * catalog failure is only shown when there is nothing to pick from.
     */
    suspend fun load(force: Boolean = false) {
        if (isLoading) return
        if (!force && models.isNotEmpty()) return
        isLoading = true
        loadError = null
        try {
            if (force || cloudModels.isEmpty()) loadCloud()
            when (val r = ShellApi.api.listModels()) {
                is ApiResult.Ok -> {
                    models = r.value.map { it.toPicker() }
                    // A persisted id the computer catalog no longer lists is a dead end.
                    val id = selectedId
                    if (id != null && models.isNotEmpty() && models.none { it.id == id }) select(null)
                }
                is ApiResult.HttpError -> if (!isNoComputer(r) && r.status != 503) {
                    loadError = statusSentence(r.status) ?: FriendlyErrorMessage.from(r.message ?: "", "load models")
                } else if (cloudModels.isEmpty()) {
                    loadError = NoComputerCopy.DETAIL
                }
                is ApiResult.Network -> loadError = "Could not connect to the server. Check your connection and try again."
                is ApiResult.Decode -> loadError = FriendlyErrorMessage.FALLBACK
            }
        } finally {
            isLoading = false
        }
    }

    /** `{cloud}/v1/models` (iOS SetupAPI.cloudModelCatalog): listed, standard-lane rows only. */
    private suspend fun loadCloud() {
        val url = ApiConfig().cloudBaseUrl.newBuilder().addPathSegments("v1/models").build()
        val r = ApiCall.execute(ShellApi.okHttp, ApiJson.default, ApiCall.request("GET", url, null), JsonObject.serializer())
        if (r !is ApiResult.Ok) return
        val rows = (r.value["data"] as? JsonArray).orEmpty().mapNotNull { item ->
            val o = item as? JsonObject ?: return@mapNotNull null
            fun str(key: String) = (o[key] as? JsonPrimitive)?.contentOrNull
            fun bool(key: String) = (o[key] as? JsonPrimitive)?.booleanOrNull == true
            val alias = str("id")?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
            if (bool("hidden") || str("lane") == "open") return@mapNotNull null
            val extraName = ((o["extra"] as? JsonObject)?.get("name") as? JsonPrimitive)?.contentOrNull
            val name = (str("name") ?: extraName ?: alias).ifEmpty { alias }
            PickerModel(
                id = CLOUD_PREFIX + alias, name = name, provider = "allternit",
                description = "Allternit Cloud · no computer needed", isDefault = bool("default"),
                hidden = false, rank = null, thinking = null,
            )
        }
        if (rows.isNotEmpty()) cloudModels = rows
    }

    companion object {
        private const val FILE = "allternit_shell"
        const val KEY_SELECTED = "allternit-runtime-model-id"
        private const val KEY_EFFORT = "allternit-model-effort"
        private const val KEY_FAVORITES = "allternit:model-favorites"
        const val CLOUD_PREFIX = "allternit/"
    }
}

/** Provider names and order (iOS ModelProviderNames): Allternit Cloud first, local last. */
internal object ModelProviderNames {
    private val names = mapOf(
        "allternit" to "Allternit Cloud", "anthropic" to "Anthropic", "openai" to "OpenAI",
        "google" to "Google", "gemini" to "Google", "moonshot" to "Moonshot", "kimi" to "Moonshot",
        "deepseek" to "DeepSeek", "xai" to "xAI", "mistral" to "Mistral", "groq" to "Groq",
        "openrouter" to "OpenRouter", "ollama" to "Ollama", "omlx" to "oMLX",
        "local-brain" to "Local Brain", "allternit-sidecar" to "Local Brain",
        "qwen" to "Qwen", "zai" to "ZAI", "meta" to "Meta",
    )
    val local = setOf("ollama", "omlx", "allternit-sidecar", "local-brain")

    fun name(pid: String): String = names[pid] ?: pid.replaceFirstChar { it.uppercase() }

    fun rank(pid: String): Int = if (pid == "allternit") 0 else if (pid in local) 2 else 1

    fun mark(pid: String): String = name(pid).take(1)

    /** Upstream vendor of a Cloud model (iOS inferVendor, web inferVendor). */
    fun inferVendor(m: PickerModel): String {
        val s = "${m.id} ${m.name}".lowercase()
        val table = listOf(
            "anthropic" to listOf("claude", "anthropic", "opus", "sonnet", "haiku"),
            "openai" to listOf("gpt", "openai", "codex"),
            "google" to listOf("gemini", "gemma", "google"),
            "xai" to listOf("grok", "xai"),
            "deepseek" to listOf("deepseek"),
            "mistral" to listOf("mistral", "mixtral", "codestral", "magistral"),
            "kimi" to listOf("kimi", "moonshot"),
            "qwen" to listOf("qwen", "qwq"),
            "zai" to listOf("glm", "zhipu", "z-ai"),
            "meta" to listOf("llama"),
        )
        return table.firstOrNull { (_, keys) -> keys.any { s.contains(it) } }?.first ?: "other"
    }
}
