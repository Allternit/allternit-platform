package com.allternit.feature.code

import java.io.File
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.boolean
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The Kotlin agent-frame parser against the real captured screens shared with the web and iOS
 * (surfaces/allternit-mobile/agent-frame-fixtures, a copy of allternit-ai
 * src/lib/terminal/__fixtures__/agent-frame; build.gradle.kts adds the folder as test resources).
 * Same steps as agent-frame.test.ts: split on "\n", drop the one trailing empty piece, pad to the
 * 40-row viewport, compare with expected.json.
 */
class AgentFrameTest {
    private val viewportRows = 40

    /** The shared folder: from the test resources, else found above the module (an IDE run). */
    private fun fixturesDir(): File {
        javaClass.classLoader?.getResource("expected.json")?.takeIf { it.protocol == "file" }?.let { url ->
            return File(url.toURI()).parentFile
        }
        var dir: File? = File(System.getProperty("user.dir")).absoluteFile
        while (dir != null) {
            val f = File(dir, "surfaces/allternit-mobile/agent-frame-fixtures")
            if (File(f, "expected.json").exists()) return f
            val sibling = File(dir, "agent-frame-fixtures")
            if (File(sibling, "expected.json").exists()) return sibling
            dir = dir.parentFile
        }
        error("agent-frame-fixtures not found above ${System.getProperty("user.dir")}")
    }

    /** Rows the way the terminal caller builds them: one string per viewport row. */
    private fun rowsOf(text: String): List<String> {
        val rows = text.removeSuffix("\n").split("\n").toMutableList()
        while (rows.size < viewportRows) rows.add("")
        return rows
    }

    private fun expected(): Map<String, AgentFrame?> {
        val root = Json.parseToJsonElement(File(fixturesDir(), "expected.json").readText()) as JsonObject
        return root.mapValues { (_, v) -> frameOf(v) }
    }

    private fun str(e: JsonElement?): String? = if (e == null || e is JsonNull) null else e.jsonPrimitive.content

    private fun frameOf(e: JsonElement): AgentFrame? {
        if (e is JsonNull) return null
        val o = e as JsonObject
        val dialog = (o["dialog"] as? JsonObject)?.let { d ->
            AgentDialog(
                question = str(d["question"]).orEmpty(),
                options = (d["options"] as JsonArray).map { opt ->
                    val p = opt as JsonObject
                    AgentDialogOption(
                        key = str(p["key"]).orEmpty(),
                        label = str(p["label"]).orEmpty(),
                        selected = p["selected"]!!.jsonPrimitive.boolean,
                        index = p["index"]!!.jsonPrimitive.int,
                    )
                },
            )
        }
        return AgentFrame(
            agent = str(o["agent"]),
            hideFrom = o["hideFrom"]!!.jsonPrimitive.int,
            status = AgentStatus.from(str(o["status"])!!)!!,
            dialog = dialog,
            mode = str(o["mode"]),
        )
    }

    @Test
    fun everyFixtureHasAnExpectationAndBackAgain() {
        val files = fixturesDir().listFiles { f -> f.name.endsWith(".txt") }!!.map { it.name }.sorted()
        assertEquals(files, expected().keys.sorted())
        assertEquals(33, files.size)
    }

    @Test
    fun capturedScreens() {
        val dir = fixturesDir()
        for ((file, want) in expected().toSortedMap()) {
            val text = File(dir, file).readText(Charsets.UTF_8)
            assertEquals(file, want, AgentFrameParser.parse(rowsOf(text)))
        }
    }

    @Test
    fun plainShellScreenIsNotAnAgent() {
        val screen = listOf("joe@mac repo % ls", "README.md  package.json  src", "joe@mac repo % ").joinToString("\n")
        assertNull(AgentFrameParser.parse(rowsOf(screen)))
    }

    /** The bytes the composer sends (agent-frame.ts agentPromptInput / agentDialogKeys). */
    @Test
    fun keysForPromptAndAnswers() {
        assertEquals("\u001b[200~fix it\nplease\u001b[201~", AgentKeys.promptPaste("fix it\nplease"))
        assertEquals("\r", AgentKeys.ENTER)

        val numbered = listOf(AgentDialogOption("1", "Yes", true, 0), AgentDialogOption("2", "No", false, 1))
        assertEquals("2", AgentKeys.dialogKeys(numbered, numbered[1]))

        val arrows = listOf(AgentDialogOption("", "A", false, 0), AgentDialogOption("", "B", true, 1), AgentDialogOption("", "C", false, 2))
        assertEquals("\u001b[A\r", AgentKeys.dialogKeys(arrows, arrows[0]))
        assertEquals("\u001b[B\r", AgentKeys.dialogKeys(arrows, arrows[2]))
        assertEquals("\r", AgentKeys.dialogKeys(arrows, arrows[1]))
    }

    /**
     * Where java.util.regex and JS disagree, the translated pattern follows JS (JsRegex docs): a
     * no-break space is `\s`, `$` is end of input only, `\b` is ASCII, and `i` folds ASCII only.
     */
    @Test
    fun regexFollowsJavaScriptWhereJavaDiffers() {
        assertTrue(JsRegex("^\\s*›\\s").test("\u00a0› Ask Codex"))
        assertTrue(!JsRegex("^\\s+$").test("\u0085"))
        assertTrue(!JsRegex("ok$").test("ok\n"))
        // ASCII \b: "é" is not a word character in JS, so "écodexé" has boundaries round "codex".
        assertTrue(JsRegex("\\bcodex\\b", ignoreCase = true).test("\u00e9Codex\u00e9"))
        assertTrue(!JsRegex("\\bcodex\\b", ignoreCase = true).test("acodexa"))
        // The Kelvin sign never folds to k in JS without the u flag.
        assertTrue(!JsRegex("kimi", ignoreCase = true).test("\u212Aimi"))
        assertTrue(JsRegex("([a-z][a-z -]*?) on", ignoreCase = true).test("Bypass permissions on"))
        // JS trim strips U+00A0 and U+FEFF but not U+001C (Kotlin's trim() does the opposite).
        assertEquals("x", jsTrim("\u00a0  x\ufeff"))
        assertEquals("\u001cx", jsTrim("\u001cx"))
    }
}
