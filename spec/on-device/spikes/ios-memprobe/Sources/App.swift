import SwiftUI
import os

private let log = Logger(subsystem: "com.allternit.spike.memprobe", category: "probe")

func mb(_ b: UInt64) -> Int { Int(b / 1_048_576) }

func deviceModel() -> String {
    var s = utsname(); uname(&s)
    return withUnsafePointer(to: &s.machine) { $0.withMemoryRebound(to: CChar.self, capacity: 1) { String(cString: $0) } }
}

func footprintMB() -> Int {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    let kr = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count) }
    }
    return kr == KERN_SUCCESS ? mb(info.phys_footprint) : -1
}

@MainActor
final class Probe: ObservableObject {
    // Cold-start reading, taken as early as possible.
    let coldAvail = os_proc_available_memory()
    let phys = ProcessInfo.processInfo.physicalMemory
    let model = deviceModel()
    let os = UIDevice.current.systemVersion
    let variant = Bundle.main.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String ?? "?"
    @Published var lines: [String] = []
    @Published var done = false
    private var blocks: [UnsafeMutableRawPointer] = []

    init() {
        add("variant=\(variant) model=\(model) iOS=\(os)")
        add("physicalMemory_MB=\(mb(phys))")
        add("cold_available_MB=\(coldAvail / 1_048_576) (bytes=\(coldAvail))")
        add("footprint_MB=\(footprintMB())")
        Task { await self.stepTest() }
    }

    func add(_ s: String) { lines.append(s); log.notice("PROBE \(s, privacy: .public)"); print("PROBE \(s)"); fflush(stdout) }

    func stepTest() async {
        try? await Task.sleep(nanoseconds: 3_000_000_000)
        let stop = Int(coldAvail / 1_048_576) - 300
        var total = 0
        let step = 256
        while total + step <= stop {
            guard let p = malloc(step * 1_048_576) else { add("malloc failed at \(total)"); break }
            // touch every page with non-zero data so it can't be compressed/zero-filled away
            let pages = step * 256
            let bp = p.assumingMemoryBound(to: UInt8.self)
            for i in 0..<pages { bp[i * 4096] = UInt8(truncatingIfNeeded: i | 1); bp[i * 4096 + 2048] = 0xA5 }
            blocks.append(p); total += step
            add("touched_total_MB=\(total) avail_now_MB=\(os_proc_available_memory() / 1_048_576) footprint_MB=\(footprintMB())")
            try? await Task.sleep(nanoseconds: 200_000_000)
        }
        add("STEPTEST_DONE touched_MB=\(total) stop_limit_MB=\(stop) (avail-300)")
        done = true
    }
}

@main
struct App: SwiftUI.App {
    @StateObject private var p = Probe()
    var body: some Scene {
        WindowGroup {
            ScrollView { VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(p.lines.enumerated()), id: \.offset) { Text($1).font(.system(size: 13, design: .monospaced)) }
            }.padding().padding(.top, 40) }
        }
    }
}
