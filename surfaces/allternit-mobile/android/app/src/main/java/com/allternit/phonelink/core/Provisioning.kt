package com.allternit.phonelink.core

/** Persisted app settings (SharedPreferences on device, a map in tests). */
interface KeyValueStore {
    fun getString(key: String): String?
    fun getBoolean(key: String, default: Boolean): Boolean
    fun getInt(key: String, default: Int): Int
    fun getLong(key: String, default: Long): Long
    fun edit(block: KeyValueStore.Editor.() -> Unit)

    interface Editor {
        fun putString(key: String, value: String?)
        fun putBoolean(key: String, value: Boolean)
        fun putInt(key: String, value: Int)
        fun putLong(key: String, value: Long)
    }
}

class MemoryKeyValueStore : KeyValueStore {
    val map = HashMap<String, Any?>()
    override fun getString(key: String) = map[key] as? String
    override fun getBoolean(key: String, default: Boolean) = map[key] as? Boolean ?: default
    override fun getInt(key: String, default: Int) = map[key] as? Int ?: default
    override fun getLong(key: String, default: Long) = map[key] as? Long ?: default
    override fun edit(block: KeyValueStore.Editor.() -> Unit) {
        val e = object : KeyValueStore.Editor {
            override fun putString(key: String, value: String?) { map[key] = value }
            override fun putBoolean(key: String, value: Boolean) { map[key] = value }
            override fun putInt(key: String, value: Int) { map[key] = value }
            override fun putLong(key: String, value: Long) { map[key] = value }
        }
        e.block()
    }
}

/**
 * Typed view over the settings. Defaults: loopback only until provisioned,
 * LAN after provisioning (the PROVISION broadcast turns it on unless told
 * otherwise), port 8200.
 */
class AppSettings(private val kv: KeyValueStore, private val defaultName: () -> String, private val newId: () -> String) {
    companion object {
        const val DEFAULT_PORT = 8200
        /** An armed one-shot pairing grant lapses after this long. */
        const val ARM_WINDOW_MS = 10 * 60 * 1000L
        private const val K_DEVICE_ID = "device_id"
        private const val K_NAME = "device_name"
        private const val K_REMOTE = "allow_remote_access"
        private const val K_PORT = "listen_port"
        private const val K_PROVISIONED = "provisioned"
        private const val K_USER_STOPPED = "user_stopped"
        private const val K_ARMED_UNTIL = "armed_until_ms"
    }

    val deviceId: String
        @Synchronized get() = kv.getString(K_DEVICE_ID) ?: newId().also { id -> kv.edit { putString(K_DEVICE_ID, id) } }

    var deviceName: String
        get() = kv.getString(K_NAME)?.takeIf { it.isNotEmpty() } ?: defaultName()
        set(v) = kv.edit { putString(K_NAME, v.ifEmpty { null }) }

    var allowRemote: Boolean
        get() = kv.getBoolean(K_REMOTE, false)
        set(v) = kv.edit { putBoolean(K_REMOTE, v) }

    var port: Int
        get() = kv.getInt(K_PORT, DEFAULT_PORT)
        set(v) = kv.edit { putInt(K_PORT, v) }

    var provisioned: Boolean
        get() = kv.getBoolean(K_PROVISIONED, false)
        set(v) = kv.edit { putBoolean(K_PROVISIONED, v) }

    /** Set by the notification's Stop (or config stop_server); cleared by Start / re-provisioning. */
    var userStopped: Boolean
        get() = kv.getBoolean(K_USER_STOPPED, false)
        set(v) = kv.edit { putBoolean(K_USER_STOPPED, v) }

    fun armPairing(nowMs: Long) = kv.edit { putLong(K_ARMED_UNTIL, nowMs + ARM_WINDOW_MS) }

    fun isArmed(nowMs: Long): Boolean = kv.getLong(K_ARMED_UNTIL, 0) > nowMs

    /**
     * Consumes the one-shot grant. Only a loopback request (adb forward, or an
     * app on the phone) may use it, so a LAN neighbour cannot race Desktop for it.
     */
    @Synchronized
    fun consumeArmedGrant(local: Boolean, nowMs: Long): Boolean {
        if (!local || !isArmed(nowMs)) return false
        kv.edit { putLong(K_ARMED_UNTIL, 0) }
        return true
    }

    fun serverConfig() = ServerConfig(deviceId, deviceName, allowRemote, port)
}

/** The PROVISION broadcast's extras, validated. */
data class ProvisionCommand(
    val armPairing: Boolean,
    val allowRemote: Boolean,
    val deviceName: String?,
    val port: Int?,
) {
    companion object {
        const val ACTION = "com.allternit.phonelink.PROVISION"
        const val EXTRA_ARM = "arm_pairing"
        const val EXTRA_REMOTE = "allow_remote_access"
        const val EXTRA_NAME = "device_name"
        const val EXTRA_PORT = "listen_port"

        /** @param extras the intent extras as plain values (am broadcast --ez/--es/--ei). */
        fun parse(extras: Map<String, Any?>): Result<ProvisionCommand> {
            val arm = extras[EXTRA_ARM].asBool() ?: true
            val remote = extras[EXTRA_REMOTE].asBool() ?: true
            val name = (extras[EXTRA_NAME] as? String)?.trim()
            if (name != null && name.toByteArray(Charsets.UTF_8).size > 63) {
                return Result.failure(IllegalArgumentException("device_name is at most 63 bytes"))
            }
            val port = when (val p = extras[EXTRA_PORT]) {
                null -> null
                is Number -> p.toInt()
                is String -> p.toIntOrNull() ?: return Result.failure(IllegalArgumentException("listen_port must be a number"))
                else -> return Result.failure(IllegalArgumentException("listen_port must be a number"))
            }
            if (port != null && port !in 1024..65535) return Result.failure(IllegalArgumentException("listen_port must be 1024..65535"))
            return Result.success(ProvisionCommand(arm, remote, name?.ifEmpty { null }, port))
        }

        private fun Any?.asBool(): Boolean? = when (this) {
            is Boolean -> this
            is String -> when (lowercase()) { "true", "1", "yes" -> true; "false", "0", "no" -> false; else -> null }
            is Number -> toInt() != 0
            else -> null
        }
    }

    fun applyTo(settings: AppSettings, nowMs: Long) {
        // Pin the device id now: Desktop keys its pairing on it, and a lazily
        // created id would come back different after the next restart.
        settings.deviceId
        settings.allowRemote = allowRemote
        deviceName?.let { settings.deviceName = it }
        port?.let { settings.port = it }
        settings.provisioned = true
        settings.userStopped = false
        if (armPairing) settings.armPairing(nowMs)
    }

    fun summary(settings: AppSettings, nowMs: Long): String =
        "provisioned device_id=${settings.deviceId} name=${settings.deviceName} " +
            "remote=${settings.allowRemote} port=${settings.port} armed=${settings.isArmed(nowMs)}"
}

/** Whether the server should come up on its own (boot, package update, accessibility bind). */
object AutoStart {
    fun shouldStart(settings: AppSettings): Boolean = settings.provisioned && !settings.userStopped
}
