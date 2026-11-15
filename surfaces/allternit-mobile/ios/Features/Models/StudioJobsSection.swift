import SwiftUI

// Freeweights Studio jobs on the paired computer: read-only list, log tail and
// cancel. Jobs are started on Desktop or the web, never here.

@MainActor
final class StudioJobsStore: ObservableObject {
    enum Phase: Equatable {
        case loading
        case noComputer
        case offline
        case failed(String)
        case loaded([StudioJob])
    }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var engines: [String: StudioEngineCard] = [:]
    var engineNames: [String: String] { engines.mapValues(\.name) }
    private let client = StudioJobsClient.shared

    func refresh() async {
        do {
            phase = .loaded(try await client.listJobs())
            if engines.isEmpty, let cards = try? await client.engines() { engines = cards }
        } catch RuntimeRelayError.noComputer {
            phase = .noComputer
        } catch RuntimeRelayError.unreachable {
            phase = .offline
        } catch {
            if case .loaded = phase { return } // keep the last list on a blip
            phase = .failed(StudioJobsFormat.message(for: error))
        }
    }

    /// Polls the list every 5 s while the section is on screen.
    func poll() async {
        await refresh()
        while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(5))
            if Task.isCancelled { return }
            await refresh()
        }
    }
}

enum StudioJobsFormat {
    static func message(for error: Error) -> String {
        if let relay = error as? RuntimeRelayError { return relay.localizedDescription }
        if case APIError.httpError(_, let text?) = error, !text.isEmpty {
            // The host's own sentence (a code on its own, or its reason) for refusals.
            return sentence(code: text, reason: text) ?? "Training jobs couldn't be loaded. Try again in a moment."
        }
        return "Training jobs couldn't be loaded. Try again in a moment."
    }

    static let hardwareLabels: [String: String] = [
        "apple-silicon": "Mac with Apple Silicon",
        "cuda-linux": "Linux with an NVIDIA GPU",
        "cuda-windows": "Windows with an NVIDIA GPU",
        "rocm-linux": "Linux with an AMD GPU",
        "xpu-linux": "Linux with an Intel GPU",
        "xpu-windows": "Windows with an Intel GPU",
        "cpu": "CPU only",
    ]

    static let backendLabels: [String: String] = [
        "mlx": "MLX",
        "torch-mps": "PyTorch (Metal)",
        "torch-cuda": "PyTorch (CUDA)",
        "torch-rocm": "PyTorch (ROCm)",
        "torch-xpu": "PyTorch (Intel XPU)",
        "torch-cpu": "PyTorch (CPU)",
    ]

    /// Hardware Allternit has run real jobs on. Empty until a run is recorded
    /// (phase 4 decision 6), so every backend carries the untested line.
    static let testedHardware: Set<String> = []

    static let untestedLine = "Not yet tested by Allternit on this hardware"

    static func outputLabel(_ kind: String) -> String {
        switch kind {
        case "adapter": return "Adapter"
        case "merged": return "Merged model"
        case "gguf": return "GGUF model"
        case "mlx": return "MLX model"
        default: return kind
        }
    }

    /// "Runs with MLX on Mac with Apple Silicon", or nil when the card says
    /// nothing usable (older host, engine refused here).
    static func backendLine(_ fit: StudioFit?) -> String? {
        guard let fit, fit.ok != false, let backend = fit.backend, !backend.isEmpty else { return nil }
        let name = backendLabels[backend] ?? backend
        if let hw = fit.hardware, let label = hardwareLabels[hw] { return "Runs with \(name) on \(label)" }
        return "Runs with \(name)"
    }

    static func isUntested(_ fit: StudioFit?) -> Bool {
        guard let fit, fit.ok != false, fit.backend != nil else { return false }
        return !testedHardware.contains(fit.hardware ?? "")
    }

    /// Same wording as the web (studio-jobs.ts): the host's reason when it
    /// sent one. Nil for a string that is not one of these refusal codes.
    static func sentence(code: String, reason: String?) -> String? {
        let host = reason.flatMap { $0 == code ? nil : ($0.isEmpty ? nil : $0) }
        switch code {
        case "hardware_not_supported": return host ?? "Your computer can't run this engine. Pick another engine."
        case "method_not_supported": return host ?? "Your computer can't train with that method. Pick another method."
        case "backend_not_supported": return host ?? "That backend can't be used on your computer. Leave the backend on automatic."
        case "output_not_supported": return host ?? "Your computer can't make one of the files you picked. Untick it, then start again."
        case "hardware_unavailable": return host ?? "The training device on your computer can't be used right now. Restart the computer, then try again."
        case "invalid_base": return "That base model can't be used. Pick a model from the Freeweights list."
        default: return reason
        }
    }

