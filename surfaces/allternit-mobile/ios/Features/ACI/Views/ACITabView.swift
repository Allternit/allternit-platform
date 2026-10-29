import SwiftUI

/// ACI mode, as the web phone draws it (allternit-ai
/// src/fabric-session/phone/screens/AciScreen.tsx): a ☰-only header, then
/// one of four views the drawer's ACI rail picks (`ACINavStore.tab`):
///
///   - `.agent` — the computer agent: the "Computer the agent uses" picker,
///     the browser landing (BrowserPane.tsx: Navigator's Deck, search/URL
///     bar, "Allternit Computer Agent" toggle, shortcuts), the in-app
///     browser, a live agent run, or a browser-stamped chat from the drawer.
///   - `.apps` — MCP Apps (`ACIMcpAppsView`).
///   - `.computers` — paired computers, phones, backups, workflows, keys.
///   - `.extensions` — ACI Extensions.
///
/// Pair / Teach suggestions sit at the top of the canvas until dismissed once.
struct ACITabView: View {
    @Binding var isSidebarOpen: Bool
    @Binding var selectedSessionId: String?

    /// A bot's `web.open` while the browser was closed lands here.
    @ObservedObject private var bridge = ACIBrowserBridge.shared
    @ObservedObject private var nav = ACINavStore.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared

    private enum Destination {
        case landing
        case browser(URL)
        case agent(String)
        case chat(sessionId: String?)
    }

    @State private var destination: Destination
    /// Set when the chat's own Back clears the selection — the resulting
    /// nil change must NOT be read as the sidebar's New-chat pill (which
    /// starts a FRESH browser chat instead of leaving chat entirely).
    @State private var suppressNextNilChange = false
    @State private var pairSeen = ACIOnceNudge.seen("pair-computer")
    @State private var teachSeen = ACIOnceNudge.seen("aci-teach")

    init(isSidebarOpen: Binding<Bool>, selectedSessionId: Binding<String?>) {
        self._isSidebarOpen = isSidebarOpen
        self._selectedSessionId = selectedSessionId
        if launchArgumentEnabled("extension") {
            self._destination = State(initialValue: .browser(URL(string: "https://news.ycombinator.com")!))
        } else {
            self._destination = State(initialValue: .landing)
        }
    }

    private var isLanding: Bool {
        if case .landing = destination { return true }
        return false
    }

    /// The browser, a run and a chat bring their own top bar.
    private var showsHeader: Bool { nav.tab != .agent || isLanding }

    private var showPairNudge: Bool {
        relay.availability != .unknown && relay.devices.isEmpty && !pairSeen
            && (nav.tab == .agent || nav.tab == .computers)
    }

    private var showTeachNudge: Bool {
        nav.tab == .agent && isLanding && !showPairNudge && !teachSeen
    }

