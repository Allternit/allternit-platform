package com.allternit.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PlayLaunchUrlTest {

    private val defaultUrl = "https://m.allternit.com/"
    private val shareTarget =
        """{"action": "/share-target", "method": "GET", "params": {"title": "title", "text": "text"}}"""

    @Test
    fun defaultUrlGetsTheMarker() {
        assertEquals("https://m.allternit.com/?src=play", PlayLaunchUrl.withPlayMarker(defaultUrl))
    }

    @Test
    fun shortcutKeepsItsOwnParams() {
        assertEquals(
            "https://m.allternit.com/?allternit_open=ask&src=play",
            PlayLaunchUrl.withPlayMarker("https://m.allternit.com/?allternit_open=ask"),
        )
    }

    @Test
    fun fragmentStaysAfterTheQuery() {
        assertEquals(
            "https://m.allternit.com/settings/billing?tab=plans&src=play#top",
            PlayLaunchUrl.withPlayMarker("https://m.allternit.com/settings/billing?tab=plans#top"),
        )
    }

    @Test
    fun anotherSrcValueIsReplacedNotDuplicated() {
        assertEquals(
            "https://m.allternit.com/?a=1&src=play",
            PlayLaunchUrl.withPlayMarker("https://m.allternit.com/?src=web&a=1"),
        )
        assertEquals(
            "https://m.allternit.com/?src=play",
            PlayLaunchUrl.withPlayMarker("https://m.allternit.com/?src=play"),
        )
    }

    @Test
    fun paramsThatOnlyStartWithSrcAreKept() {
        assertEquals(
            "https://m.allternit.com/?srcset=x&src=play",
            PlayLaunchUrl.withPlayMarker("https://m.allternit.com/?srcset=x"),
        )
    }

    @Test
    fun nonWebUrlsAreLeftAlone() {
        assertEquals("allternit://phone-share?code=1", PlayLaunchUrl.withPlayMarker("allternit://phone-share?code=1"))
    }

    @Test
    fun textShareOpensTheShareTargetWithTheMarker() {
        assertEquals(
            "https://m.allternit.com/share-target?title=Read+this&text=a+%26+b+https%3A%2F%2Fx.io&src=play",
            PlayLaunchUrl.shareUrl(defaultUrl, shareTarget, "Read this", "a & b https://x.io"),
        )
    }

    @Test
    fun shareWithOnlyTextSkipsTheTitle() {
        assertEquals(
            "https://m.allternit.com/share-target?text=hi&src=play",
            PlayLaunchUrl.shareUrl(defaultUrl, shareTarget, null, "hi"),
        )
    }

    @Test
    fun shareFallsBackToChromeWhenItCannotBuildTheUrl() {
        assertNull(PlayLaunchUrl.shareUrl(defaultUrl, shareTarget, null, null))
        assertNull(PlayLaunchUrl.shareUrl(defaultUrl, null, "t", "x"))
        assertNull(PlayLaunchUrl.shareUrl(defaultUrl, "not json", "t", "x"))
        assertNull(
            PlayLaunchUrl.shareUrl(
                defaultUrl,
                """{"action": "/share-target", "method": "POST", "params": {"text": "text"}}""",
                null,
                "x",
            ),
        )
    }
}
