package com.allternit.core.rfb

import android.graphics.Bitmap
import java.util.zip.DataFormatException
import java.util.zip.Inflater

/*
 * Framebuffer + rectangle decoders (iOS RFBDecoder.swift): Raw, CopyRect and ZRLE into an ARGB
 * pixel array, snapshotted to a Bitmap after each FramebufferUpdate batch.
 *
 * ZRLE's zlib stream (RFC 1950 wrapper) is PERSISTENT across rectangles: each rectangle's payload
 * is the next segment of one stream, so one [Inflater] lives for the whole connection and input it
 * has not consumed yet carries into the next rectangle.
 */

/** One persistent zlib inflate stream shared by every ZRLE rectangle of the connection. */
class ZrleInflater {
    private val inflater = Inflater(false)
    private var pending = ByteArray(0)

    fun feed(compressed: ByteArray) {
        pending = if (pending.isEmpty()) compressed else pending + compressed
    }

    /** Inflates exactly [count] bytes or throws (never renders a partial tile). */
    fun read(count: Int): ByteArray {
        val out = ByteArray(count)
        if (count == 0) return out
        var produced = 0
        while (produced < count) {
            if (inflater.needsInput()) {
                if (pending.isEmpty()) throw RfbException.ZrleUnderflow()
                inflater.setInput(pending)
                pending = ByteArray(0)
            }
            val n = try {
                inflater.inflate(out, produced, count - produced)
            } catch (e: DataFormatException) {
                throw RfbException.ZrleInflateFailed(e.message ?: "bad data")
            }
            produced += n
            if (n == 0) {
                if (inflater.needsDictionary() || inflater.finished()) throw RfbException.ZrleInflateFailed("stream ended")
                if (inflater.needsInput() && pending.isEmpty()) throw RfbException.ZrleUnderflow()
            }
        }
        return out
    }

    fun end() = inflater.end()
}

/** The negotiated framebuffer as ARGB_8888 ints. */
class RfbFramebuffer(val width: Int, val height: Int) {
    private val pixels: IntArray
    private val inflater = ZrleInflater()

    init {
        // 8K ceiling protects the allocation from a garbage ServerInit.
        if (width <= 0 || height <= 0 || width > 8192 || height > 8192) throw RfbException.InvalidFramebufferSize(width, height)
        pixels = IntArray(width * height) { 0xFF000000.toInt() }
    }

    /** A copy of the current buffer: later rectangles never change a published frame. */
    fun makeBitmap(): Bitmap = Bitmap.createBitmap(pixels, width, height, Bitmap.Config.ARGB_8888)

    fun release() = inflater.end()

    fun apply(rect: RfbRectangle) {
        val x = rect.x
        val y = rect.y
        val w = rect.width
        val h = rect.height
        if (x < 0 || y < 0 || x + w > width || y + h > height) throw RfbException.RectangleOutOfBounds()
        if (w == 0 || h == 0) return
        when (rect.encoding) {
            RfbEncoding.RAW -> applyRaw(rect.payload, x, y, w, h)
            RfbEncoding.COPY_RECT -> applyCopyRect(rect.payload, x, y, w, h)
            RfbEncoding.ZRLE -> applyZrle(rect.payload, x, y, w, h)
            else -> throw RfbException.UnsupportedEncoding(rect.encoding)
        }
    }

    /** Raw pixels as [B, G, R, pad]. */
    private fun applyRaw(p: ByteArray, x: Int, y: Int, w: Int, h: Int) {
        if (p.size != w * h * 4) throw RfbException.MalformedRectanglePayload()
        var i = 0
        for (row in 0 until h) {
            var o = (y + row) * width + x
            for (col in 0 until w) {
                pixels[o++] = argb(p[i + 2], p[i + 1], p[i])
                i += 4
            }
        }
    }

    /** src-x/src-y, moved within the buffer; overlapping moves are staged through a copy. */
    private fun applyCopyRect(p: ByteArray, x: Int, y: Int, w: Int, h: Int) {
        if (p.size != 4) throw RfbException.MalformedRectanglePayload()
        val sx = ((p[0].toInt() and 0xFF) shl 8) or (p[1].toInt() and 0xFF)
        val sy = ((p[2].toInt() and 0xFF) shl 8) or (p[3].toInt() and 0xFF)
        if (sx + w > width || sy + h > height) throw RfbException.RectangleOutOfBounds()
        val staging = IntArray(w * h)
        for (row in 0 until h) System.arraycopy(pixels, (sy + row) * width + sx, staging, row * w, w)
        for (row in 0 until h) System.arraycopy(staging, row * w, pixels, (y + row) * width + x, w)
    }

