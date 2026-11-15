import SwiftUI

/// Dev-tool benchmark screen (PocketPal-inspired), reached from the Models
/// tab / Model Management. Device spec card + a speed test against whichever
/// model the user picks, with persisted cross-run history. See
/// BenchmarkResult's doc comment for why this measures network round-trip
/// timing rather than real on-device inference telemetry.
struct BenchmarkView: View {
    @ObservedObject private var modelStore = ModelStore.shared
    @StateObject private var benchmarkStore = BenchmarkStore.shared

    @State private var selectedModelId: String?
    @State private var isRunning = false
    @State private var runError: String? = nil

    var body: some View {
        List {
            deviceSpecSection
            runSection
            if !benchmarkStore.results.isEmpty {
                historySection
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .navigationTitle("Benchmark")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            modelStore.fetchModelsIfNeeded()
            if selectedModelId == nil {
                selectedModelId = modelStore.selectedModelId
            }
        }
    }

    // MARK: - Device spec card

    private var deviceSpecSection: some View {
        let specs = DeviceSpecs.current()
        return Section {
            specRow("Device", specs.hardwareIdentifier)
            specRow("OS", "\(specs.systemName) \(specs.systemVersion)")
            specRow("CPU cores", "\(specs.coreCount)")
            specRow("Memory", specs.memoryLabel)
            if let gpuName = specs.gpuName {
                specRow("GPU", gpuName)
            }
        } header: {
            Text("This Device")
        }
    }

    private func specRow(_ label: String, _ value: String) -> some View {
        HStack {
            Text(label)
                .foregroundColor(Color("TextPrimary"))
            Spacer()
            Text(value)
                .font(.subheadline)
                .foregroundColor(Color("TextSecondary"))
        }
    }

    // MARK: - Run

    private var runSection: some View {
        Section {
            Picker("Model", selection: $selectedModelId) {
                Text("Backend default").tag(String?.none)
                ForEach(modelStore.models) { model in
                    Text(model.name).tag(model.id as String?)
                }
            }
            .font(.subheadline)

            Button(action: runBenchmark) {
                HStack {
                    if isRunning {
                        ProgressView()
                        Text("Running…")
                    } else {
                        Text("Run Benchmark")
                    }
                }
            }
            .disabled(isRunning)

            if let runError {
                Text(runError)
                    .font(.caption)
                    .foregroundColor(Theme.statusWarning)
            }
        } header: {
            Text("Speed Test")
        } footer: {
            Text("Sends one fixed prompt and times the response over the network — an approximation, not a real inference-engine benchmark. There's no on-device model to benchmark the way a fully local app would (Allternit's local-model hosting runs on a paired desktop/VPS/cloud runtime, not on the phone itself).")
        }
    }

    private func runBenchmark() {
        isRunning = true
        runError = nil
        Task {
            do {
                let label = modelStore.models.first(where: { $0.id == selectedModelId })?.name ?? "Backend default"
                let result = try await BenchmarkRunner.run(modelId: selectedModelId, modelLabel: label)
                benchmarkStore.add(result)
                let generator = UINotificationFeedbackGenerator()
                generator.notificationOccurred(.success)
            } catch {
                runError = FriendlyErrorMessage.from(error.localizedDescription, doing: "run the benchmark")
            }
            isRunning = false
        }
    }

    // MARK: - History

    private var historySection: some View {
        Section {
            ForEach(benchmarkStore.results) { result in
                BenchmarkResultRow(result: result)
            }
            .onDelete { offsets in
                // Capture ids before mutating — removing by index while
                // iterating stale offsets would shift later indices.
                let ids = offsets.map { benchmarkStore.results[$0].id }
                for id in ids {
                    benchmarkStore.remove(id)
                }
            }
        } header: {
            Text("History")
        } footer: {
            Text("Swipe to delete a run.")
        }
    }
}

private struct BenchmarkResultRow: View {
    let result: BenchmarkResult

    private static let dateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateStyle = .short
        formatter.timeStyle = .short
        return formatter
    }()

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(result.modelLabel)
                    .font(.subheadline)
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Text(Self.dateFormatter.string(from: result.ranAt))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            Text(String(format: "%.2fs to first token · ~%.0f tok/s", result.timeToFirstToken, result.approxTokensPerSecond))
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
        }
        .padding(.vertical, 2)
    }
}
