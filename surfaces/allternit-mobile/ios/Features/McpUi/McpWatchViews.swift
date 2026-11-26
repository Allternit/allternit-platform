import SwiftUI

/// Watching rows above the composer (web WatchingStrip): live / webhook / poll,
/// the last 3 lines, "Open all N lines", Pause and Stop. Hidden when there are none.
struct McpWatchStrip: View {
    let sessionId: String?
    @State private var watches: [McpSessionWatch] = []
    @State private var paused: Set<String> = []
    @State private var allLines: McpSessionWatch?
    @State private var tick = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Group {
            if !watches.isEmpty {
                VStack(spacing: 0) {
                    ForEach(watches) { row($0) }
                }
                .padding(.horizontal, 12)
                .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal, 16).padding(.bottom, 6)
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Watching")
            }
        }
        .task(id: sessionId) { await poll() }
        .sheet(item: $allLines) { McpWatchLinesSheet(watch: $0) }
    }

    private func icon(_ mode: String) -> String {
        switch mode {
        case "live": return "dot.radiowaves.left.and.right"
        case "webhook": return "bell"
        default: return "clock"
        }
    }

    private func note(_ w: McpSessionWatch) -> String {
        switch w.resolvedMode {
        case "live": return "Live"
        case "webhook": return "Wakes this chat when it fires"
        default: return "Checks every \(max(1, (w.pollMs ?? 30_000) / 1000)) s while open"
        }
    }

    private func row(_ w: McpSessionWatch) -> some View {
        let isPaused = paused.contains(w.watchId)
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                Image(systemName: icon(w.resolvedMode))
                    .font(.system(size: 16))
                    .symbolEffect(.pulse, options: .nonRepeating, value: reduceMotion ? 0 : tick)
                VStack(alignment: .leading, spacing: 1) {
                    Text("Watching \(w.event)").font(.footnote.weight(.medium)).lineLimit(1)
                    Text("\(w.server) · \(isPaused ? "Paused" : note(w)) · \(w.eventCount ?? 0) events")
                        .font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                Spacer(minLength: 4)
                if w.resolvedMode == "live" {
                    button(isPaused ? "Resume" : "Pause") {
                        if isPaused { paused.remove(w.watchId) } else { paused.insert(w.watchId) }
                    }
                }
                button("Stop") { Task { await stop(w) } }
            }
            if w.resolvedMode == "live", !isPaused, let tail = w.tail, !tail.isEmpty {
                ForEach(Array(tail.suffix(3).enumerated()), id: \.offset) { _, line in
                    Text(line).font(.system(size: 11, design: .monospaced))
                        .foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                if let n = w.lineCount, n > 0 {
                    Button("Open all \(n) lines") { allLines = w }
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                        .frame(minHeight: 44, alignment: .leading)
                }
            }
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.vertical, 6)
    }

    private func button(_ title: String, _ action: @escaping () -> Void) -> some View {
        Button(title) { Haptics.light(); action() }
            .font(.caption).foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 10).frame(minHeight: 44)
    }

    @MainActor private func stop(_ w: McpSessionWatch) async {
        do { try await McpServersClient.shared.stopWatch(watchId: w.watchId) } catch { return }
        watches.removeAll { $0.watchId == w.watchId }
    }

    /// Re-reads the list while the chat is open (the server keeps it in memory with a lease).
    @MainActor private func poll() async {
        guard let sessionId, !sessionId.isEmpty else { watches = []; return }
        while !Task.isCancelled {
            if let list = try? await McpServersClient.shared.watches(sessionId: sessionId) {
                let paused = self.paused
                // A paused live watch keeps its old tail instead of redrawing it.
                watches = list.map { w in paused.contains(w.watchId) ? (watches.first { $0.watchId == w.watchId } ?? w) : w }
                tick += 1
            }
            try? await Task.sleep(for: .seconds(watches.contains { $0.resolvedMode == "live" } ? 3 : 10))
        }
    }
}

/// "Open all N lines".
struct McpWatchLinesSheet: View {
    let watch: McpSessionWatch
    @State private var lines: [McpWatchLine] = []
    @State private var failed = false
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            List {
                if failed { Text("Couldn't load the lines.").foregroundColor(Theme.statusError) }
                ForEach(lines) { l in
                    Text(l.text).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
                }
            }
            .listStyle(.plain)
            .navigationTitle("Watching \(watch.event)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            .task {
                do { lines = try await McpServersClient.shared.watchLines(watchId: watch.watchId) } catch { failed = true }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

/// "Watch an app event" (decision B): pick a connected app, then one of its events.
struct McpWatchAppEventSheet: View {
    let sessionId: String?
    @State private var connectors: [Item] = []
    @State private var picked: Item?
    @State private var events: [EventDef] = []
    @State private var message: String?
    @State private var loading = true
    @Environment(\.dismiss) private var dismiss

    struct Item: Decodable, Identifiable, Hashable { let id: String; let name: String? }
    struct EventDef: Decodable, Identifiable, Hashable { let name: String; let title: String?; var id: String { name } }
    private struct ConnectorsBody: Decodable { let connectors: [Item]? }
    private struct EventsBody: Decodable { let supported: Bool?; let events: [EventDef]? }
    private struct StartBody: Encodable { let server: String; let event: String; let session_id: String; let delivery: String }

    var body: some View {
        NavigationStack {
            List {
                if sessionId == nil {
                    Text("Send a message first, then watch an event from this chat.").foregroundColor(Color("TextSecondary"))
                } else if let picked {
                    Section(picked.name ?? picked.id) {
                        if events.isEmpty { Text(loading ? "Loading…" : "This app offers no events.").foregroundColor(Color("TextSecondary")) }
                        ForEach(events) { e in
                            Button { Task { await start(picked, e) } } label: {
                                HStack { Image(systemName: "dot.radiowaves.left.and.right"); Text(e.title ?? e.name); Spacer() }
                                    .frame(minHeight: 44)
                            }
                        }
                    }
                } else {
                    if connectors.isEmpty { Text(loading ? "Loading…" : "No connected apps.").foregroundColor(Color("TextSecondary")) }
                    ForEach(connectors) { c in
                        Button { Task { await pick(c) } } label: {
                            HStack { Text(c.name ?? c.id); Spacer(); Image(systemName: "chevron.right").font(.caption) }
                                .frame(minHeight: 44)
                        }
                    }
                }
                if let message { Text(message).font(.footnote).foregroundColor(Color("TextSecondary")) }
            }
            .navigationTitle("Watch an app event")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() } } }
            .task {
                let body: ConnectorsBody? = try? await APIClient.shared.get(path: "mcp/connectors")
                connectors = body?.connectors ?? []
                loading = false
            }
        }
    }

    @MainActor private func pick(_ c: Item) async {
        picked = c; loading = true; message = nil
        let seg = c.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? c.id
        let body: EventsBody? = try? await APIClient.shared.get(path: "mcp/connectors/\(seg)/events")
        events = body?.events ?? []
        loading = false
    }

    @MainActor private func start(_ c: Item, _ e: EventDef) async {
        guard let sessionId else { return }
        do {
            try await APIClient.shared.post(path: "mcp/watches", body: StartBody(server: c.id, event: e.name, session_id: sessionId, delivery: "poll"))
            dismiss()
        } catch {
            message = "Couldn't start this watch. Try again."
        }
    }
}