    var body: some View {
        VStack(spacing: 0) {
            if showsHeader {
                ACIPhoneHeader {
                    isSidebarOpen.toggle()
                }
            }
            if nav.tab == .agent && isLanding && !relay.devices.isEmpty {
                ACIAgentTargetPicker()
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 10)
            }
            VStack(spacing: 0) {
                if showPairNudge && (nav.tab != .agent || isLanding) {
                    ACINudgeCard(
                        hero: .pair,
                        title: "Pair a computer",
                        message: "Connect Allternit Desktop with this account so the computer agent can work on your computer.",
                        closeLabel: "Dismiss pairing suggestion",
                        actionTitle: "Open Computers",
                        onClose: { rememberPair() },
                        onAction: {
                            rememberPair()
                            nav.open(.computers)
                        }
                    )
                }
                if showTeachNudge {
                    ACINudgeCard(
                        hero: .setup,
                        title: "Teach the agent a site",
                        message: "It's in the browser bar's ⋮ menu, under Teach. Record a site once in Allternit Desktop and the agent saves the path as a skill it can replay. On your phone you can see what's been taught and import a recording.",
                        closeLabel: "Dismiss Teach suggestion",
                        actionTitle: "Open Teach",
                        onClose: { rememberTeach() },
                        onAction: {
                            rememberTeach()
                            nav.openTeach()
                        }
                    )
                }
                Group {
                    switch nav.tab {
                    case .agent: agentPane
                    case .apps: ACIMcpAppsView()
                    case .computers: ACIComputersPane()
                    case .extensions: ACIExtensionsView()
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .background(Color("BgPrimary"))
        }
        .background(Color("BgPrimary"))
        .task { await relay.refresh() }
        .onAppear {
            // Landing on the tab with a browser session already selected
            // (tapped in the sidebar before switching over).
            if let sessionId = selectedSessionId {
                nav.open(.agent)
                destination = .chat(sessionId: sessionId)
            }
            openRequestedURL()
            #if DEBUG
            // `-open-browser-chat` (DEBUG only): open a fresh browser chat
            // for screenshot/send-path verification.
            if launchArgumentEnabled("open-browser-chat") {
                destination = .chat(sessionId: nil)
            }
            #endif
        }
        .onChange(of: bridge.requestedURL) { _, _ in
            openRequestedURL()
        }
        .onChange(of: selectedSessionId) { _, newValue in
            if let sessionId = newValue {
                // Sidebar history tap — open the browser chat on THIS
                // surface (was a dead tap that leaked into Chats).
                nav.open(.agent)
                destination = .chat(sessionId: sessionId)
            } else if case .chat = destination {
                if suppressNextNilChange {
                    suppressNextNilChange = false
                } else {
                    // Sidebar "New chat" pill (it only nils the selection):
                    // a fresh browser chat on this surface.
                    destination = .chat(sessionId: nil)
                }
            }
        }
        .onDisappear {
            // Leaving the tab: drop a chat selection so a browser-stamped
            // session never leaks into the Chats surface.
            if case .chat = destination {
                suppressNextNilChange = true
                selectedSessionId = nil
                destination = .landing
            }
        }
        .fullScreenCover(isPresented: $nav.isTeachOpen) {
            SiteAPIsView()
        }
    }

    // MARK: - Agent

    @ViewBuilder
    private var agentPane: some View {
        switch destination {
        case .landing:
            ACIBrowserLanding(
                onOpen: { url in destination = .browser(url) },
                onRunGoal: { goal in destination = .agent(goal) }
            )
        case .browser(let url):
            ACIWebBrowserView(initialURL: url, onExit: backToLanding, onStartAgentRun: { goal in
                destination = .agent(goal)
            })
        case .agent(let goal):
            ACIAgentRunView(goal: goal, onExit: backToLanding)
        case .chat(let sessionId):
            BrowserChatView(sessionId: sessionId, onBack: {
                suppressNextNilChange = true
                selectedSessionId = nil
                backToLanding()
            })
        }
    }

    /// Opens the browser at the URL a bot asked for (the bridge waits for
    /// the browser view to attach, then drives it).
    private func openRequestedURL() {
        guard let url = bridge.requestedURL else { return }
        bridge.requestedURL = nil
        nav.open(.agent)
        destination = .browser(url)
    }

    private func backToLanding() {
        destination = .landing
    }

    private func rememberPair() {
        ACIOnceNudge.remember("pair-computer")
        withAnimation(.easeOut(duration: 0.2)) { pairSeen = true }
    }

    private func rememberTeach() {
        ACIOnceNudge.remember("aci-teach")
        withAnimation(.easeOut(duration: 0.2)) { teachSeen = true }
    }
}

// MARK: - Target picker

/// "Computer the agent uses" (AciScreen.tsx `targetPicker`): the last one
/// picked, else the first online. Runs go through the relay to the active
/// computer, so picking one pins the relay to it.
private struct ACIAgentTargetPicker: View {
    @ObservedObject private var relay = RuntimeRelayStore.shared

    var body: some View {
        Menu {
            Picker("Computer the agent uses", selection: Binding(
                get: { relay.activeRuntimeId ?? "" },
                set: { relay.select($0.isEmpty ? nil : $0) }
            )) {
                ForEach(relay.devices) { device in
                    Text(device.isOnline ? device.displayName : "\(device.displayName) (offline)")
                        .tag(device.id)
                        .selectionDisabled(!device.isOnline)
                }
            }
        } label: {
            HStack(spacing: 8) {
                Text(relay.activeDevice?.displayName ?? "Choose a computer")
                    .font(.system(size: 16))
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(1)
                Spacer(minLength: 4)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("TextSecondary"))
            }
            .padding(.horizontal, 16)
            .frame(height: 36)
            .background(Color("FillNeutral"))
            .clipShape(Capsule())
        }
        .accessibilityLabel("Computer the agent uses")
    }
}

// MARK: - Browser landing

/// The browser's landing (BrowserPane.tsx `BrowserPaneWrapper`): dotted
/// canvas, the Allternit mark, a random title and tagline, the search/URL
/// bar, the "Allternit Computer Agent" toggle and the shortcut tiles with
/// Customize. With the agent on, the bar takes a goal and starts a run.
private struct ACIBrowserLanding: View {
    let onOpen: (URL) -> Void
    let onRunGoal: (String) -> Void

