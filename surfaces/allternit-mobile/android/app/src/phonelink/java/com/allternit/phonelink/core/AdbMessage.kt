/*
 * Adapted from Shizuku (https://github.com/RikkaApps/Shizuku,
 * manager/src/main/java/moe/shizuku/manager/adb/AdbMessage.kt and
 * AdbProtocol.kt), Copyright RikkaApps, Apache License 2.0. See THIRD-PARTY-NOTICES.md.
 */
package com.allternit.phonelink.core

import java.io.DataInputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** adb wire protocol constants (system/core/adb/protocol.txt). */
object AdbProtocol {
    const val A_CNXN = 0x4e584e43
    const val A_AUTH = 0x48545541
    const val A_OPEN = 0x4e45504f
    const val A_OKAY = 0x59414b4f
    const val A_CLSE = 0x45534c43
    const val A_WRTE = 0x45545257
    const val A_STLS = 0x534C5453

    const val A_VERSION = 0x01000001
    const val A_MAXDATA = 256 * 1024
    const val A_STLS_VERSION = 0x01000000
}

/** One adb message: a 24-byte little-endian header plus an optional payload. */
class AdbMessage(val command: Int, val arg0: Int, val arg1: Int, val data: ByteArray = ByteArray(0)) {

    constructor(command: Int, arg0: Int, arg1: Int, data: String) :
        this(command, arg0, arg1, "$data\u0000".toByteArray(Charsets.UTF_8))

    fun toByteArray(): ByteArray =
        ByteBuffer.allocate(HEADER_LENGTH + data.size).order(ByteOrder.LITTLE_ENDIAN).apply {
            putInt(command)
            putInt(arg0)
            putInt(arg1)
            putInt(data.size)
            putInt(checksum(data))
            putInt(command xor -1)
            put(data)
        }.array()

    override fun toString(): String = "AdbMessage(${name(command)}, arg0=$arg0, arg1=$arg1, ${data.size} bytes)"

    companion object {
        const val HEADER_LENGTH = 24
        /** Refuse absurd payload lengths before allocating (adbd's own cap is 1 MiB). */
        const val MAX_PAYLOAD = 1024 * 1024

        /** The legacy byte-sum checksum; adbd ignores it from A_VERSION 0x01000001 on, but older ones check it. */
        fun checksum(data: ByteArray): Int {
            var sum = 0
            for (b in data) sum += b.toInt() and 0xff
            return sum
        }

        fun read(input: DataInputStream): AdbMessage {
            val header = ByteArray(HEADER_LENGTH)
            input.readFully(header)
            val b = ByteBuffer.wrap(header).order(ByteOrder.LITTLE_ENDIAN)
            val command = b.int
            val arg0 = b.int
            val arg1 = b.int
            val length = b.int
            b.int // checksum: not checked, see above
            val magic = b.int
            require(magic == command xor -1) { "bad adb message magic" }
            require(length in 0..MAX_PAYLOAD) { "bad adb payload length $length" }
            val data = ByteArray(length)
            input.readFully(data)
            return AdbMessage(command, arg0, arg1, data)
        }

        fun name(command: Int): String = when (command) {
            AdbProtocol.A_CNXN -> "CNXN"
            AdbProtocol.A_AUTH -> "AUTH"
            AdbProtocol.A_OPEN -> "OPEN"
            AdbProtocol.A_OKAY -> "OKAY"
            AdbProtocol.A_CLSE -> "CLSE"
            AdbProtocol.A_WRTE -> "WRTE"
            AdbProtocol.A_STLS -> "STLS"
            else -> "0x" + Integer.toHexString(command)
        }
    }
}
