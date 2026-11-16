package com.allternit.app.files

import com.allternit.files.contract.ChunkRef
import com.allternit.files.contract.ChunkerParams
import java.io.InputStream

/**
 * The pinned v1 chunker (SPEC section 5): FastCDC 2020 exactly as the `fastcdc`
 * crate 3.2.1 `v2020` module cuts with normalization level 1, plus blake3-256.
 * Every client must cut the same boundaries or dedup and replica checks miss, so
 * this follows the step list in contracts/files-drive/vectors/chunker-v1.json word
 * for word; ChunkerVectorsTest runs every case in that file.
 *
 * Streaming: at most one max-size chunk (4 MiB) is held in memory at a time.
 */
internal object Chunker {
    const val MIN = 262_144
    const val AVG = 1_048_576
    const val MAX = 4_194_304
    private val MASK_S = java.lang.Long.parseUnsignedLong("0000d91767537000", 16)
    private val MASK_L = java.lang.Long.parseUnsignedLong("0000d91707537000", 16)
    private val MASK_S_LS = MASK_S shl 1
    private val MASK_L_LS = MASK_L shl 1

    val PARAMS = ChunkerParams(
        algo = "fastcdc-2020",
        hash = "blake3",
        minBytes = MIN.toLong(),
        avgBytes = AVG.toLong(),
        maxBytes = MAX.toLong(),
    )

    /** Length of the next chunk cut from `src[start, start + available)`. */
    fun cut(src: ByteArray, start: Int, available: Int): Int {
        if (available <= MIN) return available
        var n = available
        var center = AVG
        if (n > MAX) n = MAX else if (n < center) center = n
        var i = MIN / 2
        var h = 0L
        val gear = Gear.GEAR
        val gearLs = Gear.GEAR_LS
        while (i < center / 2) {
            val a = 2 * i
            h = (h shl 2) + gearLs[src[start + a].toInt() and 0xff]
            if ((h and MASK_S_LS) == 0L) return a
            h += gear[src[start + a + 1].toInt() and 0xff]
            if ((h and MASK_S) == 0L) return a + 1
            i += 1
        }
        while (i < n / 2) {
            val a = 2 * i
            h = (h shl 2) + gearLs[src[start + a].toInt() and 0xff]
            if ((h and MASK_L_LS) == 0L) return a
            h += gear[src[start + a + 1].toInt() and 0xff]
            if ((h and MASK_L) == 0L) return a + 1
            i += 1
        }
        return n
    }

    class Result(val chunks: List<ChunkRef>, val size: Long, val contentHash: String)

    /**
     * Chunk a stream. [each] sees every chunk's bytes (`buf[0, len)`, valid only
     * during the call) with its ref, in order. Returns the refs and the whole-file hash.
     */
    fun chunk(input: InputStream, each: ((ChunkRef, ByteArray, Int) -> Unit)? = null): Result {
        val buf = ByteArray(MAX)
        var filled = 0
        var eof = false
        var offset = 0L
        val whole = Blake3()
        val refs = ArrayList<ChunkRef>()
        while (true) {
            while (!eof && filled < MAX) {
                val r = input.read(buf, filled, MAX - filled)
                if (r < 0) eof = true else filled += r
            }
            if (filled == 0) break
            val len = cut(buf, 0, filled)
            whole.update(buf, 0, len)
            val hash = Blake3().apply { update(buf, 0, len) }.finalizeHex()
            val ref = ChunkRef(hash = hash, offset = offset, size = len.toLong())
            refs.add(ref)
            each?.invoke(ref, buf, len)
            offset += len
            System.arraycopy(buf, len, buf, 0, filled - len)
            filled -= len
        }
        return Result(refs, offset, whole.finalizeHex())
    }
}
