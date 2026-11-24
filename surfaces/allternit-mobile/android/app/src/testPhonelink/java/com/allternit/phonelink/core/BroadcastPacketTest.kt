package com.allternit.phonelink.core

import org.json.JSONObject
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BroadcastPacketTest {
    @Test fun headerIsTypeThenBigEndianLength() {
        val payload = ByteArray(0x0102)
        val p = BroadcastPacket.encode(BroadcastPacket.Kind.FRAME, payload)
        assertEquals(5 + 0x0102, p.size)
        assertArrayEquals(byteArrayOf(2, 0, 0, 1, 2), p.copyOfRange(0, 5))
    }

    @Test fun kindIdsMatchSwift() {
        assertEquals(listOf(0, 1, 2, 3, 4, 5), BroadcastPacket.Kind.values().map { it.id })
        assertEquals(BroadcastPacket.Kind.CONFIG, BroadcastPacket.Kind.of(1))
        assertNull(BroadcastPacket.Kind.of(9))
    }

    @Test fun configJsonHasSortedKeysAndTypes() {
        val p = BroadcastPacket.config(720, 1600, 2.0, 0)
        assertEquals(1, p[0].toInt())
        val body = String(p, 5, p.size - 5, Charsets.UTF_8)
        assertEquals("""{"height":1600,"rotation":0,"scale":2,"width":720}""", body)
    }

    @Test fun endedAndStatusJson() {
        assertEquals("""{"reason":"stopped"}""", String(BroadcastPacket.ended("stopped").copyOfRange(5, 5 + 20), Charsets.UTF_8))
        val s = BroadcastPacket.status(true)
        assertEquals(0, s[0].toInt())
        assertEquals(true, JSONObject(String(s, 5, s.size - 5, Charsets.UTF_8)).getBoolean("broadcasting"))
    }

    @Test fun largeLengthUsesAllFourBytes() {
        val p = BroadcastPacket.encode(BroadcastPacket.Kind.FRAME, ByteArray(0x010203))
        assertEquals(0, p[1].toInt()); assertEquals(0x01, p[2].toInt()); assertEquals(0x02, p[3].toInt()); assertEquals(0x03, p[4].toInt())
    }

    @Test fun decodeAllRoundTripsAndRejectsTruncation() {
        val bytes = BroadcastPacket.config(1, 2, 1.0, 90) + BroadcastPacket.frame(byteArrayOf(0, 0, 0, 1, 9)) + BroadcastPacket.ended("x")
        val all = BroadcastPacket.decodeAll(bytes)!!
        assertEquals(listOf(BroadcastPacket.Kind.CONFIG, BroadcastPacket.Kind.FRAME, BroadcastPacket.Kind.ENDED), all.map { it.kind })
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 9), all[1].payload)
        assertNull(BroadcastPacket.decodeAll(bytes.copyOf(bytes.size - 1)))
    }
}
