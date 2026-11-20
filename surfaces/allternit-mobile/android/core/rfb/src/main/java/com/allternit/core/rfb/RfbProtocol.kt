package com.allternit.core.rfb

/*
 * RFB 3.8 protocol primitives for the native screen viewer: wire messages, the incremental stream
 * parser, typed errors and the X11 keysym table (iOS RFBProtocol.swift).
 *
 * The server is a blind proxy (cmd/allternit-api/src/bot_desktop_stream.rs) that shuttles BINARY
 * WebSocket frames to a VNC server over 16 KB TCP reads, so RFB bytes arrive fragmented at arbitrary
 * boundaries: [RfbStreamParser] buffers and only emits complete messages. All multi-byte integers
 * are big-endian on the wire (RFB 3.8 §6/§7).
 */

/** RFB encodings the client asks for in SetEncodings (RFB 3.8 §7.7). */
object RfbEncoding {
    const val RAW = 0
    const val COPY_RECT = 1
    const val ZRLE = 16
}

/** Typed failures: a decode problem throws one of these instead of drawing garbage. */
sealed class RfbException(message: String) : Exception(message) {
    class InvalidUrl(path: String) : RfbException("Couldn't build a WebSocket URL from $path.")
    class InvalidProtocolVersion(line: String) : RfbException("Server didn't send an RFB version line (got $line).")
    class UnsupportedProtocolVersion(line: String) : RfbException("Server speaks $line; the viewer requires RFB 3.8.")
    class SecurityHandshakeFailed(reason: String) : RfbException("VNC security handshake failed: $reason")
    class NoSupportedSecurityType(types: List<Int>) : RfbException("This computer's screen sharing uses a sign-in the app doesn't support (offered: $types).")
    class InvalidFramebufferSize(w: Int, h: Int) : RfbException("Server advertised an invalid framebuffer (${w}×$h).")
    class UnexpectedMessage(detail: String) : RfbException("Unexpected RFB message: $detail")
    class UnknownServerMessage(type: Int) : RfbException("Unknown RFB server message type $type.")
    class UnsupportedEncoding(encoding: Int) : RfbException("Server used rectangle encoding $encoding, which was never requested.")
    class RectangleOutOfBounds : RfbException("A framebuffer rectangle extends outside the negotiated screen.")
    class MalformedRectanglePayload : RfbException("A framebuffer rectangle's payload doesn't match its encoding.")
    class InvalidZrleSubencoding(sub: Int) : RfbException("Invalid ZRLE tile subencoding $sub.")
    class MalformedZrleTile : RfbException("A ZRLE tile produced more pixels than it contains.")
    class ZrleUnderflow : RfbException("The ZRLE stream ended mid-rectangle.")
    class ZrleInflateFailed(detail: String) : RfbException("ZRLE decompression failed ($detail).")
    class NotConnected : RfbException("The desktop stream isn't connected.")
    /** The computer's screen sharing asks for a sign-in the viewer doesn't have yet (Apple's needs the user name too). */
    class CredentialsRequired(val needsUsername: Boolean) : RfbException("This computer's screen sharing asks for a password.")
    /** SecurityResult said no: wrong name or password, or access refused. */
    class AuthenticationFailed(reason: String) : RfbException(reason)
}

/** ServerInit (RFB 3.8 §7.3.2): the framebuffer geometry and desktop name. */
data class RfbServerInit(val width: Int, val height: Int, val name: String)

/** One FramebufferUpdate rectangle with its still-encoded payload (ZRLE: the u32 length already stripped). */
class RfbRectangle(val x: Int, val y: Int, val width: Int, val height: Int, val encoding: Int, val payload: ByteArray)

/** Complete server→client messages emitted by [RfbStreamParser]. */
sealed interface RfbServerMessage {
    data class ProtocolVersion(val line: String) : RfbServerMessage
    data class SecurityTypes(val types: List<Int>) : RfbServerMessage
    data class SecurityFailure(val reason: String) : RfbServerMessage
    data class ServerInit(val info: RfbServerInit) : RfbServerMessage
    class FramebufferUpdate(val rects: List<RfbRectangle>) : RfbServerMessage
    data object SetColourMapEntries : RfbServerMessage
    data object Bell : RfbServerMessage
    data object ServerCutText : RfbServerMessage
}

/**
 * Incremental parser for the server→client half of RFB. [append] as frames arrive; [nextMessage]
 * returns null until a whole message is buffered and consumes nothing for a partial one.
 */
class RfbStreamParser {
    enum class Phase { Version, Security, ServerInit, Messages }

    var phase: Phase = Phase.Version
        private set

