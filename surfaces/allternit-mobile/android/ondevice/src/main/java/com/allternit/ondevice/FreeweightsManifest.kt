package com.allternit.ondevice

import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/**
 * The trust root for a download: the Freeweights manifest of the repo that holds the file
 * (spec manifest-v0: files[] {path,size,sha256}, sources[] {type,url,priority}). The engine
 * fetches it itself; the page only names the model, quant, repo and file.
 *
 * Not done here: the manifest's minisign signature. It is fetched over HTTPS from the
 * Freeweights origin; checking the Ed25519 signature on minSdk 26 needs a vendored verifier.
 */
object FreeweightsManifest {
    const val ORIGIN = "https://freeweights.allternit.com"

    data class FileRef(val repo: String, val path: String, val size: Long, val sha256: String, val url: String)

    fun manifestUrl(repo: String): String = "$ORIGIN/manifests/${repo.split('/').joinToString("/") { enc(it) }}.json"

    fun fetch(repo: String, path: String): FileRef {
        require(REPO.matches(repo)) { "bad repo id" }
        val conn = URL(manifestUrl(repo)).openConnection() as HttpURLConnection
        conn.connectTimeout = 15_000
        conn.readTimeout = 30_000
        try {
            val code = conn.responseCode
            if (code == 404) throw Protocol.Failure("no_manifest", "Freeweights has no manifest for $repo.")
            if (code != 200) throw Protocol.Failure("manifest_http", "Freeweights answered $code for $repo.")
            val body = conn.inputStream.bufferedReader().use { it.readText() }
            return pick(JSONObject(body), repo, path)
        } finally {
            conn.disconnect()
        }
    }

    /** Finds [path] in the manifest and the first source that can serve it. Pure, JVM-tested. */
    fun pick(manifest: JSONObject, repo: String, path: String): FileRef {
        val files = manifest.optJSONArray("files") ?: throw Protocol.Failure("bad_manifest", "The manifest lists no files.")
        var entry: JSONObject? = null
        for (i in 0 until files.length()) {
            val f = files.getJSONObject(i)
            if (f.optString("path") == path) { entry = f; break }
        }
        entry ?: throw Protocol.Failure("no_file", "$path is not in the manifest of $repo.")
        val sha = entry.optString("sha256").lowercase()
        if (!SHA.matches(sha)) throw Protocol.Failure("bad_manifest", "The manifest has no sha256 for $path.")
        val size = entry.optLong("size", -1)
        if (size <= 0) throw Protocol.Failure("bad_manifest", "The manifest has no size for $path.")
        val sources = manifest.optJSONArray("sources")
        val urls = mutableListOf<Pair<Int, String>>()
        if (sources != null) {
            for (i in 0 until sources.length()) {
                val s = sources.getJSONObject(i)
                val base = s.optString("url")
                if (!base.startsWith("https://")) continue // magnets and http are skipped
                // hosted (Freeweights / mirrors) before upstream at equal priority
                val rank = s.optInt("priority", 1000) * 2 + if (s.optString("type") == "hosted") 0 else 1
                urls.add(rank to base)
            }
        }
        val base = urls.minByOrNull { it.first }?.second
            ?: throw Protocol.Failure("no_source", "No download source for $path.")
        val url = (if (base.endsWith("/")) base else "$base/") + path.split('/').joinToString("/") { enc(it) }
        return FileRef(repo, path, size, sha, url)
    }

    private fun enc(s: String) = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    private val REPO = Regex("^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$")
    private val SHA = Regex("^[0-9a-f]{64}$")
}
