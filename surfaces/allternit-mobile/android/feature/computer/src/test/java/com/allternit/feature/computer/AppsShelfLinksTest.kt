package com.allternit.feature.computer

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** A shelf row's link decides what the web view loads, so foreign hosts and non-https never open. */
class AppsShelfLinksTest {
    @Test fun hostedAndRelativeLinksOpen() {
        assertEquals("https://notes.allternitusercontent.com/", AppsShelfLinks.openUrl("https://notes.allternitusercontent.com/"))
        assertEquals("https://ai.allternit.com/app/notes", AppsShelfLinks.openUrl("/app/notes"))
    }

    @Test fun foreignHostsAndSchemesAreRefused() {
        assertNull(AppsShelfLinks.openUrl("https://evil.example.com/"))
        assertNull(AppsShelfLinks.openUrl("https://allternitusercontent.com.evil.com/"))
        assertNull(AppsShelfLinks.openUrl("http://notes.allternitusercontent.com/"))
        assertNull(AppsShelfLinks.openUrl("javascript:alert(1)"))
        assertNull(AppsShelfLinks.openUrl(null))
    }
}
