package com.allternit.app.files

import android.os.HandlerThread
import android.os.ProxyFileDescriptorCallback
import android.system.ErrnoException
import android.system.OsConstants
import java.io.IOException

/**
 * A seekable read-only file descriptor over ranged GETs (`Range: bytes=a-b`, 206),
 * for files too big to download before opening (videos, large PDFs). Holds at most
 * [KEEP] blocks of [BLOCK] bytes, so a 4 GB video costs 4 MiB of memory. Runs on its
 * own HandlerThread, which it stops on release.
 */
internal class RangeReader(
    private val drive: FilesDrive,
    private val cid: String,
    private val itemId: String,
    private val size: Long,
    private val thread: HandlerThread,
) : ProxyFileDescriptorCallback() {
    private val blocks = object : LinkedHashMap<Long, ByteArray>(KEEP + 1, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Long, ByteArray>?) = size > KEEP
    }

    override fun onGetSize(): Long = size

    override fun onRead(offset: Long, size: Int, data: ByteArray): Int {
        var done = 0
        while (done < size && offset + done < this.size) {
            val pos = offset + done
            val index = pos / BLOCK
            val block = block(index)
            val inBlock = (pos - index * BLOCK).toInt()
            val n = minOf(size - done, block.size - inBlock)
            if (n <= 0) break
            System.arraycopy(block, inBlock, data, done, n)
            done += n
        }
        return done
    }

    private fun block(index: Long): ByteArray {
        blocks[index]?.let { return it }
        val from = index * BLOCK
        val to = minOf(size, from + BLOCK) - 1
        try {
            val conn = drive.client(cid).content(itemId, from, to)
            try {
                val bytes = conn.inputStream.use { input ->
                    if (conn.responseCode == 200 && from > 0) input.skip(from) // the server ignored the range
                    val out = ByteArray((to - from + 1).toInt())
                    var got = 0
                    while (got < out.size) {
                        val r = input.read(out, got, out.size - got)
                        if (r < 0) break
                        got += r
                    }
                    if (got == out.size) out else out.copyOf(got)
                }
                blocks[index] = bytes
                return bytes
            } finally {
                conn.disconnect()
            }
        } catch (e: IOException) {
            throw ErrnoException("onRead", OsConstants.EIO)
        }
    }

    override fun onRelease() {
        blocks.clear()
        thread.quitSafely()
    }

    companion object {
        private const val BLOCK = 1L shl 20
        private const val KEEP = 4
    }
}
