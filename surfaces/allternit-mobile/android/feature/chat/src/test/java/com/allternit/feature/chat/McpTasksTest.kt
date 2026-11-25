package com.allternit.feature.chat

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The server-question form: field order, kinds, and the never-collect-secrets rule (mirrors iOS McpElicitationPart). */
class McpTasksTest {
    private fun part(json: String) = McpElicitationPart.from(Json.parseToJsonElement(json).jsonObject)!!

    @Test fun requiredFieldsComeFirstAndKindsFollowTheSchema() {
        val p = part("""{"id":"t1:q","mode":"form","message":"Pick","requestedSchema":{"required":["when"],"properties":{
            "zeta":{"type":"boolean","title":"Notify"},
            "when":{"type":"string","enum":["now","later"],"enumNames":["Right now","Later"]},
            "count":{"type":"integer"}}}}""")
        assertEquals(listOf("when", "count", "zeta"), p.fields.map { it.key })
        assertEquals(McpFieldKind.Choice, p.fields[0].kind)
        assertEquals("Right now", p.fields[0].options[0].label)
        assertEquals(McpFieldKind.Integer, p.fields[1].kind)
        assertEquals(McpFieldKind.Boolean, p.fields[2].kind)
    }

    @Test fun aSecretLikeFieldTurnsTheFormIntoAPointer() {
        val p = part("""{"id":"t1:k","mode":"form","message":"Key","requestedSchema":{"properties":{"api_key":{"type":"string"}}}}""")
        assertTrue(p.hasSecretField)
        assertFalse(part("""{"id":"t1:n","requestedSchema":{"properties":{"name":{"type":"string"}}}}""").hasSecretField)
    }

    @Test fun onlyHttpUrlsOpenAndTaskErrorsReadBothShapes() {
        assertEquals(null, part("""{"id":"u","mode":"url","url":"javascript:alert(1)"}""").url)
        val t = McpTaskPart.from(Json.parseToJsonElement("""{"taskId":"c:1","status":"failed","error":{"message":"boom"},"progress":{"done":1,"total":4}}""").jsonObject)
        assertNotNull(t)
        assertEquals("boom", t!!.errorText)
        assertEquals(0.25f, t.fraction)
    }
}
