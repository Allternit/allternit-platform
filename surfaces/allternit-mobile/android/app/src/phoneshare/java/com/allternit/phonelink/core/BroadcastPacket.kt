package com.allternit.phonelink.core

import org.json.JSONObject
import java.io.ByteArrayOutputStream

/**
 * Wire framing for live screen video, byte-for-byte the same as
 * ios/Shared/AppPhone/LiveScreenPacket.swift (and the iCTRL live screen, so
 * viewers decode iPhone and Android alike):
 *
 *     u8 type | u32 BIG-endian payload length | payload[length]
 *
 * One packet per sealed binary WebSocket frame on the ingest socket. JSON
 * payloads are UTF-8 with keys in sorted order (Swift `.sortedKeys`).
 */
object BroadcastPacket {
    enum class Kind(val id: Int) {
        /** `{"broadcasting":bool}`; also a 2 s keepalive when nothing changes. */
        STATUS(0),
        /** `{"width","height","scale","rotation"}`: encoded pixel size, pixels per point, 0/90/180/270. */
        CONFIG(1),
        /** One H.264 access unit, Annex-B; every IDR carries SPS+PPS in-band. */
        FRAME(2),
        /** `{"reason":string}`: the broadcast stopped. */
        ENDED(3),
        /** `{"codec":"aac","sampleRate","channels","asc"}`. Unused on Android (audio is N/A). */
        AUDIO_CONFIG(4),
        /** One raw AAC-LC access unit. Unused on Android. */
        AUDIO(5);

        companion object {
            fun of(id: Int): Kind? = values().firstOrNull { it.id == id }
        }
    }

    const val HEADER_LENGTH = 5

    class Packet(val kind: Kind, val payload: ByteArray)

    fun encode(kind: Kind, payload: ByteArray): ByteArray {
        val n = payload.size
        val out = ByteArray(HEADER_LENGTH + n)
        out[0] = kind.id.toByte()
        out[1] = (n ushr 24 and 0xff).toByte()
        out[2] = (n ushr 16 and 0xff).toByte()
        out[3] = (n ushr 8 and 0xff).toByte()
        out[4] = (n and 0xff).toByte()
        System.arraycopy(payload, 0, out, HEADER_LENGTH, n)
        return out
    }

    /** JSON payload with sorted keys. Values: Boolean, Int/Long/Double/Float, String. */
    fun json(kind: Kind, fields: Map<String, Any>): ByteArray {
        val sb = StringBuilder("{")
        var first = true
        for (key in fields.keys.sorted()) {
            if (!first) sb.append(',')
            first = false
            sb.append(JSONObject.quote(key)).append(':')
            when (val v = fields.getValue(key)) {
                is Boolean, is Int, is Long -> sb.append(v.toString())
                is Number -> sb.append(JSONObject.numberToString(v))
                else -> sb.append(JSONObject.quote(v.toString()))
            }
        }
        sb.append('}')
        return encode(kind, sb.toString().toByteArray(Charsets.UTF_8))
    }

    fun status(broadcasting: Boolean) = json(Kind.STATUS, mapOf("broadcasting" to broadcasting))

    fun config(width: Int, height: Int, scale: Double, rotation: Int) =
        json(Kind.CONFIG, mapOf("width" to width, "height" to height, "scale" to scale, "rotation" to rotation))

    fun frame(accessUnit: ByteArray) = encode(Kind.FRAME, accessUnit)

    fun ended(reason: String) = json(Kind.ENDED, mapOf("reason" to reason))

    /** Splits a byte stream of packets (tests, diagnostics). Null when truncated or unknown type. */
    fun decodeAll(bytes: ByteArray): List<Packet>? {
        val out = ArrayList<Packet>()
        var i = 0
        while (i < bytes.size) {
            if (bytes.size - i < HEADER_LENGTH) return null
            val kind = Kind.of(bytes[i].toInt() and 0xff) ?: return null
            val n = (bytes[i + 1].toInt() and 0xff shl 24) or (bytes[i + 2].toInt() and 0xff shl 16) or
                (bytes[i + 3].toInt() and 0xff shl 8) or (bytes[i + 4].toInt() and 0xff)
            if (n < 0 || bytes.size - i - HEADER_LENGTH < n) return null
            val body = ByteArrayOutputStream(n)
            body.write(bytes, i + HEADER_LENGTH, n)
            out.add(Packet(kind, body.toByteArray()))
            i += HEADER_LENGTH + n
        }
        return out
    }
}
