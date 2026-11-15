package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.Base64

class PetNoticeTest {
    private fun gate(kv: MemoryKeyValueStore = MemoryKeyValueStore()) =
        PetNoticeGate(PetOfferSettings(kv), ownPackage = "com.allternit.app")

    private fun node(text: String?, password: Boolean = false, editable: Boolean = false, window: String? = null) =
        UiNode(1, "android.widget.TextView", text, null, null, "com.x", Bounds(0, 0, 10, 10),
            editable = editable, password = password, windowType = window)

    // ── Matcher ─────────────────────────────────────────────────────────────

    @Test fun everyContractFixtureGivesItsExpectedKindAndRule() {
        val failures = PetOfferContract.FIXTURES.mapNotNull { f ->
            val got = PetNotice.match(f.text, f.dialog)
            if (got?.kind == f.expected && got?.ruleId == f.rule) null
            else "${f.name}: expected ${f.expected}/${f.rule}, got ${got?.kind}/${got?.ruleId}"
        }
        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }

    @Test fun lineIsTheWholeLineHoldingTheMatchTrimmed() {
        val t = PetNotice.match("ok\n  Chrome isn't responding  \nClose app", dialog = true)!!
        assertEquals("Chrome isn't responding", t.line)
        assertEquals(PetOfferKind.ERROR_DIALOG, t.kind)
    }

    @Test fun carriageReturnsCountAsLineBreaks() {
        val t = PetNotice.match("starting\r\nnpm ERR! code 1\r\nmore", dialog = false)!!
        assertEquals("npm.err", t.ruleId)
        assertEquals("npm ERR! code 1", t.line)
    }

    @Test fun lineIsCutToTheContractLimit() {
        val t = PetNotice.match("fatal: " + "x".repeat(1000), dialog = false)!!
        assertEquals(PetOfferContract.LINE_MAX_CHARS, t.line.length)
    }

    @Test fun onlyTheTailIsMatched() {
        val text = "npm ERR! early\n" + "ok\n".repeat(5000)
        assertNull(PetNotice.match(text, dialog = false, maxTextChars = 8000))
        assertEquals("npm.err", PetNotice.match(text, dialog = false, maxTextChars = 100_000)?.ruleId)
    }

    @Test fun dialogOnlyRuleNeedsADialog() {
        val text = "The document could not be opened."
        assertNull(PetNotice.match(text, dialog = false))
        assertEquals("dialog.generic", PetNotice.match(text, dialog = true)?.ruleId)
    }

    @Test fun curlyApostropheMatchesLikeAStraightOne() {
        assertEquals("android.keeps_stopping", PetNotice.match("Maps isn’t responding", dialog = true)?.ruleId)
    }

    // ── Reading ─────────────────────────────────────────────────────────────

    @Test fun passwordAndEditableFieldsAreNeverRead() {
        val text = PetNotice.textOf(listOf(
            node("com.x", window = "application"),
            node("Sign-in failed"),
            node("hunter2", password = true),
            node("draft I was typing", editable = true),
            node("Try again"),
        ))
        assertEquals("Sign-in failed\nTry again", text)
    }

    @Test fun dialogClassesComeFromTheContract() {
        assertTrue(PetNotice.isDialogClass("com.android.server.am.AppErrorDialog"))
        assertTrue(PetNotice.isDialogClass("androidx.appcompat.app.AlertDialog"))
        assertFalse(PetNotice.isDialogClass("com.example.MainActivity"))
        assertFalse(PetNotice.isDialogClass(null))
    }

    @Test fun systemDialogNamesTheAppItIsAbout() {
        assertEquals("Allternit", PetNotice.systemDialogApp("Allternit keeps stopping"))
        assertEquals("Google Maps", PetNotice.systemDialogApp("Google Maps isn’t responding"))
        assertNull(PetNotice.systemDialogApp("Could not save the file"))
    }

    @Test fun offerCopyFillsTheAppOrTheFallback() {
        assertEquals("Chrome is showing an error. Fix it?", PetNotice.copy(PetOfferKind.ERROR_DIALOG, "Chrome"))
        assertEquals("This app is showing an error. Fix it?", PetNotice.copy(PetOfferKind.ERROR_DIALOG, null))
    }

    @Test fun fixItUrlCarriesTheOfferInTheFragmentOnly() {
        val trigger = PetNotice.Trigger(PetOfferKind.ERROR_DIALOG, "android.keeps_stopping", "Café keeps stopping")
        val url = PetNotice.fixItUrl("Café", "com.cafe", trigger, "Café keeps stopping\nClose app")
        assertTrue(url.startsWith("https://m.allternit.com/#pet-offer="))
        assertFalse("no query string", url.contains('?'))
        val payload = url.substringAfter("#pet-offer=")
        assertFalse(payload.contains('+') || payload.contains('/') || payload.contains('='))
        val o = JSONObject(String(Base64.getUrlDecoder().decode(payload), Charsets.UTF_8))
        assertEquals("Café", o.getString("app"))
        assertEquals("com.cafe", o.getString("appId"))
        assertEquals("error_dialog", o.getString("kind"))
        assertEquals("Café keeps stopping", o.getString("line"))
        assertEquals("Café keeps stopping\nClose app", o.getString("text"))
    }

    // ── Settings ────────────────────────────────────────────────────────────

