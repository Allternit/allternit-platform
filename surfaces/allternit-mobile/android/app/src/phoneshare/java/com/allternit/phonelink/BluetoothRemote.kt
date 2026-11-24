package com.allternit.phonelink

import android.Manifest
import android.annotation.SuppressLint
import android.annotation.TargetApi
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothClass
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothHidDevice
import android.bluetooth.BluetoothHidDeviceAppSdpSettings
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.allternit.phonelink.core.HidAvailability
import com.allternit.phonelink.core.HidCommands
import com.allternit.phonelink.core.HidLink
import com.allternit.phonelink.core.HidReports
import com.allternit.phonelink.core.HidState
import java.util.concurrent.CopyOnWriteArraySet

/**
 * Phone Use route `bt-android` (docs/contracts/CONTRACT-phone-use-2026-11-16.md §1, §6, §9.2):
 * this phone as a Bluetooth keyboard and mouse for the person's own iPhone,
 * through Android's public HID device profile (BluetoothHidDevice, Android 9+).
 * Our own code (contract §10).
 *
 * The person turns it on in Share screen › Control my iPhone by Bluetooth
 * (BluetoothRemoteService keeps it registered while the app is in the
 * background), pairs once from the iPhone's Settings › Bluetooth, and turns on
 * AssistiveTouch there so a pointer shows. Allternit Desktop then sends `hid.*`
 * over the app-phone control socket (HidCommands); every bot action has
 * already passed the approval step on Desktop.
 *
 * Nothing here scans for devices: the iPhone starts the pairing while this
 * phone is discoverable, so BLUETOOTH_SCAN is not needed.
 */
@SuppressLint("MissingPermission") // every Bluetooth call is behind hasPermissions()
object BluetoothRemote : HidLink {
    private const val TAG = "BluetoothRemote"
    private const val PREFS = "bluetooth_remote"
    private const val KEY_TARGET = "target_address"
    /** A phone that says it has the profile but never answers the registration is treated as not supporting it. */
    private const val REGISTER_TIMEOUT_MS = 8_000L

    @Volatile private var app: Context? = null
    @Volatile private var hid: BluetoothHidDevice? = null
    @Volatile private var registered = false
    @Volatile private var starting = false
    /** Set when getProfileProxy or registerApp failed or timed out on this phone. */
    @Volatile private var unsupported = false
    @Volatile private var connected: BluetoothDevice? = null
    private val main = Handler(Looper.getMainLooper())
    private val listeners = CopyOnWriteArraySet<() -> Unit>()

    /** The `hid.*` handler AppPhoneLink calls. */
    val commands = HidCommands(this)

    fun init(context: Context) { if (app == null) app = context.applicationContext }

    fun addListener(l: () -> Unit) { listeners.add(l) }
    fun removeListener(l: () -> Unit) { listeners.remove(l) }
    private fun changed() { listeners.forEach { try { it() } catch (_: Exception) {} } }

    // ── What the screen and the bot see ─────────────────────────────────────

    /** Runtime permissions this needs on this Android version (Nearby devices on 12+; install-time before). */
    fun neededPermissions(): Array<String> =
        if (Build.VERSION.SDK_INT >= 31) arrayOf(Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_ADVERTISE) else emptyArray()

    fun hasPermissions(context: Context? = app): Boolean {
        val c = context ?: return false
        return neededPermissions().all { c.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
    }

    private fun adapter(): BluetoothAdapter? = app?.getSystemService(BluetoothManager::class.java)?.adapter

    /** The device is too old or has no Bluetooth at all. */
    fun deviceUnsupported(): Boolean = Build.VERSION.SDK_INT < 28 || adapter() == null ||
        app?.packageManager?.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH) == false

    val isOn: Boolean get() = registered || starting

