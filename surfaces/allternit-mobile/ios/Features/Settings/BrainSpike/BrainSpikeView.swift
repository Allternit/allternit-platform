#if DEBUG
//
// D3 SPIKE — "Brain Spike" screen. DEBUG builds only, linked from the
// Settings spike section (compile-gated like the `-skip-auth` shim at
// App/AllternitApp.swift:79-86). Drives BrainGit: clone → append frontmatter
// page → commit → push against a configurable remote (dev API default:
// 127.0.0.1:8013 — falls back to whatever smart-HTTP endpoint is typed in),
// with a scrolling log as the proof surface.
//
// Automation: `-open-settings-brain-spike` deep-links here from the settings
// sheet; `-brain-spike-auto` runs the full proof on appear and writes
// <Documents>/brain-spike-result.json. URL/token can be overridden with
// `-brain-spike-url <url>` / `-brain-spike-token <token>` launch args.
//

import SwiftUI

struct BrainSpikeView: View {
    @StateObject private var git = BrainGit()
    @State private var remoteURL: String = Self.defaultRemoteURL
    @State private var token: String = Self.defaultToken
    @State private var didAutoRun = false

    /// Clone target inside the app container (file:// leg manages its own
    /// bare repo next to it).
    private var cloneDir: URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("brain-spike/manual-clone")
    }

    var body: some View {
        List {
            Section {
                TextField("Remote URL", text: $remoteURL)
                    .font(.system(.subheadline, design: .monospaced))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                SecureField("Token (allternit_git_…)", text: $token)
                    .font(.system(.subheadline, design: .monospaced))
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
            } header: {
                Text("Remote")
            } footer: {
                Text("Dev API default: http://127.0.0.1:8013/git/brain.git (D2 git endpoints may not exist yet — point at any reachable smart-HTTP remote). Token rides as HTTP Basic with username `allternit`.")
            }

            Section {
                Button(action: { runFullProof() }) {
                    Label("Run full round trip", systemImage: "arrow.triangle.2.circlepath")
                }
                Button(action: { Task { await git.clone(url: remoteURL, to: cloneDir, username: "allternit", token: token) } }) {
                    Label("Clone", systemImage: "square.and.arrow.down")
                }
                Button(action: { Task { _ = await git.appendPageAndCommit(dir: cloneDir) } }) {
                    Label("Append page + commit", systemImage: "doc.badge.plus")
                }
                Button(action: { Task { _ = await git.push(dir: cloneDir, username: "allternit", token: token) } }) {
                    Label("Push", systemImage: "arrow.up.to.line")
                }
                Button("Clear log", role: .destructive) { git.clearLog() }
            } header: {
                Text("Steps")
            }
            .disabled(git.running)

            Section {
                ScrollView {
                    Text(git.log.isEmpty ? "— no output yet —" : git.log.joined(separator: "\n"))
                        .font(.system(.caption, design: .monospaced))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .textSelection(.enabled)
                }
                .frame(minHeight: 220)
            } header: {
                HStack {
                    Text("Log")
                    Spacer()
                    if git.running { ProgressView() }
                }
            }
        }
        .listStyle(.insetGrouped)
        .scrollContentBackground(.hidden)
        .background(Color("BgPrimary"))
        .accessibilityIdentifier("brainSpikeList")
        .navigationTitle("Brain Spike")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            // `-brain-spike-auto` (DEBUG only): run the whole proof without
            // taps — the result JSON is the harness-readable verdict.
            guard !didAutoRun, launchArgumentEnabled("brain-spike-auto") else { return }
            didAutoRun = true
            remoteURL = Self.launchArgValue("-brain-spike-url") ?? Self.defaultRemoteURL
            token = Self.launchArgValue("-brain-spike-token") ?? Self.defaultToken
            runFullProof()
        }
    }

    private func runFullProof() {
        Task { await git.runFullProof(httpURL: remoteURL, username: "allternit", token: token) }
    }

    // MARK: - Launch-arg plumbing (DEBUG automation)

    /// Dev API git endpoint (D2); if it 404s, override with
    /// `-brain-spike-url` pointing at a local smart-HTTP server.
    private static var defaultRemoteURL: String {
        "http://127.0.0.1:8013/git/brain.git"
    }

    private static var defaultToken: String {
        ProcessInfo.processInfo.environment["BRAIN_SPIKE_TOKEN"]
            ?? "allternit_git_spike_token_123"
    }

    /// Value of `-flag <value>` style launch arguments.
    private static func launchArgValue(_ flag: String) -> String? {
        let args = CommandLine.arguments
        guard let index = args.firstIndex(of: flag), args.indices.contains(index + 1) else { return nil }
        return args[index + 1]
    }
}
#endif