    @Test fun settingsStartFromTheContractDefaults() {
        val s = PetOfferSettings(MemoryKeyValueStore())
        assertEquals(PetOfferContract.READ_OTHER_APPS_DEFAULT, s.readOtherApps)
        assertEquals(PetOfferContract.DEFAULT_ENABLED && PetOfferContract.READ_OTHER_APPS_DEFAULT, s.watching)
        assertEquals(PetOfferContract.DEFAULT_COOLDOWN_MS, s.cooldownMs)
        assertTrue(s.blockList.isEmpty() && s.muted.isEmpty())
    }

    @Test fun theSwitchTurnsWatchingOnAndOff() {
        val kv = MemoryKeyValueStore()
        val s = PetOfferSettings(kv)
        s.enabled = false
        s.setWatching(true)
        assertTrue(s.watching)
        s.setWatching(false)
        assertFalse(s.watching)
        assertTrue("turning reading off leaves own-failure offers alone", s.enabled)
    }

    @Test fun listsKeepOneEntryEachAndSurviveAReload() {
        val kv = MemoryKeyValueStore()
        val s = PetOfferSettings(kv)
        assertTrue(s.mute("com.chrome"))
        assertFalse(s.mute("COM.CHROME"))
        s.addBlock("My Bank\nApp")
        assertEquals(listOf("com.chrome"), PetOfferSettings(kv).muted)
        assertEquals(listOf("My Bank App"), PetOfferSettings(kv).blockList)
        assertTrue(s.unmute("com.chrome"))
        assertTrue(PetOfferSettings(kv).muted.isEmpty())
    }

    // ── Gate ────────────────────────────────────────────────────────────────

    @Test fun blockDefaultsMatchByNameEvenWhenThePackageIsUnknown() {
        val g = gate()
        assertEquals(PetNoticeGate.Block.PASSWORD_MANAGER, g.blockReason("Bitwarden", "com.unknown.vault"))
        assertEquals(PetNoticeGate.Block.BANKING, g.blockReason("Chase Mobile", "com.example.chase"))
        assertEquals(PetNoticeGate.Block.BANKING, g.blockReason("Navy Federal", null))
        assertNull("a word inside another word is not a bank", g.blockReason("Purchases", "com.shop"))
    }

    @Test fun blockDefaultsMatchByPackageWhenTheNameIsUnknown() {
        val g = gate()
        assertEquals(PetNoticeGate.Block.PASSWORD_MANAGER, g.blockReason(null, "com.x8bit.bitwarden"))
        assertEquals(PetNoticeGate.Block.MESSAGES, g.blockReason("Messages", "com.google.android.apps.messaging"))
        assertEquals(PetNoticeGate.Block.OWN_APP, g.blockReason("Allternit", "com.allternit.app"))
        assertNull(g.blockReason("Chrome", "com.android.chrome"))
    }

    @Test fun nameIsCheckedBeforePackage() {
        val kv = MemoryKeyValueStore()
        val s = PetOfferSettings(kv)
        s.mute("com.android.chrome")
        s.addBlock("Chrome")
        // Muted by package and blocked by name: the name decides.
        assertEquals(PetNoticeGate.Block.USER_BLOCKED, gate(kv).blockReason("Chrome", "com.android.chrome"))
        // A password manager whose package is not in the defaults is still caught by its name.
        assertEquals(PetNoticeGate.Block.PASSWORD_MANAGER, gate(kv).blockReason("1Password", "com.chrome.mute.me"))
    }

    @Test fun userBlockListAndMuteWorkByNameOrPackage() {
        val kv = MemoryKeyValueStore()
        val s = PetOfferSettings(kv)
        s.addBlock("Slack")
        s.mute("com.android.chrome")
        val g = gate(kv)
        assertEquals(PetNoticeGate.Block.USER_BLOCKED, g.blockReason("slack", "com.Slack"))
        assertEquals(PetNoticeGate.Block.MUTED, g.blockReason("Chrome", "com.android.chrome"))
        s.unmute("com.android.chrome")
        assertNull(g.blockReason("Chrome", "com.android.chrome"))
    }

    @Test fun theSameWindowIsNotReadAgainWithinThirtySeconds() {
        val g = gate()
        assertTrue(g.shouldRead("w1", 0))
        assertFalse(g.shouldRead("w1", PetOfferContract.SAME_WINDOW_REREAD_MS - 1))
        assertTrue(g.shouldRead("w2", 1))
        assertTrue(g.shouldRead("w1", PetOfferContract.SAME_WINDOW_REREAD_MS))
    }

    @Test fun oneOfferPerWindowPerCooldown() {
        val g = gate()
        val cd = PetOfferContract.DEFAULT_COOLDOWN_MS
        assertTrue(g.allowOffer("chrome", "Chrome isn't responding", 0))
        assertFalse(g.allowOffer("chrome", "Chrome keeps stopping", cd - 1))
        assertTrue("another window has its own cooldown", g.allowOffer("maps", "Maps keeps stopping", 1))
        assertTrue(g.allowOffer("chrome", "Chrome keeps stopping", cd))
    }

    @Test fun theSameLineIsNeverOfferedTwiceForOneWindow() {
        val g = gate()
        assertTrue(g.allowOffer("chrome", "Chrome isn't responding", 0))
        assertFalse(g.allowOffer("chrome", "Chrome isn't responding", 10 * PetOfferContract.DEFAULT_COOLDOWN_MS))
        assertTrue(g.allowOffer("maps", "Chrome isn't responding", 0))
    }
}
