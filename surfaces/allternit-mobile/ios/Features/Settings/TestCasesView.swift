import SwiftUI

/// Settings › Test cases (eval loop 2.8): every case saved from a chat, one
/// list for the whole account. Run checks replays them; Run live… asks for a
/// price first, then polls the run.
struct TestCasesView: View {
    private let client = AgentEvalClient()

    private enum Phase: Equatable { case loading, ready, noComputer, unreachable, failed(String) }
    @State private var phase: Phase = .loading
    @State private var cases: [EvalCaseSummary] = []
    @State private var pendingDelete: EvalCaseSummary?
    @State private var running = false
    @State private var results: EvalReport?
    @State private var resultsError: String?
    @State private var showLive = false

    var body: some View {
        PhoneSettingsPage(title: "Test cases") {
            SettingsDescription(text: "Chats you saved to check later. Run them again after a change to see what broke.")
            switch phase {
            case .loading:
                ProgressView().frame(maxWidth: .infinity).padding(.top, 24)
            case .noComputer:
                note("Needs a computer", "Test cases are kept on your computer. Connect one to see and run them.")
            case .unreachable:
                note("Couldn't reach your computer, try again", nil)
                retry
            case .failed(let text):
                note(text, nil)
                retry
            case .ready:
                if cases.isEmpty {
                    note("No test cases yet", "Open a chat, tap the ⋯ button and choose Save as test case.")
                } else {
                    actions
                    VStack(spacing: 0) {
                        ForEach(Array(cases.enumerated()), id: \.element.id) { index, item in
                            if index > 0 { Divider() }
                            row(item)
                        }
                    }
                    .background(Color(.quaternarySystemFill), in: RoundedRectangle(cornerRadius: 12))
                    resultsSection
                }
            }
        }
        .task { await load() }
        .refreshable { await load() }
        .confirmationDialog("Delete this test case?", isPresented: Binding(
            get: { pendingDelete != nil }, set: { if !$0 { pendingDelete = nil } }),
            titleVisibility: .visible, presenting: pendingDelete) { item in
            Button("Delete", role: .destructive) { Task { await delete(item) } }
            Button("Cancel", role: .cancel) {}
        } message: { item in Text(item.title) }
        .sheet(isPresented: $showLive) { RunLiveSheet(client: client, caseCount: cases.count) }
    }

    private var actions: some View {
        HStack(spacing: 10) {
            Button { Task { await runChecks() } } label: {
                HStack(spacing: 6) {
                    if running { ProgressView() }
                    Text("Run checks")
                }
                .frame(maxWidth: .infinity, minHeight: 44)
                .background(Color(red: 0.12, green: 0.118, blue: 0.113), in: RoundedRectangle(cornerRadius: 12))
                .foregroundColor(.white)
            }
            .disabled(running)
            Button { showLive = true } label: {
                Text("Run live…").frame(maxWidth: .infinity, minHeight: 44)
                    .background(Color(.tertiarySystemFill), in: RoundedRectangle(cornerRadius: 12))
                    .foregroundColor(Color("TextPrimary"))
            }
        }
        .font(.system(size: 15, weight: .semibold))
        .buttonStyle(.plain)
        .padding(.bottom, 14)
    }

    private func row(_ item: EvalCaseSummary) -> some View {
        HStack(alignment: .top, spacing: 10) {
            VStack(alignment: .leading, spacing: 4) {
                Text(item.title).font(.system(size: 15)).foregroundColor(Color("TextPrimary")).lineLimit(2)
                HStack(spacing: 6) {
                    Text(item.label == "good" ? "Went well" : "Went wrong")
                    Text("·")
                    Text(lastResultText(item))
                    if item.needsCheck {
                        Text("Needs a check")
                            .padding(.horizontal, 6).padding(.vertical, 1)
                            .background(Color(.tertiarySystemFill), in: Capsule())
                    }
                }
                .font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
            }
            Spacer(minLength: 0)
            Button { pendingDelete = item } label: {
                Image(systemName: "trash").font(.system(size: 15)).frame(width: 44, height: 44)
            }
            .buttonStyle(.plain)
            .foregroundColor(Color("TextSecondary"))
            .accessibilityLabel("Delete")
        }
        .padding(.leading, 12)
        .padding(.vertical, 4)
        .frame(minHeight: 44)
    }

