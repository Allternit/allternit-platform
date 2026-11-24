import SwiftUI

// -----------------------------------------------------------------------------
// RuntimeLookView (Harness & Bots 2.0 M1, step 26; plan D3, D9).
//
// A bot thread drawn in its runtime's own look, from the registry's look
// tokens (contract "Registry entry" + "Token fields added in P1"; web
// `fromTokens` in look-packs/pack-kit.tsx is the reference renderer):
//
// - `kind: tui` (terminal): optional big wordmark, banner lines (optionally
//   boxed), the person's lines in the look's user style after its prompt
//   glyph, tool calls as step lines from the Read/Edit/Run templates with
//   {a}/{r} filled, answers after the answer glyph, the input in its style,
//   and the footer's left/middle/right lines.
// - `kind: app` (desktop app): header with name, subtitle and status, steps
//   in one card, bubbles for the person, and a rounded composer.
//
// Colors are the roles fg/mu/ac/ac2/ok/err (b = bold fg, pill = filled). The
// look's native palette draws in its own theme; the other theme uses the
// `light` (or `dark`) tokens over a neutral base, as the web does. The thread
// is the real one: `ChatViewModel.messages` with their tool calls.
// -----------------------------------------------------------------------------

/// One runtime look's tokens, read from the registry JSON. Unknown fields are
/// ignored; missing ones fall back as the web renderer does.
struct RuntimeLookTokens {
    typealias Segment = (text: String, role: String)
    typealias Line = [Segment]

    struct Palette { var bg, fg, mu, ac, ac2, ok, err: String }

    var app: Bool
    var light: Palette
    var dark: Palette
    var banner: [Line]
    var bigText: String?
    var bigGrad: [String]
    var bigParts: Line
    var bannerBox: Bool
    var boxRole: String
    var prompt: String
    var userStyle: String
    var steps: [String: [Line]]
    var pre: [Line]
    var stepBox: Bool
    var answerGlyph: String?
    var answerGlyphRole: String
    var answerRole: String
    var input: String
    var inputGlyph: String?
    var inputGlyphRole: String
    var inputBorderRole: String
    var placeholder: String
    var footer: (l: Line?, m: Line?, r: Line?)
    var pillRole: String
    var header: String?
    var status: String?
    var radius: CGFloat
    var userFill: String?
    var sendSquare: Bool

    /// The tokens a runtime has for `look` (terminal | desktop), else its other look.
    static func of(_ runtime: RuntimeEntry?, look: String?) -> RuntimeLookTokens? {
        guard let root = runtime?.look?.jObject else { return nil }
        let want = look ?? root["default"]?.jString ?? "terminal"
        let other = want == "terminal" ? "desktop" : "terminal"
        guard let tk = (root[want]?.jObject ?? root[other]?.jObject) else { return nil }
        return RuntimeLookTokens(tk)
    }

    /// Which looks a runtime has (for the Terminal · Desktop switch).
    static func looks(_ runtime: RuntimeEntry?) -> [String] {
        guard let root = runtime?.look?.jObject else { return [] }
        return ["terminal", "desktop"].filter { root[$0]?.jObject?["steps"] != nil }
    }

