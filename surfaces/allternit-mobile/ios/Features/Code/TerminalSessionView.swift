import SwiftUI
import UIKit
import SwiftTerm

/// The phone terminal's dark palette (phone.css "Phone Code terminal:
/// real-terminal look"): one dark surface, mono cells, the accessory keys
/// on a slightly lighter strip.
enum TerminalPalette {
    static let background = Color(hex: "#0D0F12")
    static let foreground = Color(hex: "#E4E6EA")
    static let secondary = Color(hex: "#9AA0A8")
    static let strip = Color(hex: "#14171B")
    static let key = Color(hex: "#1D2126")
    static let keyPressed = Color(hex: "#2A2F36")
    static let line = Color(hex: "#23272D")
    static let keyBorder = Color(hex: "#2C3137")
    static let pressedBorder = Color(hex: "#3A4048")
}

/// The Ctrl latch (PhoneCodeTerminal.tsx `useCtrlLatch`): the next keyboard
/// input becomes its control byte, like a physical Ctrl key.
@MainActor
final class TerminalCtrlLatch: ObservableObject {
    @Published private(set) var isOn = false

    func toggle() { isOn.toggle() }

    /// A–Z and [\]^_ become control bytes; `?` becomes DEL.
    func transform(_ data: String) -> String {
        guard isOn else { return data }
        isOn = false
        return String(String.UnicodeScalarView(data.unicodeScalars.map { scalar -> UnicodeScalar in
            if scalar == "?" { return "\u{7f}" }
            let upper = Character(scalar).uppercased().unicodeScalars.first ?? scalar
            if ("A"..."Z").contains(upper) || "[\\]^_".unicodeScalars.contains(upper) {
                return UnicodeScalar(upper.value & 31) ?? scalar
            }
            return scalar
        }))
    }
}

/// Esc / Tab / Ctrl / arrows under the terminal (PhoneCodeTerminal.tsx
/// `AccessoryKeys`). The same bytes go to a shell pty or a bot's pane.
struct TerminalAccessoryKeys: View {
    @ObservedObject var latch: TerminalCtrlLatch
    let send: (String) -> Void

    private static let keys: [(label: String, name: String, data: String)] = [
        ("Esc", "Escape", "\u{1b}"), ("Tab", "Tab", "\t"),
        ("←", "Arrow left", "\u{1b}[D"), ("↑", "Arrow up", "\u{1b}[A"),
        ("↓", "Arrow down", "\u{1b}[B"), ("→", "Arrow right", "\u{1b}[C"),
    ]

