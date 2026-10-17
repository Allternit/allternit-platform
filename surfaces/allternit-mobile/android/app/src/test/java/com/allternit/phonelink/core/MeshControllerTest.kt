package com.allternit.phonelink.core

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit

/** Scriptable [MeshNodeApi] standing in for the gomobile tsnet node. */
class FakeMeshNode(
    var hostname: String = "",
    private val failStart: Boolean = false,
    private val ip: String = "100.64.0.5",
) : MeshNodeApi {
    val started = java.util.concurrent.CountDownLatch(1)
    var startArgs: Triple<String, String, File>? = null
    var serveLocalArgs: Pair<Int, Int>? = null
    var closed = false
    var stateOverride: String = "running"

    override fun start(controlUrl: String, authKey: String, dataDir: File) {
        if (failStart) throw RuntimeException("control server unreachable")
        startArgs = Triple(controlUrl, authKey, dataDir)
        started.countDown()
    }

    override fun serveLocal(port: Int, localPort: Int) {
        serveLocalArgs = port to localPort
    }

    override fun state() = stateOverride
    override fun meshIp() = ip
    override fun close() { closed = true }
}

class MeshControllerTest {
    @get:Rule val tmp = TemporaryFolder()

    private val kv = MemoryKeyValueStore()
    private val settings = AppSettings(kv, defaultName = { "phone" }, newId = { "12345678-abcd" })
    private val box = MeshKeyBox(kv, AesGcmTestSealer())
    private val seenRequests = CopyOnWriteArrayList<HttpRequest>()
    private var controller: MeshController? = null

    @After fun tearDown() { controller?.stop() }

    private fun newController(fake: FakeMeshNode): MeshController {
        val c = MeshController(
            settings = settings,
            keyBox = box,
            meshDir = File(tmp.root, "mesh"),
            handler = { req -> seenRequests.add(req); HttpResponse.text(200, "ok") },
            nodeFactory = { hostname -> fake.also { it.hostname = hostname } },
            log = {},
        )
        controller = c
        return c
    }

    @Test fun noJoinWithoutMeshSettings() {
        var made = 0
        val fake = FakeMeshNode()
        val c = MeshController(settings, box, File(tmp.root, "mesh"), { HttpResponse.text(200, "ok") }, { made++; fake })
        controller = c
        c.start()
        Thread.sleep(200)
        assertEquals(0, made)
        assertEquals("stopped", c.state())
        assertNull(c.host())
    }

    @Test fun joinUsesSealedKeyThenDropsIt() {
        settings.meshControlUrl = "https://hs.example:8444"
        box.store("preauth-key-material")
        val fake = FakeMeshNode()
        val c = newController(fake)
        assertEquals(18200, c.internalServePort())

        c.start()
        assertTrue("node started", fake.started.await(10, TimeUnit.SECONDS))
        assertEquals("https://hs.example:8444", fake.startArgs!!.first)
        assertEquals("preauth-key-material", fake.startArgs!!.second)
        assertEquals(File(tmp.root, "mesh"), fake.startArgs!!.third)
        assertEquals("phone-12345678", fake.hostname)
        // Serves the tailnet :8200 forward onto the dedicated internal loopback port.
        assertEquals(8200 to 18200, fake.serveLocalArgs)
        // The pre-auth key is single-use: gone after the successful join.
        assertTrue("key dropped after join", !box.hasKey())
        assertEquals("running", c.state())
        assertEquals("100.64.0.5:8200", c.host())
        c.stop()
        assertTrue(fake.closed)
        assertEquals("stopped", c.state())
        assertNull(c.host())
    }

    @Test fun failedJoinKeepsTheKey() {
        settings.meshControlUrl = "https://hs.example:8444"
        box.store("preauth-key-material")
        val fake = FakeMeshNode(failStart = true)
        val c = newController(fake)
        c.start()
        for (i in 0..100) {
            if (c.state().startsWith("error:")) break
            Thread.sleep(50)
        }
        assertTrue("state should carry the error, got ${c.state()}", c.state().startsWith("error:"))
        assertTrue("key kept after a failed join", box.hasKey())
        assertEquals("preauth-key-material", box.read())
        assertNull(c.host())
    }

    @Test fun joinWithoutStoredKeyStillStarts() {
        settings.meshControlUrl = "https://hs.example:8444"
        val fake = FakeMeshNode()
        val c = newController(fake)
        c.start()
        assertTrue(fake.started.await(10, TimeUnit.SECONDS))
        // Re-join after reboot: tsnet re-authenticates from its state dir; empty key.
        assertEquals("", fake.startArgs!!.second)
    }

    @Test fun internalPortTracksServerPort() {
        settings.port = 9000
        val c = newController(FakeMeshNode())
        assertEquals(19000, c.internalServePort())
    }

    /** The internal loopback listener the mesh forwards to marks requests viaMesh. */
    @Test fun meshListenerMarksRequestsViaMesh() {
        settings.meshControlUrl = "https://hs.example:8444"
        val fake = FakeMeshNode()
        val c = newController(fake)
        c.start()
        assertTrue(fake.started.await(10, TimeUnit.SECONDS))
        val internal = fake.serveLocalArgs!!.second
        java.net.Socket("127.0.0.1", internal).use { s ->
            s.getOutputStream().write("GET / HTTP/1.1\r\nHost: x\r\n\r\n".toByteArray())
            s.getInputStream().readBytes()
        }
        assertEquals(1, seenRequests.size)
        assertTrue("request through the mesh listener must be marked", seenRequests[0].viaMesh)
    }
}