    init?(_ tk: [String: JSONValue]) {
        guard tk["banner"] != nil, let stepsObj = tk["steps"]?.jObject else { return nil }
        let roles: Set<String> = ["fg", "mu", "ac", "ac2", "ok", "err", "b", "pill"]
        func role(_ v: JSONValue?, _ fallback: String) -> String {
            guard let r = v?.jString, roles.contains(r) else { return fallback }
            return r
        }
        func line(_ v: JSONValue?) -> Line {
            (v?.jArray ?? []).compactMap { seg in
                let parts = seg.jArray
                guard let text = parts.first?.jString else { return nil }
                return (text, role(parts.count > 1 ? parts[1] : nil, "fg"))
            }
        }
        func lines(_ v: JSONValue?) -> [Line] { (v?.jArray ?? []).map { line($0) } }
        func str(_ v: JSONValue?) -> String? { v?.jString.flatMap { $0.isEmpty ? nil : $0 } }

        app = tk["kind"]?.jString == "app"
        let bg = str(tk["bg"]) ?? "#16181d"
        let base = Palette(bg: bg, fg: str(tk["fg"]) ?? "#e5e7eb", mu: str(tk["mu"]) ?? "#9ca3af",
                           ac: str(tk["ac"]) ?? "#e5e7eb", ac2: str(tk["ac2"]) ?? str(tk["ac"]) ?? "#e5e7eb",
                           ok: str(tk["ok"]) ?? "#3fb950", err: str(tk["err"]) ?? "#f85149")
        let hasAc2 = str(tk["ac2"]) != nil
        let nativeDark = tk["dark"]?.jBool ?? (Self.luminance(bg) < 0.4)
        func other(_ generic: Palette, _ o: [String: JSONValue]) -> Palette {
            let ac = str(o["ac"]) ?? base.ac
            var p = Palette(bg: str(o["bg"]) ?? generic.bg, fg: str(o["fg"]) ?? generic.fg, mu: str(o["mu"]) ?? generic.mu,
                            ac: ac, ac2: str(o["ac2"]) ?? (hasAc2 ? base.ac2 : ac),
                            ok: str(o["ok"]) ?? generic.ok, err: str(o["err"]) ?? generic.err)
            // An accent that vanishes on the other background falls back to the text color.
            let lightBg = Self.luminance(p.bg) > 0.5
            func fix(_ c: String) -> String { (lightBg ? Self.luminance(c) > 0.6 : Self.luminance(c) < 0.03) ? p.fg : c }
            p.ac = fix(p.ac)
            p.ac2 = fix(p.ac2)
            return p
        }
        let genericLight = Palette(bg: "#fbfbfa", fg: "#1d1d1b", mu: "#74746f", ac: "", ac2: "", ok: "#1a7f37", err: "#cf222e")
        let genericDark = Palette(bg: "#16181d", fg: "#e5e7eb", mu: "#9ca3af", ac: "", ac2: "", ok: "#3fb950", err: "#f85149")
        if nativeDark {
            dark = base
            light = other(genericLight, tk["light"]?.jObject ?? [:])
        } else {
            light = base
            dark = other(genericDark, tk["dark"]?.jObject ?? [:])
        }

        banner = lines(tk["banner"])
        let big = tk["big"]?.jObject
        bigText = str(big?["text"])
        bigGrad = (big?["grad"]?.jArray ?? []).compactMap(\.jString)
        bigParts = line(big?["parts"])
        bannerBox = tk["bannerBox"]?.jBool ?? false
        boxRole = role(tk["boxRole"], "mu")
        prompt = tk["prompt"]?.jString ?? ""
        let style = tk["userStyle"]?.jString ?? "plain"
        userStyle = ["plain", "shade", "bar", "color", "bubble"].contains(style) ? style : "plain"
        var s: [String: [Line]] = [:]
        for verb in ["Read", "Edit", "Run"] { s[verb] = lines(stepsObj[verb]) }
        steps = s
        pre = lines(tk["pre"])
        stepBox = tk["stepBox"]?.jBool ?? false
        answerGlyph = str(tk["answerGlyph"])
        answerGlyphRole = role(tk["agentGlyph"] ?? tk["answerGlyphRole"], "fg")
        answerRole = role(tk["answerRole"], "fg")
        let inp = tk["input"]?.jString ?? "plain"
        input = ["round", "shade", "bar", "line", "rule", "plain"].contains(inp) ? inp : "plain"
        inputGlyph = tk["inputGlyph"]?.jString
        inputGlyphRole = role(tk["glyphRole"], "mu")
        inputBorderRole = role(tk["inputRole"], input == "bar" ? "ac" : "mu")
        placeholder = tk["placeholder"]?.jString ?? ""
        let f = tk["footer"]?.jObject
        footer = (f?["l"].map { line($0) }, f?["m"].map { line($0) }, f?["r"].map { line($0) })
        pillRole = role(tk["pillRole"], "ac")
        header = str(tk["header"])
        status = str(tk["status"])
        if case .number(let n)? = tk["radius"] { radius = CGFloat(n) } else { radius = 14 }
        userFill = str(tk["userFill"])
        sendSquare = tk["sendSquare"]?.jBool ?? false
        self.nativeDark = nativeDark
    }

    /// True when the look's own palette is dark.
    var nativeDark: Bool