    @ViewBuilder private var resultsSection: some View {
        if let resultsError {
            Text(resultsError).font(.system(size: 14)).foregroundColor(Color("TextSecondary")).padding(.top, 14)
        }
        if let results {
            SettingsSectionHeading(text: "Results")
            if let rate = results.passRate {
                SettingsDescription(text: "\(Int((rate * 100).rounded()))% passed")
            }
            VStack(spacing: 0) {
                ForEach(Array(results.perCase.enumerated()), id: \.element.id) { index, item in
                    if index > 0 { Divider() }
                    HStack {
                        Text(cases.first(where: { $0.id == item.id })?.title ?? item.id)
                            .font(.system(size: 15)).lineLimit(1)
                        Spacer()
                        Text(item.pass ? "Pass" : "Fail").font(.system(size: 13, weight: .semibold))
                    }
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 12).frame(minHeight: 44)
                }
            }
            .background(Color(.quaternarySystemFill), in: RoundedRectangle(cornerRadius: 12))
        }
    }

    private func note(_ title: String, _ detail: String?) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.system(size: 15, weight: .medium)).foregroundColor(Color("TextPrimary"))
            if let detail { Text(detail).font(.system(size: 14)).foregroundColor(Color("TextSecondary")) }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 8)
    }

    private var retry: some View {
        Button("Try again") { Task { await load() } }
            .font(.system(size: 15, weight: .semibold)).padding(.top, 10)
    }

    private func lastResultText(_ item: EvalCaseSummary) -> String {
        switch item.lastResult {
        case "pass": return "Passed"
        case "fail": return "Failed"
        default: return "Not run yet"
        }
    }

    private func fail(_ error: Error) {
        switch AgentEvalError.map(error) {
        case .noComputer: phase = .noComputer
        case .unreachable: phase = .unreachable
        case let other: phase = .failed(other.message)
        }
    }

    private func load() async {
        do {
            cases = try await client.listCases()
            phase = .ready
        } catch { fail(error) }
    }

    private func delete(_ item: EvalCaseSummary) async {
        do {
            try await client.deleteCase(id: item.id)
            cases.removeAll { $0.id == item.id }
            Haptics.light()
        } catch { fail(error) }
    }

    private func runChecks() async {
        running = true; resultsError = nil
        defer { running = false }
        do {
            let started = try await client.startRun(mode: "replay")
            results = started.report
            if started.report == nil { resultsError = "No results came back, try again" }
            await load()
        } catch { resultsError = AgentEvalError.map(error, conflict: .estimateRequired).message }
    }
}

/// Run live…: the price first, then the run with progress and Cancel.
private struct RunLiveSheet: View {
    let client: AgentEvalClient
    let caseCount: Int

    @Environment(\.dismiss) private var dismiss
    private enum Phase: Equatable { case estimating, ready(EvalEstimate), running(String, Int, Int), done(EvalReport?), ended(String), failed(String) }
    @State private var phase: Phase = .estimating
    @State private var poll: Task<Void, Never>?

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Run live").font(.system(size: 17, weight: .semibold))
            content
            Spacer(minLength: 0)
            buttons
        }
        .padding(16)
        .background(Color.white.ignoresSafeArea())
        .presentationDetents([.medium])
        .task { await estimate() }
        .onDisappear { poll?.cancel() }
        .interactiveDismissDisabled({ if case .running = phase { return true } else { return false } }())
    }

    @ViewBuilder private var content: some View {
        switch phase {
        case .estimating: HStack { ProgressView(); Text("Getting a price…") }
        case .ready(let e):
            Text("\(e.attempts) runs · about \(e.tokensEst) tokens · about \(e.minutesEst) min").font(.system(size: 15))
            Text(e.billing == "metered" ? "About $\(String(format: "%.2f", e.usdEst ?? 0))" : "Uses your plan")
                .font(.system(size: 15, weight: .medium))
        case .running(_, let done, let total):
            ProgressView(value: Double(done), total: Double(max(total, 1)))
            Text("\(done) of \(total) done").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
        case .done(let report):
            Text(report?.passRate.map { "\(Int(($0 * 100).rounded()))% passed" } ?? "Finished")
                .font(.system(size: 15, weight: .medium))
        case .ended(let text), .failed(let text): Text(text).font(.system(size: 15))
        }
    }

    @ViewBuilder private var buttons: some View {
        switch phase {
        case .ready(let e):
            HStack {
                Button("Cancel") { dismiss() }.frame(maxWidth: .infinity, minHeight: 44)
                Button("Run") { Task { await start(e) } }.frame(maxWidth: .infinity, minHeight: 44).fontWeight(.semibold)
            }
        case .running(let id, _, _):
            Button("Cancel") { Task { try? await client.cancelRun(id: id) } }.frame(maxWidth: .infinity, minHeight: 44)
        case .estimating:
            HStack {
                Button("Cancel") { dismiss() }.frame(maxWidth: .infinity, minHeight: 44)
                Button("Run") {}.frame(maxWidth: .infinity, minHeight: 44).disabled(true)
            }
        default:
            Button("Done") { dismiss() }.frame(maxWidth: .infinity, minHeight: 44)
        }
    }

    private func estimate() async {
        do { phase = .ready(try await client.estimateRun()) }
        catch { phase = .failed(AgentEvalError.map(error).message) }
    }

    private func start(_ e: EvalEstimate) async {
        do {
            let started = try await client.startRun(mode: "live", estimateId: e.estimateId)
            phase = .running(started.runId, 0, e.attempts)
            poll = Task { await pollRun(started.runId) }
        } catch let error {
            let mapped = AgentEvalError.map(error, conflict: .estimateRequired)
            if mapped == .estimateRequired { phase = .estimating; await estimate() }
            else { phase = .failed(mapped.message) }
        }
    }

    private func pollRun(_ id: String) async {
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            if Task.isCancelled { return }
            do {
                let run = try await client.getRun(id: id)
                if run.isFinished {
                    switch run.state {
                    case "done": phase = .done(run.report)
                    case "cancelled": phase = .ended("Run cancelled")
                    default: phase = .failed(run.error ?? "The run failed, try again")
                    }
                    return
                }
                phase = .running(id, run.done, run.total)
            } catch {
                phase = .failed(AgentEvalError.map(error).message)
                return
            }
        }
    }
}
