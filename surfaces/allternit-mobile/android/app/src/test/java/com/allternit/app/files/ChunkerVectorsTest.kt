package com.allternit.app.files

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.File

/**
 * The Android chunker must cut exactly where the Rust engine (fastcdc 3.2.1 v2020) cuts,
 * or dedup, resume and replica checks miss across devices (SPEC section 5). Runs every
 * case in contracts/files-drive/vectors/chunker-v1.json, the file the engine, web and
 * Swift chunkers are checked against too.
 */
class ChunkerVectorsTest {
    private val vectors: JSONObject by lazy { JSONObject(contractFile("vectors/chunker-v1.json").readText()) }

    @Test
    fun gearTableMatchesTheVectorFile() {
        val gear = vectors.getJSONArray("gear")
        assertEquals(256, gear.length())
        for (k in 0 until 256) {
            val want = java.lang.Long.parseUnsignedLong(gear.getString(k).removePrefix("0x"), 16)
            assertEquals("GEAR[$k]", want, Gear.GEAR[k])
        }
    }

    @Test
    fun paramsMatchTheVectorFile() {
        val p = vectors.getJSONObject("params")
        assertEquals(p.getLong("min_bytes"), Chunker.MIN.toLong())
        assertEquals(p.getLong("avg_bytes"), Chunker.AVG.toLong())
        assertEquals(p.getLong("max_bytes"), Chunker.MAX.toLong())
        assertEquals(vectors.getString("chunker"), Chunker.PARAMS.algo)
    }

    @Test
    fun everyCaseCutsAndHashesLikeTheEngine() {
        val cases = vectors.getJSONArray("cases")
        for (i in 0 until cases.length()) {
            val c = cases.getJSONObject(i)
            val name = c.getString("name")
            val res = Chunker.chunk(ByteArrayInputStream(input(c.getJSONObject("input"))))
            val want = c.getJSONArray("chunks")
            assertEquals("$name: chunk count", want.length(), res.chunks.size)
            for (j in 0 until want.length()) {
                val w = want.getJSONObject(j)
                val got = res.chunks[j]
                assertEquals("$name: chunk $j offset", w.getLong("offset"), got.offset)
                assertEquals("$name: chunk $j size", w.getLong("size"), got.size)
                assertEquals("$name: chunk $j hash", w.getString("hash"), got.hash)
            }
            assertEquals("$name: size", c.getLong("size"), res.size)
            assertEquals("$name: content hash", c.getString("content_hash"), res.contentHash)
        }
    }

    /** The vector file's input kinds (its "algorithm" list, last entry). */
    private fun input(spec: JSONObject): ByteArray {
        val len = spec.getInt("len")
        return when (spec.getString("kind")) {
            "zeros" -> ByteArray(len)
            "splitmix64" -> {
                val out = ByteArray(len)
                var x = spec.getLong("seed")
                var at = 0
                while (at < len) {
                    x += -0x61c8864680b583ebL // 0x9E3779B97F4A7C15
                    var z = x
                    z = (z xor (z ushr 30)) * -0x40a7b892e31b1a47L // 0xBF58476D1CE4E5B9
                    z = (z xor (z ushr 27)) * -0x6b2fb644ecceee15L // 0x94D049BB133111EB
                    z = z xor (z ushr 31)
                    for (b in 0 until 8) {
                        if (at >= len) break
                        out[at++] = (z ushr (8 * b)).toByte()
                    }
                }
                out
            }
            else -> error("unknown input kind ${spec.getString("kind")}")
        }
    }
}

/** A file under contracts/files-drive, found by walking up from the test's working directory. */
internal fun contractFile(rel: String): File {
    var dir: File? = File(System.getProperty("user.dir")).absoluteFile
    while (dir != null) {
        val f = File(dir, "contracts/files-drive/$rel")
        if (f.exists()) return f
        dir = dir.parentFile
    }
    error("contracts/files-drive/$rel not found above ${System.getProperty("user.dir")}")
}
