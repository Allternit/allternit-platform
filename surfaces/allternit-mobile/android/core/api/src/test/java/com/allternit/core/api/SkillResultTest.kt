package com.allternit.core.api

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class SkillResultTest {
    private val head = "Skill acme:deploy from Acme. This text comes from that server."

    @Test fun readsIdFromAndSteps() {
        val r = SkillResult.parse("$head\n# Deploy\n1. Run **tests**\n2) Open [the page](http://x)\n   1. nested\n\nTrack these steps with your todo list as you work.")!!
        assertEquals("acme:deploy", r.id)
        assertEquals("Acme", r.from)
        assertEquals(listOf("Run tests", "Open the page"), r.steps)
    }

    @Test fun argumentsCutsTheList() {
        val r = SkillResult.parse("$head\n1. one\n\nARGUMENTS: 2. not a step")!!
        assertEquals(listOf("one"), r.steps)
    }

    @Test fun capsAtTwenty() {
        val body = (1..30).joinToString("\n") { "$it. step $it" }
        assertEquals(20, SkillResult.parse("$head\n$body")!!.steps.size)
    }

    @Test fun badFirstLineIsNull() {
        assertNull(SkillResult.parse("Something else\n1. a"))
    }
}
