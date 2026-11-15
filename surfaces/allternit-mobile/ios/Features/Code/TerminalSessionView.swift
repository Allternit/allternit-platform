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
struct TerminalSessionView: View {
    @ObservedObject var session: PtySession
    @StateObject private var latch = TerminalCtrlLatch()

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
                TerminalSurfaceView(
                    attach: { view in
                        session.onOutput = { [weak view] text in view?.feed(text: text) }
                    },
                    onInput: { [weak session, weak latch] text in
                        guard let session else { return }
                        session.send(latch?.transform(text) ?? text)
                    },
                    onResize: { [weak session] cols, rows in session?.resize(cols: cols, rows: rows) }
                )
                .padding(8)
                if session.state == .connecting {
                    ProgressView()
                        .tint(TerminalPalette.secondary)
                }
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

    func makeCoordinator() -> Coordinator {
        Coordinator(onInput: onInput, onResize: onResize)
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

        init(onInput: @escaping @MainActor (String) -> Void, onResize: @escaping @MainActor (Int, Int) -> Void) {
            self.onInput = onInput
            self.onResize = onResize
        }
    }
}

extension TerminalSurfaceView.Coordinator: TerminalViewDelegate {
    func send(source: TerminalView, data: ArraySlice<UInt8>) {
        let text = String(decoding: data, as: UTF8.self)
        let onInput = self.onInput
        Task { @MainActor in onInput(text) }
    }

    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
        let onResize = self.onResize
        Task { @MainActor in onResize(newCols, newRows) }
    }

    func setTerminalTitle(source: TerminalView, title: String) {}

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
