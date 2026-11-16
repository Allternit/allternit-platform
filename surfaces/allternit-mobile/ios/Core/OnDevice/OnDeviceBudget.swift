import Darwin
import Foundation
import os

/// The iPhone's model budget: 0.75 x `os_proc_available_memory()` at cold
/// start (fit spec, S2 spike spec/on-device/ios-memory-spike.md).
///
/// Measured, never assumed. The app does not carry the
/// increased-memory-limit entitlement (production App ID not approved), so
/// an iPhone 15 reads about 3,063 MB -> 2.24 GB budget; with the
/// entitlement it would read about 4,088 MB and this code picks it up with
/// no change.
///
/// "Cold start" without hooking app launch: S2 showed available +
/// phys_footprint stays equal to the cold number while the app grows, so
/// the cold value is reconstructed as available + our own footprint, minus
/// what our loaded model holds (so a loaded model does not inflate it).
/// Read once and cached for the process.
enum OnDeviceBudget {
    static let fraction = 0.75

    private static let lock = OSAllocatedUnfairLock<Int64?>(initialState: nil)

    /// Cold-start available bytes for this process. 0 on the simulator and
    /// macOS (no per-app limit), where fit uses physical memory instead.
    static func coldAvailableBytes(modelBytesHeld: Int64 = 0) -> Int64 {
        lock.withLock { cached in
            if let cached { return cached }
            #if os(iOS) && !targetEnvironment(simulator)
            let avail = Int64(os_proc_available_memory())
            let value = avail > 0 ? avail + max(0, footprintBytes() - modelBytesHeld) : 0
            #else
            var value: Int64 = 0
            #if DEBUG
            // `-ondevice-cold-mb 3063` (DEBUG, simulator only): use a measured
            // iPhone number (S2: iPhone 15 = 3,063 MB) so simulator screens
            // show phone fits instead of half the Mac's memory.
            let mb = UserDefaults.standard.integer(forKey: "ondevice-cold-mb")
            if mb > 0 { value = Int64(mb) * 1_000_000 }
            #endif
            #endif
            cached = value
            return value
        }
    }

    /// The fit budget for this device.
    static func budget() -> DeviceBudget {
        let cold = coldAvailableBytes()
        let base = cold > 0 ? Double(cold) : Double(ProcessInfo.processInfo.physicalMemory) * 0.5
        return DeviceBudget(kind: .unified, fastBytes: (base * fraction).rounded(.down), spillBytes: 0, apple: true)
    }

    /// Live headroom right now (for "load anyway?" checks right before a load).
    static func availableNow() -> Int64 {
        #if os(iOS) && !targetEnvironment(simulator)
        return Int64(os_proc_available_memory())
        #else
        return Int64(ProcessInfo.processInfo.physicalMemory / 2)
        #endif
    }

    /// This process's physical footprint (what jetsam counts).
    static func footprintBytes() -> Int64 {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let kr = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        return kr == KERN_SUCCESS ? Int64(info.phys_footprint) : 0
    }
}