    static func methodLabel(_ method: String) -> String {
        switch method {
        case "lora": return "LoRA"
        case "qlora": return "QLoRA"
        case "dpo": return "DPO"
        case "full": return "Full fine-tune"
        default: return method
        }
    }

    static func stateLabel(_ state: String) -> String {
        switch state {
        case "queued": return "Queued"
        case "preparing": return "Preparing"
        case "running": return "Training"
        case "exporting": return "Exporting"
        case "succeeded": return "Done"
        case "failed": return "Failed"
        case "cancelled": return "Cancelled"
        default: return state.capitalized
        }
    }

    static func tint(_ state: String) -> Color {
        switch state {
        case "succeeded": return Theme.statusSuccess
        case "failed": return Theme.statusError
        case "cancelled": return Theme.statusWarning
        default: return Theme.statusInfo
        }
    }
}

struct StudioJobsSection: View {
    @StateObject private var store = StudioJobsStore()
    @State private var selected: StudioJob?
    @State private var isConnectSheetPresented = false
    @ObservedObject private var relay = RuntimeRelayStore.shared

    var body: some View {
        Section {
            VStack(alignment: .leading, spacing: 14) { content }
                .sheet(item: $selected) { job in
                    StudioJobDetailView(job: job, engineName: store.engineNames[job.spec.engine] ?? job.spec.engine, fit: store.engines[job.spec.engine]?.fit) { Task { await store.refresh() } }
                }
                .sheet(isPresented: $isConnectSheetPresented, onDismiss: {
                    Task { await relay.refresh(); await store.refresh() }
                }) {
                    ConnectComputerSheet()
                }
        } header: {
            Text("Training jobs")
        }
        .task { await store.poll() }
    }

    @ViewBuilder
    private var content: some View {
        switch store.phase {
        case .loading:
            ProgressView().frame(maxWidth: .infinity, alignment: .center)
        case .noComputer:
            FriendlyInlineStateView(
                style: .empty,
                icon: "desktopcomputer",
                title: "No computer paired",
                message: "Training jobs run on a computer. Pair one in Computers.",
                actionTitle: "Pair a computer",
                action: { isConnectSheetPresented = true }
            )
        case .offline:
            FriendlyInlineStateView(
                style: .offline,
                icon: "desktopcomputer.trianglebadge.exclamationmark",
                title: "Can't reach your computer",
                message: "Open Allternit Desktop or wake your cloud computer, then pull to refresh.",
                actionTitle: "Fix",
                action: { isConnectSheetPresented = true }
            )
        case .failed(let message):
            FriendlyInlineStateView(
                style: .error,
                icon: "exclamationmark.triangle",
                title: "Couldn't load jobs",
                message: message,
                actionTitle: "Retry",
                action: { Task { await store.refresh() } }
            )
        case .loaded(let jobs):
            if jobs.isEmpty {
                FriendlyInlineStateView(
                    style: .empty,
                    icon: "slider.horizontal.3",
                    title: "No training jobs",
                    message: "Start jobs from Freeweights Studio on Desktop or the web."
                )
            } else {
                ForEach(jobs) { job in
                    Button { selected = job } label: { StudioJobRow(job: job, engineName: store.engineNames[job.spec.engine] ?? job.spec.engine, fit: store.engines[job.spec.engine]?.fit) }
                        .buttonStyle(.plain)
                }
            }
        }
    }
}

