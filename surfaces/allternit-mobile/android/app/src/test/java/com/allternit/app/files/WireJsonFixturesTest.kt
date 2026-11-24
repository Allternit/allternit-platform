package com.allternit.app.files

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * WireJson decodes and encodes the generated contract types by reflection (no
 * serialization library in the app), so a wrong constructor pick, a list of objects, an
 * enum or a dropped optional would only show up against a real computer. Every valid
 * fixture in contracts/files-drive/fixtures (the same ones the Rust, TS and Swift types are
 * checked against) must decode into its type and encode back to the same JSON.
 */
class WireJsonFixturesTest {
    @Test
    fun everyValidFixtureRoundTrips() {
        val files = contractFile("fixtures").listFiles { f -> f.name.endsWith(".json") }!!.sortedBy { it.name }
        var checked = 0
        for (f in files) {
            val fx = JSONObject(f.readText())
            if (!fx.optBoolean("valid")) continue
            val cls = Class.forName("com.allternit.files.contract." + fx.getString("type"))
            val message = fx.getJSONObject("message")
            val decoded = WireJson.decodeObject(message, cls)
            val encoded = WireJson.encode(decoded!!)
            // Event and feed variants carry their tag in "type"; the data class holds it as a
            // constant (wireTag), not as a field.
            val want = JSONObject(message.toString())
            if (cls.methods.any { it.name == "getWireTag" }) {
                assertEquals("${f.name}: type tag", want.getString("type"), cls.getMethod("getWireTag").invoke(decoded))
                want.remove("type")
            }
            assertTrue("${f.name}: re-encoded JSON differs\n want $want\n got  $encoded", jsonSame(want, encoded))
            assertEquals("${f.name}: decode(encode(x)) == x", decoded, WireJson.decodeObject(encoded, cls))
            checked += 1
        }
        // 86 valid fixtures on 2026-11-15; fewer means the fixture folder was not found whole.
        assertTrue("only $checked fixtures checked", checked >= 80)
    }

    @Test
    fun aMissingRequiredFieldIsAnErrorNotAHalfObject() {
        val ok = JSONObject(contractFile("fixtures/endpoints-cloud.json").readText()).getJSONObject("message")
        ok.remove("computer_id")
        val failed = try {
            WireJson.decodeObject(ok, com.allternit.files.contract.EndpointsResponse::class.java)
            false
        } catch (e: IllegalArgumentException) {
            true
        }
        assertTrue("decoding without computer_id must throw", failed)
    }
}

/**
 * Structural JSON equality (keys in any order, numbers by value). Not JSONObject.similar:
 * under Kotlin 2.3 the unit tests compile against android.jar's org.json stubs, which lack it.
 */
private fun jsonSame(a: Any?, b: Any?): Boolean = when {
    a === JSONObject.NULL || a == null -> b === JSONObject.NULL || b == null
    a is JSONObject && b is JSONObject ->
        a.length() == b.length() && a.keys().asSequence().all { b.has(it) && jsonSame(a.get(it), b.get(it)) }
    a is JSONArray && b is JSONArray ->
        a.length() == b.length() && (0 until a.length()).all { jsonSame(a.get(it), b.get(it)) }
    a is Number && b is Number -> a.toString().toBigDecimal().compareTo(b.toString().toBigDecimal()) == 0
    else -> a == b
}
