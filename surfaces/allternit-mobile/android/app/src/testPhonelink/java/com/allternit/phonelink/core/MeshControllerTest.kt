package com.allternit.phonelink.core

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Scriptable [MeshNodeApi] standing in for the gomobile tsnet node. */
class FakeMeshNode(
    var hostname: String = "",
    private val failStart: Boolean = false,
    private val ip: String = "100.64.0.5",
) : MeshNodeApi {
    val started = CountDownLatch(1)
    var startArgs: Triple<String, String, File>? = null
    var serveLocalArgs: Pair<Int, Int>? = null
    var closed = false
    var stateOverride: String = "running"
    var refreshCount = 0
    var healthLine: String = ""

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
    override fun refreshInterfaces() { refreshCount++ }
    override fun health() = healthLine
}

/**
 * The join engine now runs inside the `:mesh` process and reports state
 * changes instead of owning an HTTP listener or the key box; the internal
 * viaMesh listener and the key lifecycle are main-process concerns (see
 * MeshSupervisorTest and the MeshManager wiring).
 */
class MeshControllerTest {
    @get:Rule val tmp = TemporaryFolder()

    private val reports = CopyOnWriteArrayList<MeshReport>()
    private var controller: MeshController? = null

    @After fun tearDown() { controller?.stop() }

    private fun params(authKey: String = "preauth-key-material") = MeshStartParams(
        controlUrl = "https://hs.example:8444",
        internalPort = 18200,
        hostname = "phone-12345678",
        meshStateDir = File(tmp.root, "mesh").absolutePath,
        authKey = authKey,
    )

    private fun newController(fake: FakeMeshNode, p: MeshStartParams = params()): MeshController {
        val c = MeshController(
            params = p,
            nodeFactory = { hostname -> fake.also { it.hostname = hostname } },
            onReport = { reports.add(it) },
            log = {},
        )
        controller = c
        return c
    }

    /** The `running` report lands after serveLocal, one hop behind `start`. */
    private fun awaitRunning(): MeshReport {
        val deadline = System.currentTimeMillis() + 10_000
        while (System.currentTimeMillis() < deadline) {
            reports.lastOrNull { it.state == "running" }?.let { return it }
            Thread.sleep(10)
        }
        throw AssertionError("no running report in $reports")
    }

    @Test fun joinStartsNodeAndReportsRunning() {
        val fake = FakeMeshNode()
        val c = newController(fake)
        c.start()

        assertTrue("node started", fake.started.await(10, TimeUnit.SECONDS))
        assertEquals("https://hs.example:8444", fake.startArgs!!.first)
        assertEquals("preauth-key-material", fake.startArgs!!.second)
        assertEquals(File(tmp.root, "mesh"), fake.startArgs!!.third)
        assertEquals("phone-12345678", fake.hostname)
        assertTrue("starting report first", reports.any { it.state == "starting" })
        // serveLocal and the running report both land right after start returns.
        val running = awaitRunning()
        // Serves the tailnet :8200 forward onto the main process's internal loopback port.
        assertEquals(8200 to 18200, fake.serveLocalArgs)
        assertEquals("100.64.0.5", running.meshIp)
        c.stop()
        assertTrue(fake.closed)
    }

    @Test fun failedJoinReportsError() {
        val fake = FakeMeshNode(failStart = true)
        val c = newController(fake)
        c.start()
        val deadline = System.currentTimeMillis() + 10_000
        while (reports.none { it.state.startsWith("error:") } && System.currentTimeMillis() < deadline) Thread.sleep(20)
        val error = reports.firstOrNull { it.state.startsWith("error:") }
        assertTrue("error report expected, got $reports", error != null)
        assertTrue(error!!.state.contains("control server unreachable"))
        // A failed join reports no mesh address.
        assertNull(error.meshIp)
    }

    @Test fun rejoinPassesEmptyKeyForStateDirAuth() {
        // After a reboot there is no stored key; tsnet re-authenticates from the state dir.
        val fake = FakeMeshNode()
        val c = newController(fake, params(authKey = ""))
        c.start()
        assertTrue(fake.started.await(10, TimeUnit.SECONDS))
        assertEquals("", fake.startArgs!!.second)
    }