private struct StudioJobRow: View {
    let job: StudioJob
    let engineName: String
    var fit: StudioFit?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(job.spec.base.repo)
                    .font(.system(size: 15, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer()
                Text(StudioJobsFormat.stateLabel(job.state))
                    .font(.caption.weight(.semibold))
                    .foregroundColor(StudioJobsFormat.tint(job.state))
            }
            Text("\(engineName) · \(StudioJobsFormat.methodLabel(job.spec.method))")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            if job.spec.outputs?.mlx == true {
                Text("Also makes an MLX model")
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}

struct StudioJobDetailView: View {
    let job: StudioJob
    let engineName: String
    var fit: StudioFit?
    var onChange: () -> Void = {}

    @Environment(\.dismiss) private var dismiss
    @State private var current: StudioJob
    @State private var tail = StudioLogTail()
    @State private var confirmCancel = false
    @State private var isCancelling = false
    @State private var error: String?
    @State private var outputs: [StudioJobOutput] = []

    init(job: StudioJob, engineName: String, fit: StudioFit? = nil, onChange: @escaping () -> Void = {}) {
        self.job = job
        self.engineName = engineName
        self.fit = fit
        self.onChange = onChange
        _current = State(initialValue: job)
    }

    private var state: String { tail.state ?? current.state }
    private var finished: Bool { ["succeeded", "failed", "cancelled"].contains(state) }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    row("Base model", current.spec.base.repo)
                    row("Engine", engineName)
                    if let line = StudioJobsFormat.backendLine(fit) {
                        Text(line).font(.caption).foregroundColor(Color("TextSecondary"))
                        if StudioJobsFormat.isUntested(fit) {
                            Text(StudioJobsFormat.untestedLine).font(.caption).foregroundColor(Color("TextSecondary"))
                        }
                    }
                    if let warning = fit?.warning, !warning.isEmpty {
                        Text(warning).font(.caption).foregroundColor(Theme.statusError)
                    }
                    row("Method", StudioJobsFormat.methodLabel(current.spec.method))
                    row("State", StudioJobsFormat.stateLabel(state))
                    if let progress = tail.progress, !finished {
                        ProgressView(value: progress) {
                            Text("\(Int(progress * 100))%").font(.caption.monospacedDigit())
                        }
                    }
                    if let message = current.error, state == "failed" {
                        Text(message).font(.caption).foregroundColor(Theme.statusError)
                    }
                }
                if !outputs.isEmpty {
                    Section("Files made") {
                        ForEach(Array(outputs.enumerated()), id: \.offset) { _, out in
                            Text(StudioJobsFormat.outputLabel(out.kind)).font(.subheadline)
                        }
                    }
                }
                Section("Log") {
                    if tail.lines.isEmpty {
                        Text("No log lines yet.")
                            .font(.subheadline)
                            .foregroundColor(Color("TextSecondary"))
                    } else {
                        ForEach(tail.lines) { line in
                            Text(line.message)
                                .font(.caption.monospaced())
                                .foregroundColor(line.kind == "error" ? Theme.statusError : Color("TextPrimary"))
                        }
                    }
                }
                if let error {
                    Section { Text(error).font(.caption).foregroundColor(Theme.statusError) }
                }
                if !finished {
                    Section {
                        Button(role: .destructive) { confirmCancel = true } label: {
                            HStack {
                                if isCancelling { ProgressView() }
                                Text("Cancel job").frame(maxWidth: .infinity)
                            }
                        }
                        .disabled(isCancelling)
                    }
                }
            }
            .listStyle(.insetGrouped)
            .navigationTitle("Training job")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
            .confirmationDialog("Cancel this training job?", isPresented: $confirmCancel, titleVisibility: .visible) {
                Button("Cancel job", role: .destructive) { Task { await cancel() } }
                Button("Keep running", role: .cancel) {}
            } message: {
                Text("Training stops on your computer. Progress so far is not kept.")
            }
            // Polls only while this sheet is visible; the task ends on dismiss.
            .task { await poll() }
        }
    }

    private func row(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label).foregroundColor(Color("TextSecondary"))
            Spacer()
            Text(value).foregroundColor(Color("TextPrimary")).multilineTextAlignment(.trailing)
        }
        .font(.subheadline)
    }

    private func poll() async {
        while !Task.isCancelled {
            do {
                let page = try await StudioJobsClient.shared.events(id: job.id, after: tail.lastSeq)
                tail.apply(page)
                if let fresh = try? await StudioJobsClient.shared.job(id: job.id) { current = fresh }
                error = nil
                if finished {
                    if state == "succeeded", let files = try? await StudioJobsClient.shared.outputs(id: job.id) { outputs = files }
                    onChange(); return
                }
            } catch {
                self.error = "Lost touch with your computer. The job keeps going there."
            }
            try? await Task.sleep(for: .seconds(3))
        }
    }

    private func cancel() async {
        isCancelling = true
        defer { isCancelling = false }
        do {
            current = try await StudioJobsClient.shared.cancel(id: job.id)
            tail.apply(StudioEventsPage(state: current.state, events: []))
            error = nil
            onChange()
        } catch {
            self.error = "The job couldn't be cancelled. It may have just finished."
        }
    }
}
