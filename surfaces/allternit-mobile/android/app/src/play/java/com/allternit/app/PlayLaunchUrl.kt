package com.allternit.app

import org.json.JSONException
import org.json.JSONObject
import java.net.URI
import java.net.URISyntaxException
import java.net.URLEncoder

/**
 * The `src=play` launch marker (store purchases D15). The web app reads the exact query
 * parameter `src=play` once, keeps it in sessionStorage, and then sells through Google
 * Play Billing only (Digital Goods API + PaymentRequest) and hides Stripe checkout,
 * the portal link and auto top-up. Every URL the play build opens in the TWA must carry
 * it: the default URL, deep links, launcher shortcuts and shares.
 *
 * Pure string code (no android.net.Uri) so the JVM unit tests run it as is.
 */
object PlayLaunchUrl {
    const val PARAM = "src"
    const val VALUE = "play"

    /**
     * [url] with `src=play` in its query. Keeps every other parameter and the fragment.
     * An existing `src` with another value is replaced, since the web app only accepts
     * the exact marker. Non-http(s) URLs come back unchanged.
     */
    fun withPlayMarker(url: String): String {
        val scheme = url.substringBefore(':', "").lowercase()
        if (scheme != "https" && scheme != "http") return url
        val hashAt = url.indexOf('#')
        val fragment = if (hashAt >= 0) url.substring(hashAt) else ""
        val beforeFragment = if (hashAt >= 0) url.substring(0, hashAt) else url
        val queryAt = beforeFragment.indexOf('?')
        val base = if (queryAt >= 0) beforeFragment.substring(0, queryAt) else beforeFragment
        val query = if (queryAt >= 0) beforeFragment.substring(queryAt + 1) else ""
        val kept = query.split('&').filter { it.isNotEmpty() && it.substringBefore('=') != PARAM }
        val marker = "$PARAM=$VALUE"
        return base + "?" + (kept + marker).joinToString("&") + fragment
    }

    /**
     * The URL a text share should open, built the way Chrome builds a GET share target
     * (title/text names from the manifest's share target JSON, `+` for spaces), plus the
     * marker. Chrome's own share path replaces the action URL's whole query with the
     * shared fields (TwaSharingController.createGETWebShareTargetUriString), so a marker
     * in the action would be dropped; the launcher opens this URL as a plain deep link
     * instead. Null when the share target is missing, not GET, or carries files, or when
     * nothing was shared: the caller then leaves the share to Chrome unchanged.
     */
    fun shareUrl(defaultUrl: String?, shareTargetJson: String?, title: String?, text: String?): String? {
        if (defaultUrl.isNullOrEmpty() || shareTargetJson.isNullOrEmpty()) return null
        if (title.isNullOrEmpty() && text.isNullOrEmpty()) return null
        return try {
            val target = JSONObject(shareTargetJson)
            val method = target.optString("method", "GET")
            if (!method.equals("GET", ignoreCase = true)) return null
            val params = target.optJSONObject("params") ?: return null
            if (params.has("files")) return null
            val action = target.optString("action", "")
            if (action.isEmpty()) return null
            val actionUrl = URI(defaultUrl).resolve(action).toString()
            val fields = buildList {
                val titleName = params.optString("title", "")
                val textName = params.optString("text", "")
                if (titleName.isNotEmpty() && !title.isNullOrEmpty()) add(titleName to title)
                if (textName.isNotEmpty() && !text.isNullOrEmpty()) add(textName to text)
            }
            if (fields.isEmpty()) return null
            val query = fields.joinToString("&") { (name, value) -> "${encode(name)}=${encode(value)}" }
            // Same as Chrome: the shared fields replace the action's own query.
            val withoutQuery = actionUrl.substringBefore('#').substringBefore('?')
            withPlayMarker("$withoutQuery?$query")
        } catch (e: JSONException) {
            null
        } catch (e: URISyntaxException) {
            null
        } catch (e: IllegalArgumentException) {
            null
        }
    }

    // URLEncoder is form encoding: spaces become "+", as the share target spec requires.
    private fun encode(s: String): String = URLEncoder.encode(s, "UTF-8")
}
