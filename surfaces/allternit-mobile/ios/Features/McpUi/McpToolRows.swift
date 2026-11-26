import SwiftUI

/// Code Mode row (`mcp_script`): collapsed "Ran a script · N servers · M calls";
/// expanded, stacked: script, calls, result. No tabs on the phone.
struct McpScriptRowView: View {
    let meta: McpScriptMeta
    @State private var open = false
    @State private var bounce = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                Haptics.light()
                if !reduceMotion { bounce += 1 }
                withAnimation(reduceMotion ? nil : .easeOut(duration: 0.18)) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: "doc.text")
                        .font(.system(size: 17))
                        .symbolEffect(.bounce, options: .nonRepeating, value: bounce)
                    Text(meta.collapsedTitle).font(.subheadline.weight(.medium))
                    Spacer(minLength: 4)
                    Text(String(format: "%.1f s", Double(meta.durationMs) / 1000))
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                    Image(systemName: open ? "chevron.up" : "chevron.down")
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                }
                .foregroundColor(Color("TextPrimary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(meta.collapsedTitle)
            .accessibilityHint(open ? "Collapse" : "Expand")

            if open {
                VStack(alignment: .leading, spacing: 12) {
                    section("Script") { mono(meta.script) }
                    section("Calls") {
                        VStack(alignment: .leading, spacing: 6) {
                            ForEach(meta.calls) { c in
                                HStack(spacing: 6) {
                                    Image(systemName: c.ok ? "checkmark" : "xmark")
                                        .font(.caption2)
                                        .foregroundColor(c.ok ? Color("TextSecondary") : Theme.statusError)
                                    Text("\(c.server) \(c.tool)").font(.footnote)
                                    Spacer(minLength: 4)
                                    Text("\(c.ms) ms").font(.caption).foregroundColor(Color("TextSecondary"))
                                }
                                if let e = c.error {
                                    Text(e).font(.caption).foregroundColor(Theme.statusError)
                                }
                            }
                        }
                    }
                    if !meta.resultText.isEmpty { section("Result") { mono(meta.resultText) } }
                }
                .padding(.bottom, 10)
            }
        }
        .padding(.horizontal, 12)
        .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
        .accessibilityElement(children: .contain)
    }

    private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary"))
            content()
        }
    }

    private func mono(_ text: String) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(text).font(.system(size: 12, design: .monospaced))
                .foregroundColor(Color("TextPrimary")).textSelection(.enabled)
                .padding(10)
        }
        .frame(maxHeight: 220)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 8))
    }
}

/// Typed MCP result: key-value card (object) or table (array of objects),
/// Table / Raw switch, and "Report a problem to this server" when `canReport`.
struct McpTypedResultView: View {
    let meta: McpToolMeta
    @State private var raw = false
    @State private var reportState: ReportState = .idle
    @State private var flag = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private enum ReportState { case idle, sending, sent, failed }

    /// An array of objects, or an object whose only value is one.
    private var rows: [[String: Any]]? {
        if let arr = meta.structured as? [[String: Any]], !arr.isEmpty { return arr }
        if let obj = meta.structured as? [String: Any], obj.count == 1,
           let arr = obj.values.first as? [[String: Any]], !arr.isEmpty { return arr }
        return nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("\(meta.connectorName ?? meta.server) · \(meta.tool)").font(.caption).foregroundColor(Color("TextSecondary"))
                Spacer()
                HStack(spacing: 0) {
                    chip("Table", selected: !raw) { raw = false }
                    chip("Raw", selected: raw) { raw = true }
                }
            }
            if raw {
                ScrollView(.horizontal, showsIndicators: false) {
                    Text(McpMetaReader.pretty(meta.structured))
                        .font(.system(size: 12, design: .monospaced)).textSelection(.enabled).padding(10)
                }
                .frame(maxHeight: 240)
                .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 8))
            } else if let rows {
                table(rows)
            } else if let obj = meta.structured as? [String: Any] {
                keyValue(obj)
            } else {
                Text(McpMetaReader.cell(meta.structured)).font(.footnote)
            }
            if meta.canReport { reportRow }
        }
        .padding(12)
        .background(Color("FillNeutral").opacity(0.5), in: RoundedRectangle(cornerRadius: 12))
    }

    private func chip(_ title: String, selected: Bool, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title).font(.caption)
                .padding(.horizontal, 12).frame(minHeight: 44)
                .background(selected ? Color("FillNeutral") : .clear, in: Capsule())
                .foregroundColor(Color("TextPrimary"))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func keyValue(_ obj: [String: Any]) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(obj.keys.sorted(), id: \.self) { key in
                HStack(alignment: .top) {
                    Text(key).font(.footnote).foregroundColor(Color("TextSecondary"))
                        .frame(width: 96, alignment: .leading)
                    Text(McpMetaReader.cell(obj[key])).font(.footnote)
                        .foregroundColor(Color("TextPrimary")).textSelection(.enabled)
                    Spacer(minLength: 0)
                }
            }
        }
    }

    private func table(_ rows: [[String: Any]]) -> some View {
        var cols: [String] = []
        for r in rows { for k in r.keys.sorted() where !cols.contains(k) { cols.append(k) } }
        return ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                GridRow { ForEach(cols, id: \.self) { Text($0).font(.caption.weight(.semibold)).foregroundColor(Color("TextSecondary")) } }
                ForEach(Array(rows.prefix(50).enumerated()), id: \.offset) { _, r in
                    GridRow { ForEach(cols, id: \.self) { Text(McpMetaReader.cell(r[$0])).font(.footnote) } }
                }
            }
        }
    }

    @ViewBuilder private var reportRow: some View {
        switch reportState {
        case .sent:
            Label("Thanks, sent to the server", systemImage: "checkmark")
                .font(.footnote).foregroundColor(Color("TextSecondary")).frame(minHeight: 44)
        case .failed:
            Text("Couldn't send. This server may not accept reports.")
                .font(.footnote).foregroundColor(Theme.statusError).frame(minHeight: 44)
        default:
            Button {
                if !reduceMotion { flag += 1 }
                reportState = .sending
                Task {
                    do {
                        try await McpServersClient.shared.sendFeedback(connectorId: meta.connectorId ?? meta.server, tool: meta.tool)
                        reportState = .sent
                    } catch { reportState = .failed }
                }
            } label: {
                Label("Report a problem to this server", systemImage: "flag")
                    .font(.footnote).underline()
                    .symbolEffect(.bounce, options: .nonRepeating, value: flag)
                    .frame(minHeight: 44)
            }
            .buttonStyle(.plain)
            .foregroundColor(Color("TextSecondary"))
            .disabled(reportState == .sending)
        }
    }
}
