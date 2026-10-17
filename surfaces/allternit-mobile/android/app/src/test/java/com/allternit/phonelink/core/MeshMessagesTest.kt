package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The wire codec between the main and `:mesh` processes: round-trips and never invents fields. */
class MeshMessagesTest {
    private val paramsWithKey = MeshStartParams(
        controlUrl = "https://mail.news.allternit.com:8444",
        internalPort = 18200,
        hostname = "phone-12345678",
        meshStateDir = "/data/user/0/com.allternit.phonelink/files/mesh",
        authKey = "hskey-single-use-material",
    )

    @Test fun startRoundTripsWithKey() {
        val decoded = MeshMessages.decodeStart(MeshMessages.encodeStart(paramsWithKey))!!
        assertEquals(paramsWithKey, decoded)
    }

    @Test fun startRoundTripsWithEmptyKey() {
        // Re-join after reboot: no stored pre-auth key, tsnet uses its state dir.
        val decoded = MeshMessages.decodeStart(MeshMessages.encodeStart(paramsWithKey.copy(authKey = "")))!!
        assertEquals("", decoded.authKey)
        assertEquals(paramsWithKey.controlUrl, decoded.controlUrl)
        assertEquals(18200, decoded.internalPort)
    }

    @Test fun startSurvivesSpecialCharacters() {
        val tricky = paramsWithKey.copy(
            authKey = "line1\nline2=with&delims % + state=running ünïcode",
            controlUrl = "https://hs.example:8444/path?a=b&c=d",
        )
        val encoded = MeshMessages.encodeStart(tricky)
        // URL-encoding keeps every value on its own line: no format smuggling.
        assertEquals(5, encoded.lines().size)
        assertEquals(tricky, MeshMessages.decodeStart(encoded)!!)
    }

    @Test fun decodeStartRejectsMalformed() {
        assertNull(MeshMessages.decodeStart(""))
        assertNull(MeshMessages.decodeStart("not-a-message"))
        assertNull(MeshMessages.decodeStart("controlUrl=https://hs.example\ninternalPort=abc\nhostname=h\nmeshStateDir=/m"))
        assertNull(MeshMessages.decodeStart("controlUrl=\ninternalPort=18200\nhostname=h\nmeshStateDir=/m"))
        assertNull(MeshMessages.decodeStart("internalPort=18200\nhostname=h\nmeshStateDir=/m"))
    }

    @Test fun reportRoundTrips() {
        val withIp = MeshReport("running", "100.64.0.5")
        assertEquals(withIp, MeshMessages.decodeReport(MeshMessages.encodeReport(withIp))!!)

        val error = MeshReport("error:control server unreachable", null)
        val decodedError = MeshMessages.decodeReport(MeshMessages.encodeReport(error))!!
        assertEquals(error.state, decodedError.state)
        assertNull(decodedError.meshIp)

        val noIp = MeshReport("needs_login", null)
        assertNull(MeshMessages.decodeReport(MeshMessages.encodeReport(noIp))!!.meshIp)
    }

    @Test fun decodeReportRejectsMalformed() {
        assertNull(MeshMessages.decodeReport(""))
        assertNull(MeshMessages.decodeReport("meshIp=100.64.0.5"))
        assertNull(MeshMessages.decodeReport("state="))
        // An empty optional meshIp is tolerated as "no address", not a format error.
        assertEquals("running", MeshMessages.decodeReport("state=running\nmeshIp=")!!.state)
    }
}
