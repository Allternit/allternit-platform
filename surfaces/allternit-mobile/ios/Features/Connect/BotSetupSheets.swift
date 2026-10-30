import SwiftUI
import UniformTypeIdentifiers

// -----------------------------------------------------------------------------
// "Make a new bot" and "Import a bot" — the new-bot and import paths of
// allternit-ai `src/components/bots/look/useBotSetup.ts`, shared by Meet your
// bots (who leads) and the Bots rail. A new bot gets a job (a template, or
// custom) and a name; a templated job also starts its first task in the bot's
// main chat, like the web's `useBotTemplate(…, { runFirstTask: true })`.
// Import reads a team file (openmaus.team v1/v2, openmaus.package, or the
// simple `{ name, bots: [...] }` manifest — lib/bots/team-import.ts) from Files
// or a link and makes one bot per member, or a bot package (.zip with a
// bot.json — lib/bots/bot-import.ts, see BotZipImport.swift) and makes that bot
// with its memory, skills, tasks and docs copied to the computer.
// -----------------------------------------------------------------------------

/// What a setup sheet made.
struct BotSetupOutcome: Equatable {
    var botIds: [String]
    /// The first bot made became the main bot (the store does that when there is none).
    var mainBotId: String?
    var summary: String
}

enum BotSetupMaker {
    /// web defaultBotFormData → buildCreateBotPayload for a bot with no template.
    @MainActor
    static func plainBody(name: String, description: String, systemPrompt: String, tagline: String,
                          welcome: String, color: String, avatar: JSONValue?, makeMain: Bool) -> [String: JSONValue] {
        let model = MeetTemplateCreate.currentModel()
        var handle = BotModeStore.slugify(name)
        if handle.count < 3 { handle = "\(handle.isEmpty ? "my" : handle)-bot" }
        var profile: [String: JSONValue] = [
            "displayName": .string(name), "tagline": .string(tagline), "welcomeMessage": .string(welcome),
            "starterPrompts": .array([]), "accentColor": .string(color), "groupChatEnabled": .bool(true),
            "botCategory": .string("custom"),
        ]
        if makeMain { profile["chiefOfStaff"] = .bool(true) }
        var config: [String: JSONValue] = ["isBot": .bool(true), "botProfile": .object(profile)]
        if let avatar { config["avatar"] = avatar }
        return [
            "name": .string(handle), "description": .string(description), "type": .string("worker"),
            "model": .string(model.model), "provider": .string(model.provider),
            "capabilities": .array([]), "tools": .array([]), "max_iterations": .number(10), "temperature": .number(0.7),
            "system_prompt": .string(systemPrompt), "trust_tier": .string("standard"),
            "enabled_modes": .array([.string("chat")]), "is_bot": .bool(true),
            "harness_config": .object(["mode": .string("cloud")]),
            "bot_profile": .object(profile), "config": .object(config),
        ]
    }

    /// Whether the account has a main bot yet (the first bot made becomes it).
    static func needsMain() async -> Bool {
        guard let roster = try? await MeetAgentsAPI.roster() else { return false }
        return !roster.contains(where: \.isMain)
    }
}

// MARK: - New bot

struct NewBotSheet: View {
    /// Shows "Start from a team": a project with its bots and plan laid out
    /// (the Bots home moved its templates row here).
    var onBrowseTemplates: (() -> Void)? = nil
    var onCreated: (BotSetupOutcome) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @State private var job = "researcher"
    @State private var name = ""
    @State private var lookId: String?
    @State private var busy = false
    @State private var error: String?

    private struct Job { let id: String; let label: String; let template: String? }
    /// useBotSetup JOBS.
    private static let jobs: [Job] = [
        Job(id: "researcher", label: "Researcher", template: "research-analyst"), Job(id: "inbox", label: "Inbox", template: "inbox-triage"),
        Job(id: "sales", label: "Sales", template: "lead-qualifier"), Job(id: "writer", label: "Writer", template: "content-writer"),
        Job(id: "custom", label: "Custom", template: nil),
    ]