    var body: some View {
        HStack(spacing: 2) {
            ForEach(Self.keys.prefix(2), id: \.name) { key in keyButton(key) }
            Button(action: { latch.toggle() }) {
                keyLabel("Ctrl", pressed: latch.isOn)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Control")
            .accessibilityAddTraits(latch.isOn ? .isSelected : [])
            ForEach(Self.keys.dropFirst(2), id: \.name) { key in keyButton(key) }
        }
        .padding(4)
        .background(TerminalPalette.strip)
        .overlay(alignment: .top) { Rectangle().fill(TerminalPalette.line).frame(height: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Terminal accessory keys")
    }

    private func keyButton(_ key: (label: String, name: String, data: String)) -> some View {
        Button(action: { send(key.data) }) {
            keyLabel(key.label, pressed: false)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(key.name)
    }

    private func keyLabel(_ text: String, pressed: Bool) -> some View {
        Text(text)
            .font(.system(size: 15, design: .monospaced))
            .foregroundColor(TerminalPalette.foreground)
            .frame(maxWidth: .infinity, minHeight: 44)
            .background(pressed ? TerminalPalette.keyPressed : TerminalPalette.key)
            .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

/// One shell tile: a pty on the computer's gizzi-code, drawn by SwiftTerm
/// with the accessory keys under it. Detaches on disappear; the pty keeps
/// running server-side and replays its scrollback on the next attach.
///
/// Agent mode (the Code terminal, UnifiedTerminal.tsx `onAgentFrame`): when a
/// coding agent (Claude Code, Codex, …) runs in the shell, its own input box is
/// hidden below the visible frame (the pty gets those rows on top of the
/// visible ones, so the agent keeps its room) and `AgentComposerBar` drives it.
/// An unrecognised screen shows the full terminal at its normal size.
struct TerminalSessionView: View {
    @ObservedObject var session: PtySession
    /// Read the screen for a coding agent and drive it from a native composer.
    var agentMode = false
    /// Called with the parsed frame whenever it changes (nil = plain shell).
    var onAgentFrame: (@MainActor (AgentFrame?) -> Void)? = nil
    @StateObject private var latch = TerminalCtrlLatch()
    @StateObject private var reader = AgentFrameReader()

    var body: some View {
        VStack(spacing: 0) {
            switch session.state {
            case .failed(let message):
                statusLine(message.isEmpty ? "Terminal disconnected. Open a new terminal to reconnect." : message)
            case .exited:
                statusLine("Terminal disconnected. Open a new terminal to reconnect.")
            case .connecting, .live:
                EmptyView()
            }
            ZStack {
                if agentMode {
                    // Rows from the frame's `hideFrom` down sit below the clip.
                    GeometryReader { geo in
                        surface
                            .frame(width: geo.size.width, height: geo.size.height + reader.hiddenHeight, alignment: .top)
                    }
                    .clipped()
                    .padding(8)
                } else {
                    surface
                        .padding(8)
                }
                if session.state == .connecting {
                    ProgressView()
                        .tint(TerminalPalette.secondary)
                }
            }
            if agentMode, let frame = reader.frame {
                AgentComposerBar(frame: frame, session: session)
            }
            TerminalAccessoryKeys(latch: latch) { data in session.send(data) }
        }
        .background(TerminalPalette.background)
        .task {
            await session.start()
        }
        .onDisappear {
            session.stop()
        }
        .onChange(of: reader.frame) { _, frame in
            onAgentFrame?(frame)
        }
    }

    private var surface: some View {
        TerminalSurfaceView(
            attach: { [agentMode, weak reader] view in
                if agentMode { reader?.view = view }
                session.onOutput = { [weak view, weak reader] text in
                    view?.feed(text: text)
                    if agentMode { reader?.schedule() }
                }
            },
            onInput: { [weak session, weak latch] text in
                guard let session else { return }
                session.send(latch?.transform(text) ?? text)
            },
            onResize: { [weak session] cols, rows in session?.resize(cols: cols, rows: rows) },
            onTitle: { [weak reader] title in reader?.title = title }
        )
    }

    /// `.phone-terminal-status`: the tile's error, in the status colours.
    private func statusLine(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 13))
            .foregroundColor(Theme.statusError)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(8)
            .background(Theme.statusError.opacity(0.12))
            .accessibilityAddTraits(.isStaticText)
    }
}

// MARK: - Agent frame

/// Reads a coding agent's frame from the SwiftTerm screen after output
/// settles, and how many rows of its input box to hide (UnifiedTerminal.tsx
/// `parseAgentScreen`, same timing and caps).
@MainActor
final class AgentFrameReader: ObservableObject {
    @Published private(set) var frame: AgentFrame?
    /// Rows of the agent's own input box drawn below the visible frame.
    @Published private(set) var hiddenRows = 0
    /// The row height the hidden rows were sized with.
    @Published private(set) var cellHeight: CGFloat = 0
    weak var view: TerminalView?
    var title = ""
    private var pending = (rows: 0, seen: 0)
    private var timer: Task<Void, Never>?

    /// Extra height under the clip that holds the hidden rows.
    var hiddenHeight: CGFloat { hiddenRows > 0 && cellHeight > 0 ? CGFloat(hiddenRows) * cellHeight : 0 }

    /// After an output batch: read the screen 120 ms later (at most ~8 times a second).
    func schedule() {
        guard timer == nil else { return }
        timer = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            self?.read()
        }
    }

    private func read() {
        timer = nil
        guard let view else { return }
        // Scrolled into history: leave it as it is.
        if view.canScroll && view.scrollPosition < 1 { return }
        let terminal = view.getTerminal()
        let rowCount = terminal.rows
        let rows = (0..<rowCount).map { Self.text(of: terminal.getLine(row: $0)) }
        let next = AgentFrameParser.parse(rows, title: title)
        if next != frame { frame = next }
        // Hide the agent's box: rows from hideFrom down, capped so a misread can never hide much.
        let want = next.map { max(0, min(rowCount - $0.hideFrom, 14, rowCount / 2)) } ?? 0
        if want == hiddenRows {
            pending = (want, 0)
            return
        }
        // Change only after two reads agree, so a redraw mid-frame never makes it jump.
        pending = (want, pending.rows == want ? pending.seen + 1 : 1)
        if pending.seen < 2 {
            schedule()
            return
        }
        if rowCount > 0, view.bounds.height > 0 { cellHeight = view.bounds.height / CGFloat(rowCount) }
        hiddenRows = want
    }

    /// One visible row as xterm's `translateToString(true)` gives it: trailing
    /// blanks trimmed, one character per wide glyph, never-written cells as spaces.
    static func text(of line: BufferLine?) -> String {
        guard let line else { return "" }
        return line.translateToString(trimRight: true, skipNullCellsFollowingWide: true)
            .replacingOccurrences(of: "\u{0}", with: " ")
    }
}

// MARK: - SwiftTerm surface

/// A SwiftTerm `TerminalView` in the phone terminal's look (15pt mono,
/// 8pt padding, Terminal.app's dark profile). Output is pushed imperatively
/// through the view `attach` hands out; input and grid size flow back
/// through the closures. Nothing syncs in `updateUIView`, so SwiftUI passes
/// never disturb the emulator.
struct TerminalSurfaceView: UIViewRepresentable {
    var fontSize: CGFloat = 15
    let attach: @MainActor (TerminalView) -> Void
    let onInput: @MainActor (String) -> Void
    let onResize: @MainActor (Int, Int) -> Void
    /// The terminal title (OSC 0/2), e.g. an agent naming itself.
    var onTitle: (@MainActor (String) -> Void)? = nil

    func makeCoordinator() -> Coordinator {
        Coordinator(onInput: onInput, onResize: onResize, onTitle: onTitle)
    }

    func makeUIView(context: Context) -> TerminalView {
        let terminalView = TerminalView(frame: .zero)
        terminalView.terminalDelegate = context.coordinator
        terminalView.font = UIFont.monospacedSystemFont(ofSize: fontSize, weight: .regular)
        terminalView.nativeBackgroundColor = UIColor(TerminalPalette.background)
        terminalView.nativeForegroundColor = UIColor(TerminalPalette.foreground)
        terminalView.backgroundColor = UIColor(TerminalPalette.background)
        // The phone terminal draws its own key row under the cells.
        terminalView.inputAccessoryView = nil
        attach(terminalView)
        return terminalView
    }

    func updateUIView(_ uiView: TerminalView, context: Context) {}

    /// Non-isolated like SandboxedArtifactWebView.Coordinator: SwiftTerm is a
    /// pre-concurrency module, so the delegate requirements are non-isolated
    /// witnesses; calls into main-actor state hop explicitly.
    final class Coordinator: NSObject, @unchecked Sendable {
        let onInput: @MainActor (String) -> Void
        let onResize: @MainActor (Int, Int) -> Void
        let onTitle: (@MainActor (String) -> Void)?

        init(onInput: @escaping @MainActor (String) -> Void, onResize: @escaping @MainActor (Int, Int) -> Void,
             onTitle: (@MainActor (String) -> Void)?) {
            self.onInput = onInput
            self.onResize = onResize
            self.onTitle = onTitle
        }
    }
}

extension TerminalSurfaceView.Coordinator: TerminalViewDelegate {
    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        let text = String(decoding: data, as: UTF8.self)
        // Replaying scrollback makes the emulator answer old cursor/device
        // queries. Those replies are not typing; sent to the shell they show up
        // as "^[[12;23R" (UI feedback K3, same guard as the web terminal).
        if Self.isTerminalReport(text) { return }
        let onInput = self.onInput
        Task { @MainActor in onInput(text) }
    }

    /// Cursor position (`ESC[r;cR`), device attributes (`ESC[?..c`, `ESC[>..c`),
    /// status (`ESC[0n`, `ESC[?..n`), mode reports (`ESC[?..;..$y`) and OSC
    /// colour replies (`ESC]1x;rgb:...`): replies, never keystrokes.
    static func isTerminalReport(_ text: String) -> Bool {
        guard text.hasPrefix("\u{1B}[") || text.hasPrefix("\u{1B}]") else { return false }
        let patterns = [
            "^(\u{1B}\\[[0-9]+;[0-9]+R)+$",
            "^\u{1B}\\[[?>][0-9;]*c$",
            "^\u{1B}\\[[?]?[0-9;]*n$",
            "^\u{1B}\\[[?]?[0-9;]*\\$y$",
            "^\u{1B}\\]1[0-9];rgb:[0-9a-fA-F/]+(\u{07}|\u{1B}\\\\)$",
        ]
        return patterns.contains { text.range(of: $0, options: .regularExpression) != nil }
    }

    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        let onResize = self.onResize
        Task { @MainActor in onResize(newCols, newRows) }
    }

    func setTerminalTitle(source: TerminalView, title: String) {
        guard let onTitle else { return }
        Task { @MainActor in onTitle(title) }
    }

    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}

    func scrolled(source: TerminalView, position: Double) {}

    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}

    func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {
        guard let url = URL(string: link), let scheme = url.scheme, scheme.hasPrefix("http") else { return }
        Task { @MainActor in UIApplication.shared.open(url) }
    }

    func clipboardCopy(source: TerminalView, content: Data) {
        let text = String(decoding: content, as: UTF8.self)
        Task { @MainActor in UIPasteboard.general.string = text }
    }
}
