package com.allternit.feature.shell

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PickerCatalogTest {
    private fun parse(s: String) = parsePickerCatalogCloud(Json.parseToJsonElement(s) as JsonObject)

    @Test fun signedOutRowsAreLockedWithSignIn() {
        val rows = parse("""{"cloudModels":[{"id":"allternit/claude-sonnet-5.5","name":"Claude Sonnet 5.5","provider":"anthropic","access":{"state":"locked","reason":"Sign in","action":"sign_in"}}]}""")
        assertEquals(1, rows.size)
        val a = rows[0].access!!
        assertTrue(a.locked); assertEquals("Sign in", a.reason); assertEquals("sign_in", a.action)
        assertEquals("allternit/claude-sonnet-5.5", rows[0].id)
    }

    @Test fun fundedRowsAreOkAndNoCreditsRowsRouteToCredits() {
        val rows = parse("""{"cloudModels":[
            {"id":"allternit/a","name":"A","provider":"openai","access":{"state":"ok"}},
            {"id":"allternit/b","name":"B","provider":"openai","access":{"state":"locked","reason":"No credits left","action":"add_credits"}}]}""")
        assertFalse(rows[0].access!!.locked); assertNull(rows[0].access!!.action)
        assertEquals("add_credits", rows[1].access!!.action)
    }

    @Test fun hiddenRowsAreNotShown() {
        val rows = parse("""{"cloudModels":[{"id":"allternit/h","name":"H","provider":"x","access":{"state":"hidden"}},{"id":"allternit/v","name":"V","provider":"x","access":{"state":"ok"}}]}""")
        assertEquals(listOf("allternit/v"), rows.map { it.id })
    }
}