    private var buffer = ByteArray(64 * 1024)
    private var start = 0
    private var end = 0

    private val available: Int get() = end - start

    fun append(data: ByteArray) {
        if (start > 0) {
            // Compact first so the buffer can't grow between updates.
            System.arraycopy(buffer, start, buffer, 0, end - start)
            end -= start
            start = 0
        }
        if (end + data.size > buffer.size) {
            buffer = buffer.copyOf(maxOf(buffer.size * 2, end + data.size))
        }
        System.arraycopy(data, 0, buffer, end, data.size)
        end += data.size
    }

    fun nextMessage(): RfbServerMessage? = when (phase) {
        Phase.Version -> parseVersion()
        Phase.Security -> parseSecurity()
        Phase.ServerInit -> parseServerInit()
        Phase.Messages -> parseServerToClient()
    }

    /** Exactly [count] raw bytes (the security exchange), or null with nothing consumed. */
    fun takeBytes(count: Int): ByteArray? {
        if (available < count) return null
        val out = bytes(0, count)
        start += count
        return out
    }

    private fun parseVersion(): RfbServerMessage? {
        if (available < 12) return null
        val line = String(bytes(0, 12), Charsets.US_ASCII)
        if (!line.startsWith("RFB ")) throw RfbException.InvalidProtocolVersion(line)
        start += 12
        phase = Phase.Security
        return RfbServerMessage.ProtocolVersion(line)
    }

    private fun parseSecurity(): RfbServerMessage? {
        if (available < 1) return null
        val count = u8(0)
        if (count == 0) {
            if (available < 5) return null
            val len = u32(1).toInt()
            if (available < 5 + len) return null
            val reason = String(bytes(5, len), Charsets.UTF_8).ifEmpty { "unknown reason" }
            start += 5 + len
            return RfbServerMessage.SecurityFailure(reason)
        }
        if (available < 1 + count) return null
        val types = (0 until count).map { u8(1 + it) }
        start += 1 + count
        phase = Phase.ServerInit
        return RfbServerMessage.SecurityTypes(types)
    }

    private fun parseServerInit(): RfbServerMessage? {
        if (available < 24) return null
        val width = u16(0)
        val height = u16(2)
        // Bytes 4..<20 are the server's pixel format: overridden by SetPixelFormat right after.
        val nameLength = u32(20).toInt()
        if (available < 24 + nameLength) return null
        val name = String(bytes(24, nameLength), Charsets.UTF_8)
        start += 24 + nameLength
        phase = Phase.Messages
        return RfbServerMessage.ServerInit(RfbServerInit(width, height, name))
    }

    private fun parseServerToClient(): RfbServerMessage? {
        if (available < 1) return null
        return when (val type = u8(0)) {
            0 -> parseFramebufferUpdate()
            1 -> {
                if (available < 6) return null
                val colours = u16(4)
                if (available < 6 + 6 * colours) return null
                start += 6 + 6 * colours
                RfbServerMessage.SetColourMapEntries
            }
            2 -> {
                start += 1
                RfbServerMessage.Bell
            }
            3 -> {
                if (available < 8) return null
                val len = u32(4).toInt()
                if (available < 8 + len) return null
                start += 8 + len
                RfbServerMessage.ServerCutText
            }
            else -> throw RfbException.UnknownServerMessage(type)
        }
    }

    /** The whole update is emitted at once; a partial one leaves everything unconsumed. */
    private fun parseFramebufferUpdate(): RfbServerMessage? {
        if (available < 4) return null
        val rectCount = u16(2)
        var pos = 4
        val rects = ArrayList<RfbRectangle>(rectCount)
        repeat(rectCount) {
            if (available < pos + 12) return null
            val x = u16(pos)
            val y = u16(pos + 2)
            val w = u16(pos + 4)
            val h = u16(pos + 6)
            val encoding = u32(pos + 8).toInt()
            pos += 12
            val payload: ByteArray
            when (encoding) {
                RfbEncoding.RAW -> {
                    val len = w * h * 4
                    if (available < pos + len) return null
                    payload = bytes(pos, len)
                    pos += len
                }
                RfbEncoding.COPY_RECT -> {
                    if (available < pos + 4) return null
                    payload = bytes(pos, 4)
                    pos += 4
                }
                RfbEncoding.ZRLE -> {
                    if (available < pos + 4) return null
                    val len = u32(pos).toInt()
                    if (len < 0 || available < pos + 4 + len) return null
                    payload = bytes(pos + 4, len)
                    pos += 4 + len
                }
                else -> throw RfbException.UnsupportedEncoding(encoding)
            }
            rects.add(RfbRectangle(x, y, w, h, encoding, payload))
        }
        start += pos
        return RfbServerMessage.FramebufferUpdate(rects)
    }