    @Test fun networkChangeRefreshesInterfacesAndReports() {
        val fake = FakeMeshNode()
        val c = newController(fake)
        c.start()
        awaitRunning()
        val refreshesAfterJoin = fake.refreshCount

        fake.stateOverride = "needs_login"
        c.onNetworkChanged()
        assertTrue(fake.refreshCount > refreshesAfterJoin)
        assertEquals("needs_login", reports.last().state)
        assertEquals("100.64.0.5", reports.last().meshIp)
    }

    @Test fun stopClosesNodeAndSuppressesLateJoins() {
        val fake = FakeMeshNode()
        val c = newController(fake)
        c.stop()
        c.start()
        assertTrue(fake.started.await(10, TimeUnit.SECONDS))
        // The join posts its "running" report just after the node starts; count after it.
        awaitRunning()
        val countAfterJoin = reports.size
        c.stop()
        assertTrue(fake.closed)
        // No further reports once stopped (a late retry of a dead generation).
        Thread.sleep(300)
        assertEquals(countAfterJoin, reports.size)
    }

    /** The internal loopback listener the mesh forwards to marks requests viaMesh. */
    @Test fun internalListenerMarksRequestsViaMesh() {
        val seen = CopyOnWriteArrayList<HttpRequest>()
        val internal = HttpServer({ req -> seen.add(req); HttpResponse.text(200, "ok") }, viaMesh = true, log = {})
        try {
            internal.start(java.net.InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), 0)
            java.net.Socket("127.0.0.1", internal.localPort).use { s ->
                s.getOutputStream().write("GET / HTTP/1.1\r\nHost: x\r\n\r\n".toByteArray())
                s.getInputStream().readBytes()
            }
            assertEquals(1, seen.size)
            assertTrue("request through the mesh listener must be marked", seen[0].viaMesh)

            // And the server's own listener does not mark: only the dedicated one does.
            val plainSeen = CopyOnWriteArrayList<HttpRequest>()
            val plain = HttpServer({ req -> plainSeen.add(req); HttpResponse.text(200, "ok") }, log = {})
            try {
                plain.start(java.net.InetAddress.getByAddress(byteArrayOf(127, 0, 0, 1)), 0)
                java.net.Socket("127.0.0.1", plain.localPort).use { s ->
                    s.getOutputStream().write("GET / HTTP/1.1\r\nHost: x\r\n\r\n".toByteArray())
                    s.getInputStream().readBytes()
                }
                assertEquals(1, plainSeen.size)
                assertFalse(plainSeen[0].viaMesh)
            } finally {
                plain.stop()
            }
        } finally {
            internal.stop()
        }
    }

    @Test fun restartClosesTheNodeAndRejoinsOnTheSameStateDir() {
        val nodes = CopyOnWriteArrayList<FakeMeshNode>()
        val c = MeshController(
            params = params(authKey = ""),
            nodeFactory = { h -> FakeMeshNode(hostname = h).also { nodes.add(it) } },
            onReport = { reports.add(it) },
        )
        controller = c
        c.start()
        awaitRunning()
        assertEquals(1, nodes.size)
        reports.clear()

        c.restart("test")
        val deadline = System.currentTimeMillis() + 10_000
        while (nodes.size < 2 || reports.none { it.state == "running" }) {
            if (System.currentTimeMillis() > deadline) throw AssertionError("no rejoin: $reports")
            Thread.sleep(10)
        }
        assertTrue("old node closed", nodes[0].closed)
        assertFalse("new node live", nodes[1].closed)
        assertEquals(File(tmp.root, "mesh"), nodes[1].startArgs!!.third)
        assertEquals("starting", reports.first().state)
    }

    @Test fun healthSnapshotParsesNodeHealthAndFallsBackForOlderAars() {
        val fake = FakeMeshNode()
        val c = newController(fake)
        assertNull("no node yet", c.healthSnapshot())
        c.start()
        awaitRunning()
        // Older Mesh.aar: Health() missing -> state + address only.
        assertEquals(MeshHealth("running", "100.64.0.5", "unknown"), c.healthSnapshot())
        fake.healthLine = "state=running;ip=100.64.0.5;serve=dead;warn=not-in-map-poll"
        val h = c.healthSnapshot()!!
        assertEquals("dead", h.serve)
        assertEquals(setOf("not-in-map-poll"), h.warnings)
    }
}