    @StateObject private var shortcuts = ACIShortcutsStore.shared
    @State private var input = ""
    @State private var agentActive = false
    @State private var editMode = false
    @State private var adding = false
    @State private var newLabel = ""
    @State private var newURL = ""
    @State private var newIcon = ""
    @State private var title: String
    @State private var tagline: String
    @FocusState private var inputFocused: Bool

    init(onOpen: @escaping (URL) -> Void, onRunGoal: @escaping (String) -> Void) {
        self.onOpen = onOpen
        self.onRunGoal = onRunGoal
        self._title = State(initialValue: ACIBrowserLanding.titles.randomElement() ?? "Navigator's Deck")
        self._tagline = State(initialValue: ACIBrowserLanding.taglines.randomElement() ?? "Your Web, Amplified")
    }

    private static let titles = ["Navigator's Deck", "Browse & Build", "The Web Layer", "Allternit Explorer", "Portal Online", "Web Surface"]
    private static let taglines = ["AI-Powered Browsing", "Navigate with Intelligence", "Your Web, Amplified", "Explore Fearlessly", "Browse. Automate. Build."]

    var body: some View {
        ScrollView {
            VStack(spacing: 0) {
                Image("MatrixLogo")
                    .resizable()
                    .scaledToFit()
                    .frame(width: 84, height: 84)
                    .padding(.bottom, 24)
                    .accessibilityLabel("Allternit")
                Text(title)
                    .font(.system(size: 32, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.center)
                    .padding(.bottom, 8)
                HStack(spacing: 12) {
                    Rectangle().fill(Color("BorderStrong")).frame(width: 32, height: 1)
                    Text(tagline.uppercased())
                        .font(.system(size: 14))
                        .tracking(1.1)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                    Rectangle().fill(Color("BorderStrong")).frame(width: 32, height: 1)
                }
                .padding(.bottom, 32)

                searchBar
                    .padding(.bottom, 24)
                agentToggle
                    .padding(.bottom, 32)

                HStack {
                    Spacer()
                    Button(editMode ? "Done" : "Customize") { editMode.toggle() }
                        .font(.system(size: 12))
                        .foregroundColor(editMode ? Color("TextPrimary") : Color("TextSecondary"))
                        .underline()
                        .buttonStyle(.plain)
                        .padding(.horizontal, 4)
                }
                .padding(.bottom, 8)
                shortcutGrid
            }
            .frame(maxWidth: 560)
            .padding(.horizontal, 24)
            .padding(.vertical, 48)
            .frame(maxWidth: .infinity)
        }
        .scrollDismissesKeyboard(.interactively)
        .background(alignment: .topLeading) { ACIDotGrid().ignoresSafeArea(edges: .bottom) }
    }

    /// The search/URL form: 56 pt neutral field, accent search button.
    private var searchBar: some View {
        HStack(spacing: 8) {
            TextField(agentActive ? "Describe a task for the Computer Agent…" : "Search the web or enter a URL…", text: $input)
                .font(.system(size: 18))
                .foregroundColor(Color("TextPrimary"))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(agentActive ? .default : .URL)
                .submitLabel(.go)
                .focused($inputFocused)
                .onSubmit(submit)
                .accessibilityLabel("Search the web or enter a URL…")
            Button(action: submit) {
                Image(systemName: agentActive ? "play.fill" : "magnifyingglass")
                    .font(.system(size: 18, weight: .medium))
                    .foregroundColor(Color("OnAccent"))
                    .frame(width: 36, height: 36)
                    .background(Color("AccentPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: 8))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(agentActive ? "Start agent run" : "Search")
        }
        .padding(.leading, 24)
        .padding(.trailing, 12)
        .frame(height: 56)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(
            RoundedRectangle(cornerRadius: 12)
                .stroke(inputFocused ? Color("TextPrimary") : Color("BorderSubtle"), lineWidth: 1)
        )
    }

    private var agentToggle: some View {
        Button {
            agentActive.toggle()
            inputFocused = true
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "puzzlepiece.fill")
                    .font(.system(size: 12))
                Text("Allternit Computer Agent: \(agentActive ? "Active" : "Off")")
                    .font(.system(size: 12))
            }
            .foregroundColor(agentActive ? Color("TextPrimary") : Color("TextSecondary"))
            .padding(.horizontal, 14)
            .padding(.vertical, 6)
            .background(agentActive ? Color("BorderStrong") : Color("FillNeutral"))
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(agentActive ? .isSelected : [])
    }

    private var shortcutGrid: some View {
        let columns = Array(repeating: GridItem(.flexible(), spacing: 12), count: min(shortcuts.items.count + 1, 4))
        return LazyVGrid(columns: columns, spacing: 12) {
            ForEach(shortcuts.items) { item in
                Button {
                    if !editMode, let url = URL(string: item.url) { onOpen(url) }
                } label: {
                    VStack(spacing: 8) {
                        favicon(item)
                        Text(item.label)
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 16)
                    .padding(.horizontal, 6)
                    .background(Color("FillNeutral"))
                    .clipShape(RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .overlay(alignment: .topTrailing) {
                    if editMode {
                        Button { shortcuts.remove(item.id) } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundColor(.white)
                                .frame(width: 20, height: 20)
                                .background(Theme.statusError)
                                .clipShape(Circle())
                        }
                        .buttonStyle(.plain)
                        .offset(x: 6, y: -6)
                        .accessibilityLabel("Remove \(item.label)")
                    }
                }
            }
            if adding {
                addForm
            } else {
                Button { adding = true } label: {
                    VStack(spacing: 8) {
                        Image(systemName: "plus")
                            .font(.system(size: 18))
                            .frame(height: 48)
                        Text("Add").font(.system(size: 12))
                    }
                    .foregroundColor(Color("TextTertiary"))
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 16)
                    .overlay(
                        RoundedRectangle(cornerRadius: 12)
                            .strokeBorder(Color("BorderSubtle"), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                    )
                }
                .buttonStyle(.plain)
            }
        }
    }

    private var addForm: some View {
        VStack(spacing: 6) {
            formField("Label", text: $newLabel)
            formField("URL", text: $newURL, url: true)
            HStack(spacing: 6) {
                formField("Emoji", text: $newIcon).frame(width: 44)
                Button("Save") { saveShortcut() }
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundColor(Color("OnAccent"))
                    .frame(maxWidth: .infinity, minHeight: 26)
                    .background(Color("AccentPrimary"))
                    .clipShape(RoundedRectangle(cornerRadius: 6))
                Button("Cancel") { adding = false }
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextSecondary"))
                    .padding(.horizontal, 6)
            }
            .buttonStyle(.plain)
        }
        .padding(10)
        .background(Color("FillNeutral"))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
    }

    private func formField(_ placeholder: String, text: Binding<String>, url: Bool = false) -> some View {
        TextField(placeholder, text: text)
            .font(.system(size: 12))
            .foregroundColor(Color("TextPrimary"))
            .keyboardType(url ? .URL : .default)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .padding(.horizontal, 8)
            .frame(height: 26)
            .background(Color("BgPanel"))
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .overlay(RoundedRectangle(cornerRadius: 6).stroke(Color("BorderSubtle"), lineWidth: 1))
            .accessibilityLabel("Shortcut \(placeholder.lowercased())")
    }

    /// Google's favicon service (`getFaviconUrl`), the globe when it fails.
    private func favicon(_ item: ACIShortcut) -> some View {
        let host = URL(string: item.url)?.host ?? ""
        return ZStack {
            Circle().fill(Color("BorderSubtle"))
            AsyncImage(url: URL(string: "https://www.google.com/s2/favicons?domain=\(host)&sz=64")) { phase in
                if let image = phase.image {
                    image.resizable().scaledToFit()
                } else {
                    Image(systemName: "globe")
                        .font(.system(size: 18))
                        .foregroundColor(Color("TextSecondary").opacity(0.5))
                }
            }
            .frame(width: 28, height: 28)
        }
        .frame(width: 48, height: 48)
    }

    private func saveShortcut() {
        let label = newLabel.trimmingCharacters(in: .whitespaces)
        var url = newURL.trimmingCharacters(in: .whitespaces)
        guard !label.isEmpty, !url.isEmpty else { return }
        if !url.lowercased().hasPrefix("http://") && !url.lowercased().hasPrefix("https://") { url = "https://\(url)" }
        shortcuts.add(label: label, url: url, icon: newIcon)
        newLabel = ""
        newURL = ""
        newIcon = ""
        adding = false
    }

    private func submit() {
        let value = input.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return }
        inputFocused = false
        if agentActive {
            onRunGoal(value)
        } else {
            onOpen(ACIWebInput.resolve(value))
        }
        input = ""
    }
}

/// The landing's dot pattern: 1 pt dots every 22 pt, offset 11.
private struct ACIDotGrid: View {
    var body: some View {
        Canvas { context, size in
            var path = Path()
            var y: CGFloat = 11
            while y < size.height {
                var x: CGFloat = 11
                while x < size.width {
                    path.addEllipse(in: CGRect(x: x - 1, y: y - 1, width: 2, height: 2))
                    x += 22
                }
                y += 22
            }
            context.fill(path, with: .color(Color("TextPrimary").opacity(0.18)))
        }
        .accessibilityHidden(true)
    }
}

// MARK: - Shortcuts

struct ACIShortcut: Identifiable, Codable, Equatable, Sendable {
    let id: String
    let label: String
    let url: String
    var icon: String?
}

/// The landing's shortcut tiles (browserShortcuts.store.ts), kept on the device.
@MainActor
final class ACIShortcutsStore: ObservableObject {
    static let shared = ACIShortcutsStore()

    @Published private(set) var items: [ACIShortcut] {
        didSet {
            if let data = try? JSONEncoder().encode(items) {
                UserDefaults.standard.set(data, forKey: Self.key)
            }
        }
    }

    private static let key = "allternit.aci-browser-shortcuts"

    /// DEFAULT_SHORTCUTS.
    private static let defaults: [ACIShortcut] = [
        ACIShortcut(id: "default-google", label: "Google", url: "https://www.google.com", icon: "🔍"),
        ACIShortcut(id: "default-github", label: "GitHub", url: "https://github.com", icon: "🐙"),
        ACIShortcut(id: "default-hn", label: "Hacker News", url: "https://news.ycombinator.com", icon: "📰"),
    ]

    private init() {
        if let data = UserDefaults.standard.data(forKey: Self.key),
           let saved = try? JSONDecoder().decode([ACIShortcut].self, from: data) {
            items = saved
        } else {
            items = Self.defaults
        }
    }

    func add(label: String, url: String, icon: String) {
        items.append(ACIShortcut(id: "shortcut-\(Int(Date().timeIntervalSince1970 * 1000))", label: label, url: url, icon: icon.isEmpty ? nil : icon))
    }

    func remove(_ id: String) {
        items.removeAll { $0.id == id }
    }
}