    private fun u8(pos: Int): Int = buffer[start + pos].toInt() and 0xFF
    private fun u16(pos: Int): Int = (u8(pos) shl 8) or u8(pos + 1)
    private fun u32(pos: Int): Long =
        (u8(pos).toLong() shl 24) or (u8(pos + 1).toLong() shl 16) or (u8(pos + 2).toLong() shl 8) or u8(pos + 3).toLong()
    private fun bytes(pos: Int, count: Int): ByteArray = buffer.copyOfRange(start + pos, start + pos + count)
}

/** Client→server messages (RFB 3.8 §7.5): one message per binary WebSocket frame. */
object RfbClientMessage {
    fun protocolVersion(): ByteArray = "RFB 003.008\n".toByteArray(Charsets.US_ASCII)

    fun securitySelection(type: Int): ByteArray = byteArrayOf(type.toByte())

    /** ClientInit: shared-flag 1 (the bot keeps its own session alive). */
    fun clientInit(): ByteArray = byteArrayOf(1)

    /**
     * SetPixelFormat: 32 bpp, depth 24, true colour, little-endian, maxima 255, shifts R16/G8/B0.
     * Wire pixels are then [B, G, R, pad]; ZRLE CPIXELs [B, G, R].
     */
    fun setPixelFormat(): ByteArray = Wire().apply {
        u8(0); u8(0); u8(0); u8(0)
        u8(32); u8(24); u8(0); u8(1)
        u16(255); u16(255); u16(255)
        u8(16); u8(8); u8(0)
        u8(0); u8(0); u8(0)
    }.bytes()

    /** SetEncodings: Raw + CopyRect + ZRLE. */
    fun setEncodings(): ByteArray = Wire().apply {
        u8(2); u8(0); u16(3)
        u32(RfbEncoding.RAW.toLong()); u32(RfbEncoding.COPY_RECT.toLong()); u32(RfbEncoding.ZRLE.toLong())
    }.bytes()

    fun framebufferUpdateRequest(incremental: Boolean, width: Int, height: Int): ByteArray = Wire().apply {
        u8(3); u8(if (incremental) 1 else 0); u16(0); u16(0); u16(width); u16(height)
    }.bytes()

    fun keyEvent(keysym: Long, down: Boolean): ByteArray = Wire().apply {
        u8(4); u8(if (down) 1 else 0); u16(0); u32(keysym)
    }.bytes()

    /** PointerEvent: button mask bit 0 left, 1 middle, 2 right. */
    fun pointerEvent(buttonMask: Int, x: Int, y: Int): ByteArray = Wire().apply {
        u8(5); u8(buttonMask); u16(x); u16(y)
    }.bytes()

    private class Wire {
        private val out = java.io.ByteArrayOutputStream()
        fun u8(v: Int) = out.write(v and 0xFF)
        fun u16(v: Int) { u8(v shr 8); u8(v) }
        fun u32(v: Long) { u8((v shr 24).toInt()); u8((v shr 16).toInt()); u8((v shr 8).toInt()); u8(v.toInt()) }
        fun bytes(): ByteArray = out.toByteArray()
    }
}

/** The X11 keysyms the viewer sends (RFB 3.8 §7.5.4). Printable Latin-1 maps 1:1. */
object RfbKeysym {
    const val BACK_SPACE = 0xFF08L
    const val TAB = 0xFF09L
    const val RETURN = 0xFF0DL
    const val ESCAPE = 0xFF1BL
    const val DELETE = 0xFFFFL
    const val LEFT = 0xFF51L
    const val UP = 0xFF52L
    const val RIGHT = 0xFF53L
    const val DOWN = 0xFF54L
    const val SHIFT_LEFT = 0xFFE1L
    const val SHIFT_RIGHT = 0xFFE2L
    const val CONTROL_LEFT = 0xFFE3L
    const val CONTROL_RIGHT = 0xFFE4L
    const val ALT_LEFT = 0xFFE9L
    /** The hardware Meta key maps to Super_L, as iOS maps ⌘. */
    const val SUPER_LEFT = 0xFFEBL
    const val SUPER_RIGHT = 0xFFECL

    /** Keysym for a printable code point, or null (emoji, CJK…: the remote side has none either). */
    fun forCodePoint(cp: Int): Long? = when (cp) {
        in 0x20..0x7E, in 0xA0..0xFF -> cp.toLong()
        else -> null
    }
}
