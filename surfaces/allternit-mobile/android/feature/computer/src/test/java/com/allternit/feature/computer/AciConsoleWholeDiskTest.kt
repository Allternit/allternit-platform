package com.allternit.feature.computer

import com.allternit.feature.chat.RuntimeCall
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Whole-disk Files (platform #2829): the optional `wholeDisk` field, the PATCH body, and the 403 message the user sees. */
class AciConsoleWholeDiskTest {
    private fun row(extra: String) = ConsoleComputer.parse(Json.parseToJsonElement("""{"id":"c1","name":"Studio","status":"online"$extra}""") as JsonObject)!!

    @Test fun aMissingWholeDiskIsUnavailableWithNoSwitcher() {
        val c = row("")
        assertNull(c.wholeDisk)
    }

    @Test fun enabledListsTheServerRootsInOrder() {
        val d = row(""","wholeDisk":{"enabled":true,"available":true,"roots":[{"id":"disk:C:","path":"C:\\\\","label":"Drive C:"},{"id":"disk:D:","path":"D:\\\\","label":"Drive D:"}]}""").wholeDisk!!
        assertTrue(d.showsRootsSwitcher)
        assertEquals(listOf("Drive C:", "Drive D:"), d.roots.map { it.label })
    }

    @Test fun offHidesTheSwitcherAndUnavailableSaysWhy() {
        assertFalse(row(""","wholeDisk":{"enabled":false,"available":true,"roots":[]}""").wholeDisk!!.showsRootsSwitcher)
        assertEquals("Only the owner of this computer can turn this on.",
            row(""","wholeDisk":{"enabled":false,"available":false,"reason":"not_owner","roots":[]}""").wholeDisk!!.reasonCopy)
        assertEquals("Not available on this computer.", row(""","wholeDisk":{"enabled":false,"available":false,"roots":[]}""").wholeDisk!!.reasonCopy)
    }

    @Test fun patchBodyUsesWholeDiskAndConfirmOnlyWhenTurningOn() {
        assertEquals("""{"wholeDisk":true,"confirm":true}""", wholeDiskBody(true).toString())
        assertEquals("""{"wholeDisk":false}""", wholeDiskBody(false).toString())
    }

    @Test fun wholeDiskOffKeepsTheServerMessage() {
        val msg = "This folder is outside the Files drive. Turn on whole-disk browsing for this computer in its Files settings first."
        val f = wholeDiskOffFailure(RuntimeCall.Http(403, "whole_disk_off", msg))!!
        assertEquals(msg, f.message)
        assertTrue(f.wholeDiskOff)
        assertNull(wholeDiskOffFailure(RuntimeCall.Http(403, "forbidden", "No.")))
        assertNotNull(wholeDiskAnswer(null, true))
    }
}
