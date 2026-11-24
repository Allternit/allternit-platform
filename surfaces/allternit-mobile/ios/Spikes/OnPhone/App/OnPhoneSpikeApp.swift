import SwiftUI
import UniformTypeIdentifiers

@main
struct OnPhoneSpikeApp: App {
    var body: some Scene { WindowGroup { HarnessView() } }
}

/// The measurement harness. Each section measures one row of the S0 report.
@MainActor final class Harness: ObservableObject {
    @Published var log: [String] = []
    @Published var tunnelStatus = "?"
    @Published var typed = ""
    @Published var storeAppScheme = "itms-apps://"
    @Published var extra: [String: Any] = [:]
    @Published var reportURL: URL?

    @AppStorage("runtimeURL", store: UserDefaults(suiteName: Spike.appGroup)) var runtimeURL = ""
    @AppStorage("runtimeToken", store: UserDefaults(suiteName: Spike.appGroup)) var runtimeToken = ""
    @AppStorage("phoneSerial", store: UserDefaults(suiteName: Spike.appGroup)) var phoneSerial = ""

    func say(_ s: String) { log.insert("\(Date().formatted(date: .omitted, time: .standard))  \(s)", at: 0) }

    func refresh() async { tunnelStatus = await TunnelControl.status() }

    func startTunnel(onDemand: Bool) async {
        do { try await TunnelControl.start(onDemand: onDemand); say("tunnel start requested (on demand \(onDemand))") }
        catch { say("tunnel start error: \(error)") }
        try? await Task.sleep(for: .seconds(1))
        await refresh()
    }

    func stopTunnel() async {
        do { try await TunnelControl.stop(); say("tunnel stopped") } catch { say("tunnel stop error: \(error)") }
        await refresh()
    }

    func probeLockdown(runs: Int = 20) async {
        var ms: [Double] = []
        var ok = 0
        var lastErr = ""
        for _ in 0..<runs {
            let r = await LockdownProbe.queryType()
            if let t = r.body?["Type"] as? String, t == "com.apple.mobile.lockdown" { ok += 1; ms.append(r.ms) }
            else { lastErr = r.error ?? "\(r.body ?? [:])" }
        }
        extra["lockdown"] = ["ms": ms, "ok": ok, "error": lastErr]
        say("lockdown QueryType: \(ok)/\(runs) ok, median \(Spike.median(ms).map { String(format: "%.1f ms", $0) } ?? "-") \(lastErr)")
    }

    func importPairRecord(_ url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        guard let data = try? Data(contentsOf: url),
              (try? PropertyListSerialization.propertyList(from: data, format: nil)) is [String: Any] else {
            say("pair record: not a property list"); return
        }
        try? data.write(to: Spike.url(Spike.pairRecord), options: .atomic)
        say("pair record imported (\(data.count) bytes)")
    }

    func checkPairing() async {
        guard let data = try? Data(contentsOf: Spike.url(Spike.pairRecord)),
              let rec = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
            say("no pair record"); return
        }
        let r = await LockdownProbe.startSession(pairRecord: rec)
        let ok = r.body?["SessionID"] != nil
        let note = ok ? "StartSession accepted the pair record (EnableSessionSSL=\(r.body?["EnableSessionSSL"] ?? "?")). TLS upgrade not built."
                      : "StartSession refused: \(r.body?["Error"] ?? r.error ?? "?")"
        extra["pairing"] = ["ok": ok, "ms": r.ms, "note": note]
        say(note)
    }

    func measureRead(_ app: String) {
        Spike.writeJSON(Spike.measureFile, ["app": app, "until": Spike.now + 20])
        say("read window open 20 s for \(app)")
        let url: URL?
        switch app {
        case "Settings": url = URL(string: UIApplication.openSettingsURLString)
        case "Messages": url = URL(string: "sms:")
        case "Safari": url = URL(string: "https://www.apple.com")
        case "Notes": url = URL(string: "mobilenotes://")
        default: url = URL(string: storeAppScheme)
        }
        if let url { UIApplication.shared.open(url) }
    }

    func ownRead(runs: Int = 5) {
        var ms: [Double] = []
        for _ in 0..<runs { if let r = OwnRead.once() { ms.append(r.ms) } }
        extra["ownRead"] = ms
        say("own-window read median \(Spike.median(ms).map { String(format: "%.0f ms", $0) } ?? "-")")
    }

    func queueTyping(target: String) async {
        typed = ""
        let r = await GuardClient.queueTyping(text: Spike.typingSample, target: target)
        extra["typeGuard"] = r.decision
        say("guard for phone.type: \(r.decision) \(r.error ?? "")")
        if r.decision == "allow", target == "Notes", let u = URL(string: "mobilenotes://") { await UIApplication.shared.open(u) }
    }

    func checkSelfTyped() {
        guard typed.count >= Spike.typingSample.count else { return }
        extra["selfTypedExact"] = typed == Spike.typingSample
        say("self-field typing exact: \(typed == Spike.typingSample)")
    }

    func mark(_ m: String) { Spike.appendLine(Spike.marksLog, ["mark": m]); say("mark: \(m)") }

    func soakStart() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        UserDefaults.standard.set(["level": UIDevice.current.batteryLevel, "ts": Spike.now], forKey: "soak")
        say("soak started at \(Int(UIDevice.current.batteryLevel * 100))%")
    }

    func soakEnd() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        guard let s = UserDefaults.standard.dictionary(forKey: "soak"), let l0 = s["level"] as? Float, let t0 = s["ts"] as? Double else { return }
        let l1 = UIDevice.current.batteryLevel
        let minutes = (Spike.now - t0) / 60
        let drop = Double(l0 - l1) * 100 * (30 / max(minutes, 1))
        extra["battery"] = ["start": l0, "end": l1, "minutes": Int(minutes), "dropPct": drop]
        say("soak: \(Int(minutes)) min, \(String(format: "%.1f", drop))% per 30 min")
    }

    func writeReport() { reportURL = Report.write(extra: extra); say("report written: \(reportURL?.lastPathComponent ?? "failed")") }

    /// `-autoHarness` launch argument: run the parts that need no person, write the report.
    func autoRun() async {
        say("auto harness")
        await refresh()
        await probeLockdown(runs: 5)
        ownRead()
        writeReport()
    }
}

