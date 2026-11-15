package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MeshKeyBoxTest {
    private val kv = MemoryKeyValueStore()
    private val box = MeshKeyBox(kv, AesGcmTestSealer())
    private val KEY = "hskey-1234567890abcdef"

    @Test fun storeReadClearRoundTrips() {
        box.store(KEY)
        assertTrue(box.hasKey())
        assertEquals(KEY, box.read())
        // read() does not consume; clear() does.
        assertEquals(KEY, box.read())
        box.clear()
        assertFalse(box.hasKey())
        assertNull(box.read())
    }

    @Test fun sealedKeyIsNeverInTheClear() {
        box.store(KEY)
        val raw = kv.map["mesh_auth_key_sealed"] as String
        assertFalse("key material must not be stored plaintext", raw.contains(KEY))
        // And it round-trips through the sealer.
        assertEquals(KEY, box.read())
    }

    @Test fun unopenableBlobReadsAsNull() {
        box.store(KEY)
        // A different key (e.g. the Keystore entry is gone after a restore) cannot open it.
        val afterRestore = MeshKeyBox(kv, AesGcmTestSealer())
        assertNull(afterRestore.read())
    }
}