    static func luminance(_ hex: String) -> Double {
        var h = hex.trimmingCharacters(in: CharacterSet.alphanumerics.inverted)
        if h.count == 3 { h = h.map { "\($0)\($0)" }.joined() }
        guard h.count == 6, let v = UInt64(h, radix: 16) else { return 0 }
        func ch(_ c: UInt64) -> Double {
            let x = Double(c) / 255
            return x <= 0.03928 ? x / 12.92 : pow((x + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * ch(v >> 16 & 0xFF) + 0.7152 * ch(v >> 8 & 0xFF) + 0.0722 * ch(v & 0xFF)
    }
}

/// One tool call as a step: verb (template), argument and result.
struct RuntimeLookStep: Identifiable {
    enum Status { case running, success, error }
    let id: String
    let verb: String
    let a: String
    let r: String
    let status: Status

    /// Which template a tool call uses, from its name (web `stepVerb`).
    static func verb(_ name: String) -> String {
        let n = name.lowercased()
        let edit = ["edit", "write", "patch", "replace", "update", "create", "apply", "insert", "delete", "move", "rename"]
        let read = ["read", "view", "cat", "open", "grep", "glob", "search", "find", "list", "ls", "fetch", "explore", "look"]
        if edit.contains(where: { n.contains($0) }) { return "Edit" }
        if read.contains(where: { n.contains($0) }) { return "Read" }
        return "Run"
    }

    static func oneLine(_ s: String?, max: Int = 80) -> String {
        let t = (s ?? "").split(separator: "\n").first { !$0.trimmingCharacters(in: .whitespaces).isEmpty }
            .map { $0.trimmingCharacters(in: .whitespaces) } ?? ""
        return t.count > max ? String(t.prefix(max - 1)) + "…" : t
    }

    init(_ call: ToolCallRecord) {
        id = call.id
        verb = Self.verb(call.toolName)
        let keys = ["path", "file_path", "filePath", "file", "command", "cmd", "pattern", "query", "url"]
        let arg = keys.lazy.compactMap { call.input[$0] }.first { !$0.isEmpty }
        let a0 = Self.oneLine(arg ?? call.title)
        a = a0.isEmpty ? call.toolName : a0
        switch call.state {
        case .running: status = .running; r = ""
        case .failed: status = .error; r = "failed"
        case .done:
            status = .success
            let out = Self.oneLine(call.output)
            r = out.isEmpty ? "done" : out
        }
    }
}

struct RuntimeLookView: View {
    let runtime: RuntimeEntry
    let tokens: RuntimeLookTokens
    let messages: [MessageRecord]
    let isStreaming: Bool
    let onSend: (String) -> Void
    let onStop: () -> Void

    @Environment(\.colorScheme) private var colorScheme
    @State private var draft = ""
    @FocusState private var focused: Bool

    private var t: RuntimeLookTokens { tokens }
    private var pal: RuntimeLookTokens.Palette { colorScheme == .dark ? t.dark : t.light }

    private func color(_ role: String) -> Color {
        switch role {
        case "mu": return Color(hex: pal.mu)
        case "ac": return Color(hex: pal.ac)
        case "ac2": return Color(hex: pal.ac2)
        case "ok": return Color(hex: pal.ok)
        case "err": return Color(hex: pal.err)
        default: return Color(hex: pal.fg)
        }
    }
    private var line: Color { color("mu").opacity(0.4) }
    private var shade: Color { color("fg").opacity(0.07) }
    private var mono: Font { .system(.footnote, design: .monospaced) }
    private var body14: Font { .system(.subheadline) }

    var body: some View {
        VStack(spacing: 0) {
            if runtime.hasLiveApp {
                liveNote
            }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: t.app ? 12 : 8) {
                        banner
                        ForEach(messages.filter { $0.role != "system" }) { message in
                            row(message).id(message.id)
                        }
                        Color.clear.frame(height: 1).id("runtimeLookEnd")
                    }
                    .padding(.horizontal, 16)
                    .padding(.vertical, 14)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: messages.last?.content) { _, _ in proxy.scrollTo("runtimeLookEnd", anchor: .bottom) }
                .onAppear { proxy.scrollTo("runtimeLookEnd", anchor: .bottom) }
            }
            composer
        }
        .font(t.app ? body14 : mono)
        .foregroundColor(color("fg"))
        .background(Color(hex: pal.bg).ignoresSafeArea(edges: .bottom))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(runtime.name) view")
    }