struct HarnessView: View {
    @StateObject private var h = Harness()
    @Environment(\.scenePhase) private var phase
    @State private var importing = false
    @FocusState private var fieldFocused: Bool

    var body: some View {
        NavigationStack {
            Form {
                Section("Runtime guard (Wave 1)") {
                    TextField("Runtime URL", text: $h.runtimeURL).textInputAutocapitalization(.never).keyboardType(.URL)
                    SecureField("Token", text: $h.runtimeToken)
                    TextField("Phone serial", text: $h.phoneSerial).textInputAutocapitalization(.never)
                }
                Section("Tunnel and pairing") {
                    LabeledContent("Status", value: h.tunnelStatus)
                    Button("Start tunnel (on demand)") { Task { await h.startTunnel(onDemand: true) } }
                    Button("Stop tunnel") { Task { await h.stopTunnel() } }
                    Button("Probe lockdown x20") { Task { await h.probeLockdown() } }
                    Button("Import pair record") { importing = true }
                    Button("Check pairing") { Task { await h.checkPairing() } }
                }
                Section("Screen: capture and read") {
                    Button("Start screen capture (pick Display)") { ScreenCapture.shared.onLog = { m in Task { @MainActor in h.say(m) } }; ScreenCapture.shared.present() }
                    Button("Stop screen capture") { ScreenCapture.shared.stop() }
                    ForEach(Report.targets, id: \.self) { app in
                        Button("Measure read in \(app)") { h.measureRead(app) }
                    }
                    TextField("Store app URL scheme", text: $h.storeAppScheme).textInputAutocapitalization(.never)
                    Button("Own-window read x5 (control)") { h.ownRead() }
                }
                Section("Taps") {
                    Text("Blocked on this route. See the report.").foregroundStyle(.secondary)
                }
                Section("Typing (200 characters)") {
                    TextField("Self-test field", text: $h.typed, axis: .vertical)
                        .focused($fieldFocused)
                        .onChange(of: h.typed) { h.checkSelfTyped() }
                    Button("Type here (guard, then keyboard)") { Task { await h.queueTyping(target: "OnPhone Spike"); fieldFocused = true } }
                    Button("Type in Notes (guard, then keyboard)") { Task { await h.queueTyping(target: "Notes") } }
                }
                Section("Lock, restart, other VPN, battery") {
                    Button("Mark: about to lock") { h.mark("lock") }
                    Button("Mark: back after restart") { h.mark("restart") }
                    Button("Mark: other VPN turned on") { h.mark("other_vpn_on") }
                    Button("Mark: other VPN turned off") { h.mark("other_vpn_off") }
                    Button("Start 30 min battery soak") { h.soakStart() }
                    Button("End battery soak") { h.soakEnd() }
                }
                Section("Report") {
                    Button("Write report") { h.writeReport() }
                    if let u = h.reportURL { ShareLink(item: u) }
                }
                Section("Log") { ForEach(h.log, id: \.self) { Text($0).font(.caption.monospaced()) } }
            }
            .navigationTitle("OnPhone Spike")
            .onChange(of: phase) { ScreenCapture.shared.appActive = (phase == .active) }
            .fileImporter(isPresented: $importing, allowedContentTypes: [.propertyList, .data, .xml]) { result in
                if case .success(let url) = result { h.importPairRecord(url) }
            }
            .task {
                await h.refresh()
                if ProcessInfo.processInfo.arguments.contains("-autoHarness") { await h.autoRun() }
            }
        }
    }
}
