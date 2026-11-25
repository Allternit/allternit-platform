package com.allternit.feature.computer

import java.net.URI

/** Link rules for the Apps shelf (iOS AppsShelfLinks): only https on our own domains opens. */
internal object AppsShelfLinks {
    const val APP_DOMAIN = "allternitusercontent.com"
    const val WEB_ORIGIN = "https://ai.allternit.com"

    /** The page an app opens: its hosted https link, or `/app/<slug>` on the web origin; null for anything else. */
    fun openUrl(link: String?): String? {
        if (link.isNullOrBlank()) return null
        val resolved = if (link.startsWith("/")) WEB_ORIGIN + link else link
        val uri = runCatching { URI(resolved) }.getOrNull() ?: return null
        if (uri.scheme?.lowercase() != "https") return null
        val host = uri.host?.lowercase() ?: return null
        val ok = host == APP_DOMAIN || host.endsWith(".$APP_DOMAIN") || host == URI(WEB_ORIGIN).host
        return if (ok) resolved else null
    }
}