    override fun state(): HidState {
        val a = adapter()
        val availability = when {
            deviceUnsupported() || unsupported -> HidAvailability.UNSUPPORTED
            !hasPermissions() -> HidAvailability.NO_PERMISSION
            a?.isEnabled != true -> HidAvailability.BLUETOOTH_OFF
            !registered -> HidAvailability.OFF
            else -> HidAvailability.READY
        }
        val target = targetDevice()
        val c = connected
        return HidState(
            availability = availability,
            paired = target != null && target.bondState == BluetoothDevice.BOND_BONDED,
            connected = availability == HidAvailability.READY && c != null,
            name = (c ?: target)?.let { safeName(it) },
        )
    }

    /** The phone's own Bluetooth name: what the iPhone lists under Other Devices. */
    fun ownName(): String? = try { if (hasPermissions()) adapter()?.name else null } catch (_: SecurityException) { null }

    /** Bonded devices, phones first (the iPhone the person paired before, or now). */
    fun bondedDevices(): List<BluetoothDevice> {
        if (!hasPermissions()) return emptyList()
        val all = try { adapter()?.bondedDevices?.toList().orEmpty() } catch (_: SecurityException) { emptyList() }
        return all.sortedWith(compareBy<BluetoothDevice>({ it.bluetoothClass?.majorDeviceClass != BluetoothClass.Device.Major.PHONE }, { safeName(it) ?: it.address }))
    }

    fun connectedDevice(): BluetoothDevice? = connected

    fun safeName(d: BluetoothDevice): String? = try { d.name } catch (_: SecurityException) { null }

    // ── HidLink ─────────────────────────────────────────────────────────────

    override fun send(reportId: Int, data: ByteArray): Boolean {
        if (Build.VERSION.SDK_INT < 28) return false
        val h = hid ?: return false
        val d = connected ?: return false
        return try { h.sendReport(d, reportId, data) } catch (e: Exception) { Log.w(TAG, "sendReport: ${e.javaClass.simpleName}"); false }
    }

    // ── On / off (called by BluetoothRemoteService) ─────────────────────────

    /** Registers this phone as a keyboard and mouse. Idempotent. Result arrives through the listeners. */
    fun start(context: Context) {
        init(context)
        if (registered || starting) return
        if (Build.VERSION.SDK_INT < 28 || deviceUnsupported()) { unsupported = true; changed(); return }
        if (!hasPermissions() || adapter()?.isEnabled != true) { changed(); return }
        startProxy()
    }

    @TargetApi(28)
    private fun startProxy() {
        val a = adapter() ?: return
        val ctx = app ?: return
        unsupported = false
        starting = true
        val ok = try {
            a.getProfileProxy(ctx, object : BluetoothProfile.ServiceListener {
                override fun onServiceConnected(profile: Int, proxy: BluetoothProfile) {
                    if (profile != BluetoothProfile.HID_DEVICE) return
                    hid = proxy as BluetoothHidDevice
                    register()
                }
                override fun onServiceDisconnected(profile: Int) {
                    if (profile != BluetoothProfile.HID_DEVICE) return
                    lost()
                }
            }, BluetoothProfile.HID_DEVICE)
        } catch (e: Exception) { Log.w(TAG, "getProfileProxy: ${e.javaClass.simpleName}"); false }
        if (!ok) { notSupported(); return }
        main.postDelayed(registerTimeout, REGISTER_TIMEOUT_MS)
    }

    private val registerTimeout = Runnable { if (starting && !registered) notSupported() }

    private fun notSupported() {
        Log.i(TAG, "HID device profile not available on this phone")
        unsupported = true
        stop()
    }

    @TargetApi(28)
    private fun register() {
        val h = hid ?: return
        val sdp = BluetoothHidDeviceAppSdpSettings(
            "Allternit Keyboard and Mouse",
            "Keyboard and mouse for your iPhone",
            "Allternit",
            BluetoothHidDevice.SUBCLASS1_COMBO,
            HidReports.DESCRIPTOR,
        )
        val cb = HidCallback()
        val ok = try { h.registerApp(sdp, null, null, (app ?: return).mainExecutor, cb) } catch (e: Exception) {
            Log.w(TAG, "registerApp: ${e.javaClass.simpleName}"); false
        }
        if (!ok) notSupported()
    }

