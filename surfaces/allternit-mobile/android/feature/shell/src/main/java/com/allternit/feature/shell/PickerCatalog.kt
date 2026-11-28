package com.allternit.feature.shell

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull

/**
 * A Cloud row's lock state as the server computed it (`GET {cloud}/api/v1/picker-catalog`,
 * `cloudModels[].access`). [action] is one of sign_in, add_credits, accept_terms, upgrade, retry.
 */
internal data class ModelAccess(val locked: Boolean, val reason: String?, val action: String?)

/**
 * Parses the picker catalog's `cloudModels` (docs/contracts/picker-catalog): `hidden` rows are dropped,
 * `locked` rows keep their reason and action, and `ok` rows are plain. Ids already carry `allternit/`.
 */
internal fun parsePickerCatalogCloud(root: JsonObject): List<PickerModel> =
    (root["cloudModels"] as? JsonArray).orEmpty().mapNotNull { item ->
        val o = item as? JsonObject ?: return@mapNotNull null
        fun str(obj: JsonObject, key: String) = (obj[key] as? JsonPrimitive)?.contentOrNull
        val id = str(o, "id")?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
        val acc = o["access"] as? JsonObject
        val state = acc?.let { str(it, "state") } ?: "ok"
        if (state == "hidden") return@mapNotNull null
        val locked = state == "locked"
        PickerModel(
            id = if (id.startsWith(ModelStore.CLOUD_PREFIX)) id else ModelStore.CLOUD_PREFIX + id,
            name = (str(o, "name") ?: id).ifEmpty { id },
            provider = "allternit",
            description = "Allternit Cloud · no computer needed",
            isDefault = false, hidden = false, rank = null, thinking = null,
            cloudProvider = str(o, "provider"),
            access = ModelAccess(
                locked = locked,
                reason = if (locked) acc?.let { str(it, "reason") } else null,
                action = if (locked) acc?.let { str(it, "action") } else null,
            ),
        )
    }