    private var catalog: MeetBotCatalog { .shared }
    private var template: MeetBotTemplate? {
        Self.jobs.first(where: { $0.id == job })?.template.flatMap { catalog.template($0) }
    }
    private var defaultName: String { template?.name ?? "New bot" }
    private var finalName: String {
        let n = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return n.isEmpty ? defaultName : n
    }

    var body: some View {
        NavigationStack {
            Form {
                if !auth.isSignedIn {
                    Section { Text("Sign in to make a bot.").foregroundColor(Color("TextSecondary")) }
                }
                Section("Job") {
                    Picker("Job", selection: $job) {
                        ForEach(Self.jobs, id: \.id) { Text($0.label).tag($0.id) }
                    }
                    .pickerStyle(.segmented)
                    Text(template?.tagline ?? "A bot with no set job. Tell it what to do in its chat.")
                        .font(.footnote).foregroundColor(Color("TextSecondary"))
                }
                Section("Name") {
                    TextField(defaultName, text: $name).textInputAutocapitalization(.words)
                }
                Section("Look") {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 60), spacing: 10)], spacing: 10) {
                        ForEach(catalog.looks) { look in
                            Button { lookId = lookId == look.id ? nil : look.id } label: {
                                VStack(spacing: 4) {
                                    MeetTemplateTile(color: look.field, spriteURL: look.spriteURL, size: 44, live: lookId == look.id, label: look.id)
                                        .overlay(RoundedRectangle(cornerRadius: 13, style: .continuous)
                                            .stroke(lookId == look.id ? Color("AccentPrimary") : .clear, lineWidth: 2).padding(-3))
                                    Text(look.label).font(.caption2).foregroundColor(Color("TextPrimary")).lineLimit(1)
                                }
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel(look.label)
                            .accessibilityAddTraits(lookId == look.id ? .isSelected : [])
                        }
                    }
                    .padding(.vertical, 4)
                    Text(lookId == nil ? "No look picked: it keeps the job’s own look." : "The bot wears this look.")
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                }
                if let error {
                    Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
                }
                if let onBrowseTemplates {
                    Section {
                        Button {
                            dismiss()
                            // After this sheet is gone, so the next one can present.
                            DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { onBrowseTemplates() }
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Browse team templates")
                                    .foregroundColor(Color("TextPrimary"))
                                Text("A project with its bots and plan already laid out")
                                    .font(.footnote)
                                    .foregroundColor(Color("TextSecondary"))
                            }
                        }
                        .disabled(busy)
                    } header: {
                        Text("Or start from a team")
                    }
                }
                Section {
                    Label("Nothing is sent, posted or paid without your approval.", systemImage: "lock")
                        .font(.caption).foregroundColor(Color("TextSecondary"))
                }
            }
            .navigationTitle("Make a new bot")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) }
                ToolbarItem(placement: .confirmationAction) {
                    if busy { ProgressView() } else {
                        Button("Create") { Task { await create() } }.disabled(!auth.isSignedIn)
                    }
                }
            }
            .interactiveDismissDisabled(busy)
        }
    }

    private func create() async {
        busy = true
        error = nil
        defer { busy = false }
        do {
            let makeMain = await BotSetupMaker.needsMain()
            let avatar = lookId.flatMap { id in catalog.looks.first { $0.id == id }?.avatar }
            let botId: String
            if let template {
                botId = try await MeetTemplateCreate.create(template, name: finalName, makeMain: makeMain)
                if let avatar, let bot = (try? await MeetAgentsAPI.roster())?.first(where: { $0.id == botId }) {
                    try? await MeetMainBot.dressBot(bot, name: nil, avatar: avatar)
                }
                // runFirstTask: the template's first job, as the person's own message in the bot's main chat.
                if let record = try? await AgentHubStore.shared.refreshAgent(id: botId),
                   let session = try? await BotModeStore.shared.mainChatSessionId(for: record) {
                    try? await BotModeClient.shared.sendUserMessage(sessionId: session, text: template.firstTask.prompt)
                }
            } else {
                botId = try await MeetAgentsAPI.create(body: BotSetupMaker.plainBody(
                    name: finalName, description: "", systemPrompt: "", tagline: "", welcome: "",
                    color: "#B08D6E", avatar: avatar, makeMain: makeMain))
            }
            AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
            onCreated(BotSetupOutcome(botIds: [botId], mainBotId: makeMain ? botId : nil,
                                      summary: "\(finalName) created · Job: \(template?.name ?? "Custom")"))
            dismiss()
        } catch {
            self.error = "Couldn’t create the bot: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}

// MARK: - Import

/// One bot from a team file.
struct ImportedBotSpec: Equatable {
    var name: String
    var description: String
    var systemPrompt: String
    var tagline: String
    var welcome: String
    var color: String
}

enum TeamFileParser {
    struct Team: Equatable {
        var name: String
        var bots: [ImportedBotSpec]
    }

    /// The manifest formats the web's team import reads, cut to what a phone makes: one bot per member.
    static func parse(_ data: Data) throws -> Team {
        guard let json = try? JSONDecoder().decode(JSONValue.self, from: data), json.jObject != nil else {
            throw MeetFinishError("This isn’t a team file. Pick a .json team file exported from Allternit.")
        }
        func s(_ v: JSONValue?, _ key: String) -> String { v?.at(key)?.jString?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "" }
        func member(_ m: JSONValue) -> ImportedBotSpec? {
            let name = s(m, "displayName").isEmpty ? s(m, "name") : s(m, "displayName")
            guard !name.isEmpty else { return nil }
            let title = s(m, "title")
            let desc = s(m, "description")
            let color = s(m.at("appearance"), "color").isEmpty ? (s(m, "accentColor").isEmpty ? "#B08D6E" : s(m, "accentColor")) : s(m.at("appearance"), "color")
            return ImportedBotSpec(name: name, description: desc.isEmpty ? title : desc,
                                   systemPrompt: s(m, "systemPrompt"), tagline: s(m, "tagline").isEmpty ? title : s(m, "tagline"),
                                   welcome: s(m, "welcomeMessage"), color: color)
        }
        let format = s(json, "format")
        var team: Team
        if format == "openmaus.package", let pkg = json.at("package") {
            team = Team(name: s(pkg, "name"), bots: (pkg.at("agents")?.jArray ?? []).compactMap(member))
        } else if format == "openmaus.team", let t = json.at("team") {
            team = Team(name: s(t, "name"), bots: (t.at("members")?.jArray ?? []).compactMap(member))
        } else if let bots = json.at("bots")?.jArray {
            team = Team(name: s(json, "name"), bots: bots.compactMap(member))
        } else if !s(json, "name").isEmpty || !s(json, "displayName").isEmpty, let one = member(json) {
            team = Team(name: one.name, bots: [one])
        } else {
            throw MeetFinishError("This file has no bots in it.")
        }
        if team.bots.isEmpty { throw MeetFinishError("This team file has no bots in it.") }
        if team.bots.count > 200 { throw MeetFinishError("A team file can have at most 200 bots.") }
        if team.name.isEmpty { team.name = "Imported team" }
        return team
    }
}

struct ImportBotSheet: View {
    var onImported: (BotSetupOutcome) -> Void

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @State private var source = "file"
    @State private var link = ""
    @State private var picking = false
    @State private var team: TeamFileParser.Team?
    @State private var package: BotZipPackage?
    @State private var reading = false
    @State private var warnings: [String] = []
    @State private var busy = false
    @State private var error: String?
    @State private var made: [String] = []
    @State private var failures: [String] = []

    var body: some View {
        NavigationStack {
            Form {
                if !auth.isSignedIn {
                    Section { Text("Sign in to import bots.").foregroundColor(Color("TextSecondary")) }
                }
                Section {
                    Picker("Source", selection: $source) {
                        Text("From a file").tag("file")
                        Text("From a link").tag("url")
                    }
                    .pickerStyle(.segmented)
                    if source == "file" {
                        Button("Choose a team file or bot package") { picking = true }.disabled(busy || reading)
                        if reading {
                            HStack(spacing: 8) { ProgressView(); Text("Reading the file…").foregroundColor(Color("TextSecondary")) }
                        }
                    } else {
                        TextField("https://github.com/…/team.json or bot.zip", text: $link)
                            .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("Read the file") { Task { await fetchLink() } }
                            .disabled(busy || URL(string: link.trimmingCharacters(in: .whitespaces))?.scheme?.hasPrefix("http") != true)
                    }
                } footer: {
                    Text("Bring in bots you already have: an Allternit team file (.json), or a bot package (.zip) exported from Allternit. A package’s memory, skills, tasks and docs are copied to your computer. Bots from a team file come in chat-only; you approve anything a bot sends.")
                }
                if let team {
                    Section("\(team.name) · \(team.bots.count) bot\(team.bots.count == 1 ? "" : "s")") {
                        ForEach(Array(team.bots.enumerated()), id: \.offset) { _, b in
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(b.name).font(.subheadline.weight(.semibold))
                                    if !b.tagline.isEmpty { Text(b.tagline).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(2) }
                                }
                                Spacer()
                                if made.contains(b.name) {
                                    Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                                } else if failures.contains(where: { $0.hasPrefix(b.name + ":") }) {
                                    Image(systemName: "exclamationmark.circle.fill").foregroundColor(Theme.statusError)
                                }
                            }
                        }
                    }
                }
                if let package {
                    let p = package.preview
                    Section("Bot package") {
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(p.name).font(.subheadline.weight(.semibold))
                                Spacer()
                                if !made.isEmpty {
                                    Image(systemName: "checkmark.circle.fill").foregroundColor(Theme.statusSuccess)
                                }
                            }
                            if !p.description.isEmpty {
                                Text(p.description).font(.caption).foregroundColor(Color("TextSecondary")).lineLimit(4)
                            }
                        }
                        if let category = p.category, !category.isEmpty {
                            LabeledContent("Category", value: category.capitalized)
                        }
                        LabeledContent("Includes", value: p.contents.isEmpty ? "Just the bot’s settings" : p.contents.joined(separator: " · "))
                        LabeledContent("Files", value: "\(p.fileCount)")
                        ForEach(p.warnings, id: \.self) { Text($0).font(.footnote).foregroundColor(Color("TextSecondary")) }
                    }
                }
                if !warnings.isEmpty {
                    Section("Imported with notes") {
                        ForEach(warnings, id: \.self) { Text($0).font(.footnote).foregroundColor(Color("TextSecondary")) }
                    }
                }
                if !failures.isEmpty {
                    Section("Not imported") {
                        ForEach(failures, id: \.self) { Text($0).font(.footnote).foregroundColor(Theme.statusError) }
                    }
                }
                if let error {
                    Section { Text(error).font(.footnote).foregroundColor(Theme.statusError) }
                }
            }
            .navigationTitle("Import a bot")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) }
                ToolbarItem(placement: .confirmationAction) {
                    if busy { ProgressView() } else if team != nil || (package != nil && made.isEmpty) {
                        Button(failures.isEmpty ? "Import" : "Retry") { Task { await runImport() } }.disabled(!auth.isSignedIn)
                    }
                }
            }
            .fileImporter(isPresented: $picking, allowedContentTypes: [.json, .plainText, .zip]) { result in
                switch result {
                case .success(let url): read(url)
                case .failure(let e): error = e.localizedDescription
                }
            }
            .interactiveDismissDisabled(busy)
        }
    }

    private func load(_ data: Data) async {
        error = nil
        made = []
        failures = []
        warnings = []
        team = nil
        package = nil
        if ZipArchive.looksLikeZip(data) {
            reading = true
            defer { reading = false }
            do {
                package = try await Task.detached(priority: .userInitiated) { try BotZipPackage.load(data) }.value
            } catch {
                self.error = error.localizedDescription
            }
            return
        }
        do { team = try TeamFileParser.parse(data) } catch { self.error = error.localizedDescription }
    }

    private func read(_ url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        if let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size > ZipArchive.maxTotalBytes {
            team = nil
            package = nil
            error = "This file is too big. Bot packages can hold at most 50 MB of files."
            return
        }
        do {
            let data = try Data(contentsOf: url)
            Task { await load(data) }
        } catch {
            self.error = "Couldn’t read the file: \(error.localizedDescription)"
        }
    }

    private func fetchLink() async {
        guard var url = URL(string: link.trimmingCharacters(in: .whitespaces)) else { return }
        // A GitHub page link → its raw file (team-import importTeamFromGitHubUrl).
        if url.host == "github.com", url.path.contains("/blob/") {
            url = URL(string: "https://raw.githubusercontent.com" + url.path.replacingOccurrences(of: "/blob/", with: "/")) ?? url
        }
        busy = true
        defer { busy = false }
        do {
            let (data, response) = try await URLSession.shared.data(from: url)
            if let http = response as? HTTPURLResponse, !(200...299).contains(http.statusCode) {
                error = "Couldn’t fetch the file (\(http.statusCode))."
                return
            }
            await load(data)
        } catch {
            self.error = "Couldn’t fetch the file: \(error.localizedDescription)"
        }
    }

    private func runImport() async {
        if let package {
            await runPackageImport(package)
            return
        }
        guard let team else { return }
        busy = true
        error = nil
        failures = []
        defer { busy = false }
        var ids: [String] = []
        var main: String?
        var needsMain = await BotSetupMaker.needsMain()
        for b in team.bots where !made.contains(b.name) {
            do {
                let id = try await MeetAgentsAPI.create(body: BotSetupMaker.plainBody(
                    name: b.name, description: b.description, systemPrompt: b.systemPrompt, tagline: b.tagline,
                    welcome: b.welcome, color: b.color, avatar: nil, makeMain: needsMain))
                if needsMain { main = id; needsMain = false }
                ids.append(id)
                made.append(b.name)
            } catch {
                failures.append("\(b.name): \(FriendlyErrorMessage.from(error.localizedDescription))")
            }
        }
        AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
        guard !ids.isEmpty else {
            if failures.isEmpty { dismiss() }
            return
        }
        onImported(BotSetupOutcome(botIds: ids, mainBotId: main,
                                   summary: "Imported \(team.name) · \(ids.count) bot\(ids.count == 1 ? "" : "s") added"))
        if failures.isEmpty { dismiss() }
    }

    /// One bot from a .zip package (web importBotFromZip).
    private func runPackageImport(_ package: BotZipPackage) async {
        busy = true
        error = nil
        warnings = []
        defer { busy = false }
        let makeMain = await BotSetupMaker.needsMain()
        do {
            let result = try await BotZipImporter.run(package, makeMain: makeMain)
            made = [package.preview.name]
            warnings = result.warnings
            AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
            let notes = result.warnings.isEmpty ? "" : " · \(result.warnings.count) file\(result.warnings.count == 1 ? "" : "s") not copied"
            onImported(BotSetupOutcome(botIds: [result.botId], mainBotId: makeMain ? result.botId : nil,
                                       summary: "Imported \(package.preview.name) from a bot package\(notes)"))
            if result.warnings.isEmpty { dismiss() }
        } catch let failure as BotZipError {
            error = failure.message
        } catch {
            self.error = "Couldn’t import the bot: \(FriendlyErrorMessage.from(error.localizedDescription))"
        }
    }
}
