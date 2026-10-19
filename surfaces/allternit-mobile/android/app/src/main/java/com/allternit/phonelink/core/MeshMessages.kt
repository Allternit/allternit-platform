package com.allternit.phonelink.core

import java.net.URLDecoder
import java.net.URLEncoder

/** Everything the :mesh process needs to start the node, sent in the bind Intent. */
data class MeshStartParams(
    val controlUrl: String,
    /** The main process's internal loopback port (server port + 10000); the mesh bridge targets it. */
    val internalPort: Int,
    val hostname: String,
    /** The tsnet state dir (app-private; holds the node key after the first join). */
    val meshStateDir: String,
    /** The opened pre-auth key, only while one is stored; "" once tsnet can re-authenticate itself. */
    val authKey: String,
)

/** What the mesh process reports back to the main process on every state change. */
data class MeshReport(val state: String, val meshIp: String?)

/**
 * String codec for the two payloads that cross between the main process and
 * the :mesh process. They travel as Intent extras and Messenger payloads, so
 * they must be plain strings; URL-encoded `key=value` lines keep newlines and
 * specials out of the auth key without inventing a format. Decoding a
 * malformed payload yields null, never an exception, and nothing here is
 * ever logged (the auth key must not reach logcat).
 */
object MeshMessages {
    /** Bind-Intent extra holding [encodeStart]'d [MeshStartParams]. */
    const val EXTRA_PARAMS = "com.allternit.phonelink.MESH_PARAMS"
    /** Bind-Intent extra: the main process's reply [android.os.Messenger]. */
    const val EXTRA_REPLY = "com.allternit.phonelink.MESH_REPLY"
    /** Mesh → main: [android.os.Message.obj] is [encodeReport]'d [MeshReport]. */
    const val MSG_REPORT = 1
    /** Main → mesh: stop and rejoin the node (same state dir/keys). Sent by `mesh_restart`. */
    const val MSG_RESTART = 2
    /** Bundle key for the encoded report. Message.obj can't cross processes unless it's Parcelable. */
    const val KEY_REPORT = "report"

    fun encodeStart(p: MeshStartParams): String = listOf(
        "controlUrl" to p.controlUrl,
        "internalPort" to p.internalPort.toString(),
        "hostname" to p.hostname,
        "meshStateDir" to p.meshStateDir,
        "authKey" to p.authKey,
    ).joinToString("\n") { (k, v) -> "$k=${enc(v)}" }

    fun decodeStart(s: String): MeshStartParams? = try {
        val m = parse(s)
        val controlUrl = m["controlUrl"]?.takeIf { it.isNotEmpty() } ?: return null
        val internalPort = m["internalPort"]?.toIntOrNull() ?: return null
        val hostname = m["hostname"]?.takeIf { it.isNotEmpty() } ?: return null
        val meshStateDir = m["meshStateDir"]?.takeIf { it.isNotEmpty() } ?: return null
        MeshStartParams(controlUrl, internalPort, hostname, meshStateDir, m["authKey"].orEmpty())
    } catch (e: Exception) {
        null
    }

    fun encodeReport(r: MeshReport): String = buildString {
        append("state=").append(enc(r.state))
        r.meshIp?.let { append("\nmeshIp=").append(enc(it)) }
    }

    fun decodeReport(s: String): MeshReport? = try {
        val m = parse(s)
        val state = m["state"]?.takeIf { it.isNotEmpty() } ?: return null
        MeshReport(state, m["meshIp"]?.takeIf { it.isNotEmpty() })
    } catch (e: Exception) {
        null
    }

    private fun parse(s: String): Map<String, String> =
        s.split("\n").mapNotNull { line ->
            val i = line.indexOf('=')
            if (i <= 0) null else line.substring(0, i) to dec(line.substring(i + 1))
        }.toMap()

    private fun enc(v: String): String = URLEncoder.encode(v, Charsets.UTF_8)
    private fun dec(v: String): String = URLDecoder.decode(v, Charsets.UTF_8)
}