    // MARK: Live web app note (phones have no frame for it)

    private var liveNote: some View {
        HStack(spacing: 8) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 13))
                .accessibilityHidden(true)
            Text("Open \(runtime.name)'s own app on your computer in Allternit Desktop")
                .font(.footnote)
                .fixedSize(horizontal: false, vertical: true)
        }
        .foregroundColor(Color("TextSecondary"))
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color("BgPanel"))
        .accessibilityElement(children: .combine)
    }

    // MARK: Lines

    private func fill(_ s: String, _ step: RuntimeLookStep?) -> String {
        var out = s.replacingOccurrences(of: "{name}", with: runtime.name)
        if let step {
            out = out.replacingOccurrences(of: "{a}", with: step.a).replacingOccurrences(of: "{r}", with: step.r)
        }
        return out
    }

    /// One token line as attributed text (roles to colors, `b` bold, `pill` filled).
    private func attributed(_ l: RuntimeLookTokens.Line, step: RuntimeLookStep? = nil, errorSwap: Bool = false) -> AttributedString {
        var out = AttributedString()
        for seg in l {
            var part = AttributedString(fill(seg.text, step))
            let role = errorSwap && seg.role == "ok" ? "err" : seg.role
            if role == "pill" {
                part.backgroundColor = color(t.pillRole)
                part.foregroundColor = Color(hex: pal.bg)
                part.inlinePresentationIntent = .stronglyEmphasized
            } else {
                part.foregroundColor = color(role)
                if role == "b" { part.inlinePresentationIntent = .stronglyEmphasized }
            }
            out += part
        }
        return out
    }

    private func lineText(_ l: RuntimeLookTokens.Line, step: RuntimeLookStep? = nil, errorSwap: Bool = false) -> some View {
        Text(attributed(l, step: step, errorSwap: errorSwap))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Banner / header

    @ViewBuilder
    private var banner: some View {
        if t.app {
            HStack(spacing: 8) {
                ModelProviderMark(providerId: runtime.tool ?? runtime.id, size: 22)
                Text(runtime.name).font(.subheadline.weight(.bold))
                if let header = t.header {
                    Text(header).font(.caption).foregroundColor(color("mu"))
                }
                Spacer(minLength: 4)
                if let status = t.status {
                    HStack(spacing: 6) {
                        Circle().fill(color("ac2")).frame(width: 7, height: 7).accessibilityHidden(true)
                        Text(status).font(.caption).foregroundColor(color("mu"))
                    }
                }
            }
            .padding(.bottom, 10)
            .overlay(alignment: .bottom) { Rectangle().fill(line).frame(height: 1) }
            .accessibilityElement(children: .combine)
        } else if t.bigText != nil || !t.bigParts.isEmpty || !t.banner.isEmpty {
            let lines = VStack(alignment: .leading, spacing: 0) {
                bigWordmark
                ForEach(Array(t.banner.enumerated()), id: \.offset) { _, l in lineText(l) }
            }
            if t.bannerBox {
                lines
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(color(t.boxRole), lineWidth: 1))
                    .accessibilityElement(children: .combine)
            } else {
                lines.accessibilityElement(children: .combine)
            }
        }
    }

    @ViewBuilder
    private var bigWordmark: some View {
        let font = Font.system(.title, design: .monospaced).weight(.heavy)
        if !t.bigParts.isEmpty {
            Text(attributed(t.bigParts)).font(font).tracking(2).padding(.bottom, 6)
                .accessibilityHidden(true)
        } else if let text = t.bigText {
            let base = Text(text).font(font).tracking(2)
            Group {
                if t.bigGrad.count > 1 {
                    base.foregroundStyle(LinearGradient(colors: t.bigGrad.map { Color(hex: $0) },
                                                        startPoint: .leading, endPoint: .trailing))
                } else {
                    base.foregroundColor(t.bigGrad.first.map { Color(hex: $0) } ?? color("fg"))
                }
            }
            .padding(.bottom, 6)
            .accessibilityHidden(true)
        }
    }

    // MARK: Messages

    @ViewBuilder
    private func row(_ m: MessageRecord) -> some View {
        if m.role == "user" {
            userRow(m.content)
        } else {
            let steps = m.toolCalls.filter { !$0.isTodo }.map(RuntimeLookStep.init)
            VStack(alignment: .leading, spacing: 6) {
                if !steps.isEmpty {
                    if !t.app {
                        ForEach(Array(t.pre.enumerated()), id: \.offset) { _, l in lineText(l) }
                    }
                    stepsBlock(steps)
                }
                if m.isStreaming && m.content.isEmpty {
                    HStack(spacing: 8) {
                        Text(t.app ? "●" : "✻").foregroundColor(color("ac")).accessibilityHidden(true)
                        Text("\(m.busyLine ?? "Working")…").foregroundColor(color("mu"))
                    }
                    .accessibilityElement(children: .combine)
                    .accessibilityAddTraits(.updatesFrequently)
                }
                if !m.content.isEmpty {
                    answer(m.content, streaming: m.isStreaming)
                }
                if let error = m.error {
                    lineText([(error.title, "err")])
                }
            }
        }
    }

    private func markdown(_ s: String) -> AttributedString {
        (try? AttributedString(markdown: s, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(s)
    }

    private func answer(_ text: String, streaming: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            if !t.app, let glyph = t.answerGlyph {
                Text(glyph).foregroundColor(color(t.answerGlyphRole)).accessibilityHidden(true)
            }
            (Text(markdown(text)).foregroundColor(color(t.answerRole))
                + Text(streaming ? " ▍" : "").foregroundColor(color("ac")))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    @ViewBuilder
    private func userRow(_ text: String) -> some View {
        let g = t.prompt.isEmpty ? "" : "\(t.prompt) "
        if t.app || t.userStyle == "bubble" {
            HStack {
                Spacer(minLength: 48)
                Text(text)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 14, style: .continuous)
                        .fill(t.userFill.map { Color(hex: $0) } ?? color("ac").opacity(0.15)))
                    .fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityLabel("You: \(text)")
        } else {
            Group {
                switch t.userStyle {
                case "shade":
                    lineText([(g, "mu"), (text, "fg")]).padding(.horizontal, 8).padding(.vertical, 4).background(shade)
                case "bar":
                    lineText([(text, "fg")])
                        .padding(.horizontal, 10).padding(.vertical, 4)
                        .background(shade)
                        .overlay(alignment: .leading) { Rectangle().fill(color("ac2")).frame(width: 2) }
                case "color":
                    lineText([(g, "ac"), (text, "ac")])
                default:
                    lineText([(g, "mu"), (text, "mu")])
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("You: \(text)")
        }
    }

    // MARK: Steps

    @ViewBuilder
    private func stepsBlock(_ steps: [RuntimeLookStep]) -> some View {
        if t.app {
            VStack(spacing: 0) {
                ForEach(Array(steps.enumerated()), id: \.element.id) { i, s in
                    if i > 0 { Rectangle().fill(line).frame(height: 1) }
                    appStep(s)
                }
            }
            .overlay(RoundedRectangle(cornerRadius: 10).stroke(line, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: 10))
        } else {
            let body = VStack(alignment: .leading, spacing: 0) {
                ForEach(steps) { s in termStep(s) }
            }
            if t.stepBox {
                body.padding(.horizontal, 10).padding(.vertical, 6)
                    .overlay(RoundedRectangle(cornerRadius: 6).stroke(line, lineWidth: 1))
            } else {
                body
            }
        }
    }

    private func termStep(_ s: RuntimeLookStep) -> some View {
        let lines = t.steps[s.verb] ?? []
        let shown = s.status == .running ? Array(lines.prefix(1)) : lines
        return VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.offset) { _, l in
                lineText(l, step: s, errorSwap: s.status == .error)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private func appStep(_ s: RuntimeLookStep) -> some View {
        let label = (t.steps[s.verb]?.first ?? []).map { fill($0.text, s) }.joined()
        return HStack(spacing: 8) {
            Text(s.status == .error ? "✕" : s.status == .running ? "○" : "✓")
                .foregroundColor(color(s.status == .error ? "err" : "ok"))
                .accessibilityLabel(s.status == .error ? "Failed" : s.status == .running ? "Running" : "Done")
            Text(label)
                .font(.system(.caption, design: .monospaced))
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 4)
            if s.status != .running {
                Text(s.r).font(.caption).foregroundColor(color("mu")).lineLimit(1)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }

    // MARK: Composer

    private var placeholderText: String {
        let ph = fill(t.placeholder, nil)
        return ph.isEmpty ? "Message \(runtime.name)" : ph
    }

    private func send() {
        let s = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !s.isEmpty else { return }
        Haptics.selection()
        onSend(s)
        draft = ""
    }

    private var field: some View {
        TextField("", text: $draft, prompt: Text(placeholderText).foregroundColor(color("mu")), axis: .vertical)
            .lineLimit(1...5)
            .focused($focused)
            .foregroundColor(color("fg"))
            .tint(color("ac"))
            .submitLabel(.send)
            .onSubmit(send)
            .accessibilityLabel("Message \(runtime.name)")
    }

    private var sendButton: some View {
        Group {
            if isStreaming {
                Button(action: onStop) {
                    Image(systemName: "stop.fill").font(.system(size: 12, weight: .bold))
                }
                .accessibilityLabel("Stop")
            } else {
                Button(action: send) {
                    Image(systemName: "arrow.up").font(.system(size: 14, weight: .bold))
                }
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                .accessibilityLabel("Send message")
            }
        }
        .buttonStyle(.plain)
        .foregroundColor(t.app ? Color(hex: pal.bg) : color("ac"))
        .frame(width: 30, height: 30)
        .background {
            if t.app {
                RoundedRectangle(cornerRadius: t.sendSquare ? 8 : 15).fill(color("ac"))
            }
        }
        .frame(width: 44, height: 44)
        .contentShape(Rectangle())
    }

    @ViewBuilder
    private var composer: some View {
        if t.app {
            HStack(spacing: 8) {
                field
                sendButton
            }
            .padding(.leading, 14)
            .padding(.trailing, 2)
            .overlay(RoundedRectangle(cornerRadius: t.radius, style: .continuous).stroke(line, lineWidth: 1))
            .padding(.horizontal, 14)
            .padding(.bottom, 10)
            .padding(.top, 4)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                inputBox
                footerRow
            }
            .padding(.horizontal, 16)
            .padding(.top, 6)
            .padding(.bottom, 10)
        }
    }

    private var inputRow: some View {
        HStack(alignment: .center, spacing: 8) {
            if let glyph = t.inputGlyph ?? (t.prompt.isEmpty ? nil : t.prompt) {
                Text(glyph).foregroundColor(color(t.inputGlyphRole)).accessibilityHidden(true)
            }
            field
            sendButton
        }
    }

    @ViewBuilder
    private var inputBox: some View {
        switch t.input {
        case "round":
            inputRow.padding(.leading, 10)
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(color(t.inputBorderRole), lineWidth: 1))
        case "shade":
            inputRow.padding(.leading, 10).background(shade)
        case "bar":
            inputRow.padding(.leading, 10).background(shade)
                .overlay(alignment: .leading) { Rectangle().fill(color(t.inputBorderRole)).frame(width: 3) }
        case "line":
            inputRow
                .overlay(alignment: .top) { Rectangle().fill(line).frame(height: 1) }
                .overlay(alignment: .bottom) { Rectangle().fill(line).frame(height: 1) }
        case "rule":
            VStack(alignment: .leading, spacing: 0) {
                lineText([(String(repeating: "━", count: 14), "ac"), (isStreaming ? " working" : "", "mu")])
                    .accessibilityHidden(true)
                inputRow
            }
        default:
            inputRow
        }
    }

    @ViewBuilder
    private var footerRow: some View {
        let f = t.footer
        if f.l != nil || f.m != nil || f.r != nil {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .top, spacing: 10) {
                    if let l = f.l { Text(attributed(l)) }
                    Spacer(minLength: 0)
                    if let m = f.m { Text(attributed(m)) }
                    Spacer(minLength: 0)
                    if let r = f.r { Text(attributed(r)) }
                }
                .lineLimit(1)
                VStack(alignment: .leading, spacing: 2) {
                    if let l = f.l { lineText(l) }
                    if let m = f.m { lineText(m) }
                    if let r = f.r { lineText(r) }
                }
            }
            .font(.system(.caption2, design: .monospaced))
            .accessibilityElement(children: .combine)
        }
    }
}
