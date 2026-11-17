package com.allternit.app.files

/**
 * BLAKE3-256, hash and keyed-hash modes, a direct port of the BLAKE3 reference
 * implementation (github.com/BLAKE3-team/BLAKE3, reference_impl.rs, CC0/Apache-2.0).
 * Chunk and file hashes in the Files drive are the hash mode, lower-case hex (SPEC
 * section 5); replica locators are the keyed mode (SPEC section 13). Incremental:
 * [update] any number of times, then [finalizeHex].
 */
internal class Blake3 private constructor(private val key: IntArray, private val mode: Int) {
    constructor() : this(IV, 0)

    private var chunk = ChunkState(key, 0, mode)
    private val cvStack = arrayOfNulls<IntArray>(54)
    private var cvStackLen = 0

    fun update(input: ByteArray, offset: Int = 0, length: Int = input.size) {
        var off = offset
        var len = length
        while (len > 0) {
            if (chunk.len() == CHUNK_LEN) {
                val cv = chunk.output().chainingValue()
                val total = chunk.counter + 1
                addChunkCv(cv, total)
                chunk = ChunkState(key, total, mode)
            }
            val take = minOf(CHUNK_LEN - chunk.len(), len)
            chunk.update(input, off, take)
            off += take
            len -= take
        }
    }

    fun finalizeBytes(): ByteArray {
        var output = chunk.output()
        var parents = cvStackLen
        while (parents > 0) {
            parents -= 1
            output = parentOutput(key, mode, cvStack[parents]!!, output.chainingValue())
        }
        return output.rootBytes()
    }

    fun finalizeHex(): String = hex(finalizeBytes())

    private fun addChunkCv(newCv: IntArray, totalChunks: Long) {
        var cv = newCv
        var total = totalChunks
        while ((total and 1L) == 0L) {
            cvStackLen -= 1
            cv = parentOutput(key, mode, cvStack[cvStackLen]!!, cv).chainingValue()
            total = total shr 1
        }
        cvStack[cvStackLen] = cv
        cvStackLen += 1
    }

    private class Output(
        val inputCv: IntArray,
        val block: IntArray,
        val counter: Long,
        val blockLen: Int,
        val flags: Int,
    ) {
        fun chainingValue(): IntArray = compress(inputCv, block, counter, blockLen, flags).copyOf(8)

        fun rootBytes(): ByteArray {
            val words = compress(inputCv, block, 0, blockLen, flags or ROOT)
            val out = ByteArray(32)
            for (i in 0 until 8) {
                val w = words[i]
                out[4 * i] = w.toByte()
                out[4 * i + 1] = (w ushr 8).toByte()
                out[4 * i + 2] = (w ushr 16).toByte()
                out[4 * i + 3] = (w ushr 24).toByte()
            }
            return out
        }
    }

    private class ChunkState(key: IntArray, val counter: Long, private val mode: Int) {
        var cv: IntArray = key.copyOf()
        val block = ByteArray(BLOCK_LEN)
        var blockLen = 0
        var blocksCompressed = 0

        fun len(): Int = BLOCK_LEN * blocksCompressed + blockLen

        private fun startFlag(): Int = if (blocksCompressed == 0) CHUNK_START else 0

        fun update(input: ByteArray, offset: Int, length: Int) {
            var off = offset
            var len = length
            while (len > 0) {
                if (blockLen == BLOCK_LEN) {
                    cv = compress(cv, words(block), counter, BLOCK_LEN, mode or startFlag()).copyOf(8)
                    blocksCompressed += 1
                    block.fill(0)
                    blockLen = 0
                }
                val take = minOf(BLOCK_LEN - blockLen, len)
                System.arraycopy(input, off, block, blockLen, take)
                blockLen += take
                off += take
                len -= take
            }
        }

        fun output(): Output = Output(cv, words(block), counter, blockLen, mode or startFlag() or CHUNK_END)
    }

