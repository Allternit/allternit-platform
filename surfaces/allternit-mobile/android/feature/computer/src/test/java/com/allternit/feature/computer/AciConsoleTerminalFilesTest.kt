package com.allternit.feature.computer

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class AciConsoleTerminalFilesTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject

    @Test fun relayIdPrefersRuntimeIdThenRowId() {
        val row = ConsoleComputer.parse(obj("""{"id":"c1","name":"Box","status":"online","runtimeId":"rt9"}"""))!!
        assertEquals("rt9", row.relayId)
        val bare = ConsoleComputer.parse(obj("""{"id":"c2","name":"Desk","status":"online"}"""))!!
        assertEquals("c2", bare.relayId)
        assertEquals("c3", ConsoleComputer.parse(obj("""{"id":"c3","name":"X","status":"online","runtimeId":""}"""))!!.relayId)
    }

    @Test fun aliasesAndPairedIdDecodeAndMatch() {
        val row = ConsoleComputer.parse(obj("""{"id":"p1","name":"Mac","status":"online","aliases":["p1","d7"],"pairedId":"p1"}"""))!!
        assertEquals(listOf("p1", "d7"), row.aliases)
        assertEquals("p1", row.pairedId)
        assertEquals(true, row.matches("d7"))
        assertEquals(false, row.matches("zz"))
        val old = ConsoleComputer.parse(obj("""{"id":"c2","name":"Desk","status":"online"}"""))!!
        assertEquals(emptyList<String>(), old.aliases)
        assertNull(old.pairedId)
        assertEquals(true, old.matches("c2"))
    }

    @Test fun snippetsAcceptWrappedOrBareShape() {
        val wrapped = Json.parseToJsonElement("""{"snippets":[{"id":"s1","name":"Disk","command":"df -h","description":"Space"},{"id":"bad"}]}""")
        assertEquals(listOf("s1"), wrapped.rows("snippets").mapNotNull(ConsoleSnippet::parse).map { it.id })
        val bare = Json.parseToJsonElement("""[{"id":"s2","name":"Up","command":"uptime"}]""")
        val one = bare.rows("snippets").mapNotNull(ConsoleSnippet::parse).single()
        assertEquals("uptime", one.command)
        assertNull(one.description)
    }

    @Test fun diskMountsReadPercentPerMount() {
        val mounts = DiskMount.parseAll(Json.parseToJsonElement("""{"disk":[{"mount":"/","usedBytes":50,"totalBytes":200},{"mount":"/data","usedBytes":0,"totalBytes":0},{"mount":"/x"}]}"""))
        assertEquals(2, mounts.size)
        assertEquals(25.0, mounts[0].percent, 0.0)
        assertEquals(0.0, mounts[1].percent, 0.0)
    }

    @Test fun cleanupsAreSpaceCleanFindingsOnly() {
        val clean = HostFinding("a", "info", "Apt cache", "d", HostAction("space.clean", "apt-cache", null))
        val other = HostFinding("b", "warning", "Update", "d", HostAction("update.apply", "all", null))
        val none = HostFinding("c", "info", "Note", "d", null)
        assertEquals(listOf("a"), consoleCleanups(listOf(clean, other, none)).map { it.id })
    }
}