    /** ≤64×64 tiles in reading order. */
    private fun applyZrle(p: ByteArray, x: Int, y: Int, w: Int, h: Int) {
        inflater.feed(p)
        var ty = 0
        while (ty < h) {
            val th = minOf(64, h - ty)
            var tx = 0
            while (tx < w) {
                val tw = minOf(64, w - tx)
                decodeTile(x + tx, y + ty, tw, th)
                tx += tw
            }
            ty += th
        }
    }

    private fun decodeTile(x: Int, y: Int, w: Int, h: Int) {
        val sub = inflater.read(1)[0].toInt() and 0xFF
        when (sub) {
            0 -> {
                val b = inflater.read(w * h * 3)
                var c = 0
                for (row in 0 until h) for (col in 0 until w) {
                    pixels[(y + row) * width + x + col] = argb(b[c + 2], b[c + 1], b[c])
                    c += 3
                }
            }
            1 -> {
                val b = inflater.read(3)
                val color = argb(b[2], b[1], b[0])
                for (row in 0 until h) java.util.Arrays.fill(pixels, (y + row) * width + x, (y + row) * width + x + w, color)
            }
            in 2..16 -> {
                val palette = readPalette(sub)
                val bits = if (sub <= 2) 1 else if (sub <= 4) 2 else 4
                val rowBytes = (w * bits + 7) / 8
                val packed = inflater.read(rowBytes * h)
                val mask = (1 shl bits) - 1
                for (row in 0 until h) for (col in 0 until w) {
                    val bitOffset = row * rowBytes * 8 + col * bits
                    val byte = packed[bitOffset / 8].toInt() and 0xFF
                    val shift = 8 - bits - (bitOffset % 8)
                    val index = (byte shr shift) and mask
                    if (index >= palette.size) throw RfbException.MalformedZrleTile()
                    pixels[(y + row) * width + x + col] = palette[index]
                }
            }
            128 -> {
                var pixel = 0
                val total = w * h
                while (pixel < total) {
                    val b = inflater.read(3)
                    val color = argb(b[2], b[1], b[0])
                    val run = readRunLength()
                    if (pixel + run > total) throw RfbException.MalformedZrleTile()
                    repeat(run) {
                        pixels[(y + pixel / w) * width + x + pixel % w] = color
                        pixel++
                    }
                }
            }
            129 -> {
                val count = inflater.read(1)[0].toInt() and 0xFF
                val palette = readPalette(count)
                var pixel = 0
                val total = w * h
                while (pixel < total) {
                    val head = inflater.read(1)[0].toInt() and 0xFF
                    val index = head and 0x7F
                    if (index >= palette.size) throw RfbException.MalformedZrleTile()
                    val run = if (head and 0x80 != 0) readRunLength() else 1
                    if (pixel + run > total) throw RfbException.MalformedZrleTile()
                    repeat(run) {
                        pixels[(y + pixel / w) * width + x + pixel % w] = palette[index]
                        pixel++
                    }
                }
            }
            else -> throw RfbException.InvalidZrleSubencoding(sub)
        }
    }

    /** Run length: bytes sum, and 255 means the run continues into the next byte. */
    private fun readRunLength(): Int {
        var length = 0
        while (true) {
            val b = inflater.read(1)[0].toInt() and 0xFF
            length += b
            if (b != 255) return length
        }
    }

    private fun readPalette(count: Int): IntArray {
        if (count <= 0) throw RfbException.MalformedZrleTile()
        val b = inflater.read(count * 3)
        return IntArray(count) { argb(b[it * 3 + 2], b[it * 3 + 1], b[it * 3]) }
    }

    private fun argb(r: Byte, g: Byte, b: Byte): Int =
        (0xFF shl 24) or ((r.toInt() and 0xFF) shl 16) or ((g.toInt() and 0xFF) shl 8) or (b.toInt() and 0xFF)
}