    companion object {
        private const val BLOCK_LEN = 64
        private const val CHUNK_LEN = 1024
        private const val CHUNK_START = 1
        private const val CHUNK_END = 2
        private const val PARENT = 4
        private const val ROOT = 8
        private const val KEYED_HASH = 16

        private val IV = intArrayOf(
            0x6A09E667, 0xBB67AE85.toInt(), 0x3C6EF372, 0xA54FF53A.toInt(),
            0x510E527F, 0x9B05688C.toInt(), 0x1F83D9AB, 0x5BE0CD19,
        )
        private val PERM = intArrayOf(2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)

        fun hashHex(data: ByteArray): String = Blake3().apply { update(data) }.finalizeHex()

        fun hash(data: ByteArray): ByteArray = Blake3().apply { update(data) }.finalizeBytes()

        /** Keyed-hash mode; `key` is 32 bytes. */
        fun keyed(key: ByteArray): Blake3 {
            require(key.size == 32) { "BLAKE3 keys are 32 bytes" }
            return Blake3(words(key.copyOf(BLOCK_LEN)).copyOf(8), KEYED_HASH)
        }

        fun keyedHashHex(key: ByteArray, data: ByteArray): String = keyed(key).apply { update(data) }.finalizeHex()

        fun hex(bytes: ByteArray): String {
            val sb = StringBuilder(bytes.size * 2)
            for (b in bytes) {
                val v = b.toInt() and 0xff
                sb.append("0123456789abcdef"[v ushr 4]).append("0123456789abcdef"[v and 0xf])
            }
            return sb.toString()
        }

        private fun parentOutput(key: IntArray, mode: Int, left: IntArray, right: IntArray): Output {
            val block = IntArray(16)
            System.arraycopy(left, 0, block, 0, 8)
            System.arraycopy(right, 0, block, 8, 8)
            return Output(key, block, 0, BLOCK_LEN, PARENT or mode)
        }

        private fun words(block: ByteArray): IntArray {
            val w = IntArray(16)
            for (i in 0 until 16) {
                w[i] = (block[4 * i].toInt() and 0xff) or
                    ((block[4 * i + 1].toInt() and 0xff) shl 8) or
                    ((block[4 * i + 2].toInt() and 0xff) shl 16) or
                    ((block[4 * i + 3].toInt() and 0xff) shl 24)
            }
            return w
        }

        private fun g(s: IntArray, a: Int, b: Int, c: Int, d: Int, mx: Int, my: Int) {
            s[a] = s[a] + s[b] + mx
            s[d] = (s[d] xor s[a]).rotateRight(16)
            s[c] = s[c] + s[d]
            s[b] = (s[b] xor s[c]).rotateRight(12)
            s[a] = s[a] + s[b] + my
            s[d] = (s[d] xor s[a]).rotateRight(8)
            s[c] = s[c] + s[d]
            s[b] = (s[b] xor s[c]).rotateRight(7)
        }

        private fun round(s: IntArray, m: IntArray) {
            g(s, 0, 4, 8, 12, m[0], m[1])
            g(s, 1, 5, 9, 13, m[2], m[3])
            g(s, 2, 6, 10, 14, m[4], m[5])
            g(s, 3, 7, 11, 15, m[6], m[7])
            g(s, 0, 5, 10, 15, m[8], m[9])
            g(s, 1, 6, 11, 12, m[10], m[11])
            g(s, 2, 7, 8, 13, m[12], m[13])
            g(s, 3, 4, 9, 14, m[14], m[15])
        }

        private fun compress(cv: IntArray, block: IntArray, counter: Long, blockLen: Int, flags: Int): IntArray {
            val s = intArrayOf(
                cv[0], cv[1], cv[2], cv[3], cv[4], cv[5], cv[6], cv[7],
                IV[0], IV[1], IV[2], IV[3],
                counter.toInt(), (counter ushr 32).toInt(), blockLen, flags,
            )
            var m = block.copyOf()
            for (r in 0 until 7) {
                round(s, m)
                if (r < 6) m = IntArray(16) { m[PERM[it]] }
            }
            for (i in 0 until 8) {
                s[i] = s[i] xor s[i + 8]
                s[i + 8] = s[i + 8] xor cv[i]
            }
            return s
        }
    }
}
