package com.allternit.phonelink

import com.allternit.phonelink.core.MeshNodeApi
import mesh.Mesh
import java.io.File
import java.net.NetworkInterface

/**
 * The production [MeshNodeApi]: the gomobile-bound tsnet node from
 * `infrastructure/mesh/tsnet-ios` (built into `app/libs/Mesh.aar` by
 * `build-android.sh`). Only reference mesh.* classes here, never in core, so
 * JVM unit tests never load the Go runtime.
 */
class GomobileMeshNode(hostname: String) : MeshNodeApi {
    private val node = Mesh.newNode(hostname)

    override fun start(controlUrl: String, authKey: String, dataDir: File) {
        refreshInterfaces()
        node.start(controlUrl, authKey, dataDir.absolutePath)
    }

    /**
     * Android 11+ denies apps the netlink route dump Go's net.Interfaces uses
     * ("netlinkrib: permission denied"), so interfaces are listed here with
     * java.net.NetworkInterface and handed to tsnet (Mesh.setInterfaces), as
     * Tailscale's own Android client does.
     */
    override fun refreshInterfaces() {
        Mesh.setInterfaces(interfaceSpec())
    }

    private fun interfaceSpec(): String =
        NetworkInterface.getNetworkInterfaces()?.toList().orEmpty().joinToString("\n") { nif ->
            val addrs = nif.interfaceAddresses.mapNotNull { ia ->
                // IPv6 host strings carry a %zone suffix that a prefix can't hold.
                val host = ia.address?.hostAddress?.substringBefore('%') ?: return@mapNotNull null
                "$host/${ia.networkPrefixLength}"
            }
            "${nif.name} ${nif.index} ${nif.mtu} ${nif.isUp} ${nif.isLoopback} ${nif.isPointToPoint} ${nif.supportsMulticast()} | ${addrs.joinToString(" ")}"
        }

    override fun serveLocal(port: Int, localPort: Int) =
        node.serveLocal(port.toLong(), localPort.toLong())

    /**
     * Node.Health() (`state=…;ip=…;serve=…;warn=…`), looked up reflectively so
     * an older Mesh.aar without it still runs: the watchdog then falls back
     * to [state] and [meshIp].
     */
    private val healthMethod = try { node.javaClass.getMethod("health") } catch (_: Exception) { null }

    override fun health(): String =
        healthMethod?.let { m -> try { m.invoke(node) as? String } catch (_: Exception) { null } }.orEmpty()

    override fun state(): String = node.state()
    override fun meshIp(): String = node.meshIP()
    override fun close() = node.close()
}