    /** Android's answers about the registration and the iPhone's connection (main thread). */
    @TargetApi(28)
    private class HidCallback : BluetoothHidDevice.Callback() {
        override fun onAppStatusChanged(pluggedDevice: BluetoothDevice?, registered: Boolean) {
            main.removeCallbacks(registerTimeout)
            starting = false
            BluetoothRemote.registered = registered
            if (registered) {
                // Reconnect to the iPhone the person chose (the iPhone may also connect by itself).
                val target = targetDevice() ?: pluggedDevice
                if (target != null && connected == null) connect(target)
            } else {
                connected = null
                commands.releaseAll()
            }
            changed()
        }

        override fun onConnectionStateChanged(device: BluetoothDevice, state: Int) {
            when (state) {
                BluetoothProfile.STATE_CONNECTED -> {
                    connected = device
                    // Pairing came from the iPhone: remember it as the one to control.
                    saveTarget(device.address)
                }
                BluetoothProfile.STATE_DISCONNECTED -> if (connected?.address == device.address) {
                    connected = null
                    commands.releaseAll()
                }
            }
            changed()
        }

        override fun onGetReport(device: BluetoothDevice, type: Byte, id: Byte, bufferSize: Int) {
            // The host asks for the current input report: send an all-released one.
            val data = if (id.toInt() == HidReports.MOUSE_ID) HidReports.mouse(0, 0, 0, 0) else HidReports.keyboard(0, emptyList())
            try { hid?.replyReport(device, type, id, data) } catch (_: Exception) {}
        }

        override fun onSetReport(device: BluetoothDevice, type: Byte, id: Byte, data: ByteArray) {
            // Keyboard LEDs (caps lock …): nothing to do, just acknowledge.
            try { hid?.reportError(device, BluetoothHidDevice.ERROR_RSP_SUCCESS) } catch (_: Exception) {}
        }

        override fun onVirtualCableUnplug(device: BluetoothDevice) {
            if (connected?.address == device.address) { connected = null; commands.releaseAll() }
            changed()
        }
    }

    /** Connect to a bonded iPhone and remember it as the one to control. */
    fun connect(device: BluetoothDevice): Boolean {
        if (Build.VERSION.SDK_INT < 28) return false
        saveTarget(device.address)
        val h = hid ?: return false
        val ok = try { h.connect(device) } catch (e: Exception) { Log.w(TAG, "connect: ${e.javaClass.simpleName}"); false }
        changed()
        return ok
    }

    /** Drop the connection to the iPhone; stays registered so it can reconnect. */
    fun disconnect() {
        if (Build.VERSION.SDK_INT < 28) return
        commands.releaseAll()
        val d = connected ?: targetDevice()
        saveTarget(null)
        if (d != null) try { hid?.disconnect(d) } catch (_: Exception) {}
        connected = null
        changed()
    }

    /** Unregister and close the profile proxy (turning the feature off). Keeps the `unsupported` finding. */
    fun stop() {
        main.removeCallbacks(registerTimeout)
        commands.releaseAll()
        val h = hid
        hid = null
        connected = null
        registered = false
        starting = false
        if (Build.VERSION.SDK_INT >= 28 && h != null) {
            try { h.unregisterApp() } catch (_: Exception) {}
            try { adapter()?.closeProfileProxy(BluetoothProfile.HID_DEVICE, h) } catch (_: Exception) {}
        }
        changed()
    }

    /** The profile service went away (Bluetooth turned off): back to off, the service retries when it comes back. */
    private fun lost() {
        commands.releaseAll()
        hid = null
        connected = null
        registered = false
        starting = false
        changed()
    }

    // ── The chosen iPhone ───────────────────────────────────────────────────

    private fun prefs() = app?.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    private fun saveTarget(address: String?) {
        prefs()?.edit()?.apply { if (address == null) remove(KEY_TARGET) else putString(KEY_TARGET, address) }?.apply()
    }

    private fun targetDevice(): BluetoothDevice? {
        val address = prefs()?.getString(KEY_TARGET, null) ?: return null
        if (!hasPermissions()) return null
        return try { adapter()?.getRemoteDevice(address) } catch (_: Exception) { null }
    }
}
