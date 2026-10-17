package com.allternit.phonelink

import com.allternit.phonelink.core.MeshNodeApi
import mesh.Mesh
import java.io.File

/**
 * The production [MeshNodeApi]: the gomobile-bound tsnet node from
 * `infrastructure/mesh/tsnet-ios` (built into `app/libs/Mesh.aar` by
 * `build-android.sh`). Only reference mesh.* classes here, never in core, so
 * JVM unit tests never load the Go runtime.
 */
class GomobileMeshNode(hostname: String) : MeshNodeApi {
    private val node = Mesh.newNode(hostname)

    override fun start(controlUrl: String, authKey: String, dataDir: File) =
        node.start(controlUrl, authKey, dataDir.absolutePath)

    override fun serveLocal(port: Int, localPort: Int) =
        node.serveLocal(port.toLong(), localPort.toLong())

    override fun state(): String = node.state()
    override fun meshIp(): String = node.meshIP()
    override fun close() = node.close()
}
