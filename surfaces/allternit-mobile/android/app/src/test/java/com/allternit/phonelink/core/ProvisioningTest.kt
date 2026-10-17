package com.allternit.phonelink.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.spec.GCMParameterSpec

class ProvisioningTest {
    @get:Rule val tmp = TemporaryFolder()

    private val kv = MemoryKeyValueStore()
    private val settings = AppSettings(kv, defaultName = { "motorola moto g play - 2024" }, newId = { "id-1" })
    private val now = 1_000_000L

    @Test fun defaultsBeforeProvisioning() {
        assertFalse(settings.allowRemote)
        assertEquals(8200, settings.port)
        assertFalse(AutoStart.shouldStart(settings))
        assertEquals("id-1", settings.deviceId)
        assertEquals("id-1", settings.deviceId)
    }

    @Test fun provisionDefaultsToLanAndArmsOnce() {
        val cmd = ProvisionCommand.parse(emptyMap()).getOrThrow()
        cmd.applyTo(settings, now)
        assertTrue(settings.allowRemote)
        assertTrue(settings.provisioned)
        assertTrue(AutoStart.shouldStart(settings))
        assertFalse("LAN requests may not use the cable grant", settings.consumeArmedGrant(local = false, nowMs = now))
        assertTrue(settings.consumeArmedGrant(local = true, nowMs = now))
        assertFalse(settings.consumeArmedGrant(local = true, nowMs = now))
    }

    @Test fun armedGrantLapses() {
        ProvisionCommand.parse(mapOf("arm_pairing" to true)).getOrThrow().applyTo(settings, now)
        assertFalse(settings.consumeArmedGrant(true, now + AppSettings.ARM_WINDOW_MS + 1))
    }

    @Test fun parsesAmBroadcastExtras() {
        val cmd = ProvisionCommand.parse(mapOf(
            "arm_pairing" to false, "allow_remote_access" to "false", "device_name" to " Joe's moto ", "listen_port" to 8300,
        )).getOrThrow()
        cmd.applyTo(settings, now)
        assertFalse(settings.allowRemote)
        assertEquals("Joe's moto", settings.deviceName)
        assertEquals(8300, settings.port)
        assertFalse(settings.isArmed(now))
        assertTrue(ProvisionCommand.parse(mapOf("listen_port" to 80)).isFailure)
        assertTrue(ProvisionCommand.parse(mapOf("listen_port" to "abc")).isFailure)
        assertTrue(ProvisionCommand.parse(mapOf("device_name" to "x".repeat(64))).isFailure)
    }

    @Test fun meshExtrasAreOptionalAndValidated() {
        val none = ProvisionCommand.parse(emptyMap()).getOrThrow()
        assertEquals(null, none.meshControlUrl)
        assertEquals(null, none.meshAuthKey)

        val full = ProvisionCommand.parse(mapOf(
            "mesh_control_url" to "https://mail.news.allternit.com:8444",
            "mesh_auth_key" to "abcdef0123",
        )).getOrThrow()
        assertEquals("https://mail.news.allternit.com:8444", full.meshControlUrl)
        assertEquals("abcdef0123", full.meshAuthKey)

        // https only: the pre-auth key must never travel in the clear.
        for (bad in listOf("http://mail.news.allternit.com:8444", "ftp://x", "gopher://x", "mailto:a@b")) {
            assertTrue("$bad must be rejected", ProvisionCommand.parse(mapOf("mesh_control_url" to bad)).isFailure)
        }
        assertTrue(ProvisionCommand.parse(mapOf("mesh_control_url" to "  ")).isFailure)
        assertTrue(ProvisionCommand.parse(mapOf("mesh_auth_key" to "   ")).isFailure)
        assertTrue(ProvisionCommand.parse(mapOf("mesh_auth_key" to "k".repeat(4097))).isFailure)
    }

    @Test fun meshControlUrlPersistsThroughApplyTo() {
        val cmd = ProvisionCommand.parse(mapOf("mesh_control_url" to "https://hs.example:8444")).getOrThrow()
        cmd.applyTo(settings, now)
        assertEquals("https://hs.example:8444", settings.meshControlUrl)
        // The auth key itself is NOT stored by applyTo; the receiver seals it via MeshKeyBox.
        assertFalse(settings.serverConfig().toString().contains("secret"))
    }

    @Test fun userStopSurvivesRebootUntilReprovisioned() {
        ProvisionCommand.parse(emptyMap()).getOrThrow().applyTo(settings, now)
        settings.userStopped = true
        // A reboot builds new settings over the same store.
        val afterReboot = AppSettings(kv, { "x" }, { "other" })
        assertFalse(AutoStart.shouldStart(afterReboot))
        assertEquals("id-1", afterReboot.deviceId)
        ProvisionCommand.parse(emptyMap()).getOrThrow().applyTo(afterReboot, now)
        assertTrue(AutoStart.shouldStart(afterReboot))
    }

    @Test fun emptyDeviceNameRestoresDefault() {
        settings.deviceName = "Custom"
        settings.deviceName = ""
        assertEquals("motorola moto g play - 2024", settings.deviceName)
    }

    /** Software AES-GCM standing in for the Keystore-backed sealer. */
    private class TestSealer : BlobSealer {
        private val key = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
        override fun seal(plain: ByteArray): ByteArray {
            val c = Cipher.getInstance("AES/GCM/NoPadding"); c.init(Cipher.ENCRYPT_MODE, key)
            return c.iv + c.doFinal(plain)
        }
        override fun open(sealed: ByteArray): ByteArray {
            val c = Cipher.getInstance("AES/GCM/NoPadding")
            c.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, sealed.copyOfRange(0, 12)))
            return c.doFinal(sealed, 12, sealed.size - 12)
        }
    }

    @Test fun sessionStoreIsEncryptedAndRoundTrips() {
        val f = File(tmp.root, "sessions.bin")
        val store = EncryptedFileSessionStore(f, TestSealer())
        val s = AuthSession("630dcd29", "ab".repeat(32), "Desk", 1, 2, "10.0.0.2", "lan")
        store.save(listOf(s))
        assertFalse("key material is not on disk in the clear", String(f.readBytes(), Charsets.ISO_8859_1).contains("ab".repeat(32)))
        assertEquals(listOf(s), store.load())
    }

    @Test fun unreadableStoreLoadsEmpty() {
        val f = File(tmp.root, "sessions.bin")
        EncryptedFileSessionStore(f, TestSealer()).save(listOf(AuthSession("630dcd29", "ab".repeat(32), "Desk", 1, 2)))
        // A different key (e.g. the Keystore entry is gone after a restore) cannot open it.
        assertTrue(EncryptedFileSessionStore(f, TestSealer()).load().isEmpty())
    }
}
