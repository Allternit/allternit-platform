import SwiftUI

// -----------------------------------------------------------------------------
// Create bot and Edit bot on iPhone, the layout of allternit-ai
// create-bot/CreateBotWizard.tsx (phone sheet) and steps/PhoneQuickCreate.tsx:
// close + title, "Name your bot", the Look editor, More options, one button at
// the bottom. Edit bot is the same sheet prefilled with the fields of
// EditBotForm.tsx (IdentityStep, JobStep, Owns).
// -----------------------------------------------------------------------------

/// Create bot. The caller gets the new bot's id and opens its chat.
struct NewBotSheet: View {
    /// "Start from a team": a project with its bots and plan laid out.
    var onBrowseTemplates: (() -> Void)? = nil
    var onCreated: (BotSetupOutcome) -> Void

    @StateObject private var model = BotFormModel(mode: .create)

    var body: some View {
        BotFormSheet(model: model, onBrowseTemplates: onBrowseTemplates, onCreated: onCreated)
    }
}

/// Edit bot: the same form, prefilled.
struct EditBotSheet: View {
    var onSaved: () -> Void = {}

    @StateObject private var model: BotFormModel

    init(botId: String, onSaved: @escaping () -> Void = {}) {
        self.onSaved = onSaved
        _model = StateObject(wrappedValue: BotFormModel(mode: .edit(botId: botId)))
    }

    var body: some View {
        BotFormSheet(model: model, onSaved: onSaved)
    }
}

private struct BotFormSheet: View {
    @ObservedObject var model: BotFormModel
    var onBrowseTemplates: (() -> Void)? = nil
    var onCreated: ((BotSetupOutcome) -> Void)? = nil
    var onSaved: (() -> Void)? = nil

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @ObservedObject private var look: BotLookModel
    @ObservedObject private var models = ModelStore.shared
    @State private var moreOpen = false
    @State private var advancedOpen = false
    @State private var vendorOpen = false
    @State private var modelPickerOpen = false
    @State private var starterDraft = ""
    @State private var ownDraft = ""

    init(model: BotFormModel, onBrowseTemplates: (() -> Void)? = nil,
         onCreated: ((BotSetupOutcome) -> Void)? = nil, onSaved: (() -> Void)? = nil) {
        _model = ObservedObject(wrappedValue: model)
        self.onBrowseTemplates = onBrowseTemplates
        self.onCreated = onCreated
        self.onSaved = onSaved
        _look = ObservedObject(wrappedValue: model.look)
    }

    private var busy: Bool { model.saving || look.busy }
    private var bannerError: String? { model.error ?? look.error }

    var body: some View {
        VStack(spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if !auth.isSignedIn {
                        Text(model.isEditing ? "Sign in to edit this bot." : "Sign in to make a bot.")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextSecondary"))
                    }
                    if let bannerError { errorBanner(bannerError) }
                    if model.loading {
                        ProgressView().frame(maxWidth: .infinity, minHeight: 200)
                    } else if let loadError = model.loadError {
                        FriendlyStateView(style: .error, icon: "exclamationmark.triangle", title: "Couldn't load this bot",
                                          message: loadError, actionTitle: "Retry", action: { Task { await model.loadBot() } })
                    } else {
                        nameField
                        BotLookEditor(model: look)
                        if model.isEditing { editFields } else { moreOptions }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 16)
            }
            .scrollDismissesKeyboard(.interactively)
            footer
        }
        .background(Color("BgPrimary").ignoresSafeArea())
        .interactiveDismissDisabled(busy)
        .task {
            if model.isEditing { await model.loadBot() } else { await model.loadRuntimeOptions() }
        }
        .sheet(isPresented: $vendorOpen) {
            NavigationStack {
                VendorConnectView { firstBotId, mainBotId in
                    vendorOpen = false
                    AgentHubStore.shared.fetchAgentsIfNeeded(force: true)
                    let ids = firstBotId.map { [$0] } ?? []
                    onCreated?(BotSetupOutcome(botIds: ids, mainBotId: mainBotId, summary: "Vendor bot connected"))
                    dismiss()
                }
                .navigationTitle("Bring a vendor’s bot")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Close") { vendorOpen = false } }
                }
            }
        }
        .sheet(isPresented: $modelPickerOpen) { modelPicker }
    }

    // MARK: Chrome

    private var header: some View {
        HStack(spacing: 10) {
            Button { dismiss() } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 14, weight: .bold))
                    .foregroundColor(Color("TextPrimary"))
                    .frame(width: 40, height: 40)
                    .background(Circle().fill(Color("BgPanel")))
                    .overlay(Circle().stroke(Color("BorderSubtle")))
                    .frame(minWidth: 44, minHeight: 44)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(busy)
            .accessibilityLabel("Close")
            Text(model.isEditing ? "Edit bot" : "Create bot")
                .font(.system(size: 17, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14)
        .padding(.top, 12)
        .padding(.bottom, 4)
    }

    private var footer: some View {
        Button {
            Task {
                if model.isEditing {
                    if await model.saveEdit() {
                        onSaved?()
                        dismiss()
                    }
                } else if let outcome = await model.create() {
                    onCreated?(outcome)
                    dismiss()
                }
            }
        } label: {
            Text(primaryLabel)
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("BgPrimary"))
                .frame(maxWidth: .infinity, minHeight: 48)
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("TextPrimary")))
                .opacity(primaryEnabled ? 1 : 0.4)
        }
        .buttonStyle(.plain)
        .disabled(!primaryEnabled)
        .padding(.horizontal, 16)
        .padding(.top, 12)
        .padding(.bottom, 12)
        .overlay(alignment: .top) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private var primaryLabel: String {
        if model.isEditing { return model.saving ? "Saving…" : "Save bot" }
        return model.saving ? "Creating…" : "Create"
    }

    private var primaryEnabled: Bool { auth.isSignedIn && model.canSave }

    private func errorBanner(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 13))
            Text(message).font(.system(size: 13))
            Spacer(minLength: 0)
        }
        .foregroundColor(Theme.statusError)
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Theme.statusError.opacity(0.1)))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).stroke(Theme.statusError.opacity(0.3)))
        .accessibilityAddTraits(.isStaticText)
    }

    // MARK: Name

    private var nameField: some View {
        VStack(spacing: 4) {
            TextField("Name your bot", text: $model.displayName)
                .textInputAutocapitalization(.words)
                .autocorrectionDisabled()
                .multilineTextAlignment(.center)
                .font(.system(size: 18))
                .foregroundColor(Color("TextPrimary"))
                .padding(.horizontal, 16)
                .frame(height: 48)
                .background(RoundedRectangle(cornerRadius: 16, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).stroke(Color("BorderSubtle")))
                .accessibilityLabel("Bot name")
                .onChange(of: model.displayName) { _, _ in model.nameChanged() }
            if model.isEditing {
                Text(model.handle.isEmpty ? "" : "Handle: @\(model.handle)")
                    .font(.system(size: 12))
                    .foregroundColor(Color("TextTertiary"))
            }
        }
    }

    // MARK: More options (create)

    private var moreOptions: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                moreOpen.toggle()
            } label: {
                HStack(spacing: 4) {
                    Text("More options").foregroundColor(Color("TextSecondary"))
                    Text("· accent, role, brain, computer")
                        .foregroundColor(Color("TextTertiary"))
                        .lineLimit(1)
                    Image(systemName: "chevron.down")
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .rotationEffect(.degrees(moreOpen ? 180 : 0))
                }
                .font(.system(size: 13.5))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(moreOpen ? "Expanded" : "Collapsed")
            if moreOpen {
                VStack(alignment: .leading, spacing: 24) {
                    accentRow(BotFormData.createAccents)
                    roleField
                    jobSection
                    computerSection
                    intelligenceSection
                    advancedSection
                    Button { vendorOpen = true } label: {
                        Label("Bring a vendor’s bot instead", systemImage: "powerplug")
                            .font(.system(size: 13))
                            .foregroundColor(Color("TextPrimary"))
                            .padding(.horizontal, 14)
                            .frame(minHeight: 44)
                            .background(Capsule().fill(Color("FillNeutral")))
                    }
                    .buttonStyle(.plain)
                    if let onBrowseTemplates {
                        Button {
                            dismiss()
                            // After this sheet is gone, so the next one can present.
                            DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) { onBrowseTemplates() }
                        } label: {
                            Label("Browse team templates", systemImage: "square.grid.2x2")
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextPrimary"))
                                .padding(.horizontal, 14)
                                .frame(minHeight: 44)
                                .background(Capsule().fill(Color("FillNeutral")))
                        }
                        .buttonStyle(.plain)
                        .disabled(busy)
                    }
                }
            }
        }
    }

    private func accentRow(_ colors: [String]) -> some View {
        ProjectChipWrap(spacing: 10) {
            ForEach(colors, id: \.self) { color in
                Button { model.setAccent(color) } label: {
                    Circle()
                        .fill(Color(hex: color))
                        .frame(width: 28, height: 28)
                        .overlay(Circle().stroke(Color("TextPrimary"), lineWidth: model.accent == color ? 2 : 0).padding(-3))
                        .frame(width: 44, height: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Colour \(color)")
                .accessibilityAddTraits(model.accent == color ? .isSelected : [])
            }
        }
    }

    /// RoleField: "What it does", template chips, the description, "Draft instructions from this".
    private var roleField: some View {
        VStack(alignment: .leading, spacing: 6) {
            label("What it does")
            ProjectChipWrap(spacing: 6) {
                ForEach(MeetBotCatalog.shared.templates) { template in
                    let on = model.selectedTemplateId == template.id
                    Button { model.pick(template: template) } label: {
                        Text(template.name)
                            .font(.system(size: 12))
                            .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                            .padding(.horizontal, 10)
                            .frame(minHeight: 32)
                            .overlay(Capsule().stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            textArea("e.g. Works out unit economics and pricing, and runs the monthly close. Reads Stripe and our sheets. Never moves money or emails customers.",
                     text: $model.description, lines: 3)
            Button { Task { await model.draftFromDescription() } } label: {
                HStack(spacing: 4) {
                    if model.describing { ProgressView().controlSize(.mini) } else { Image(systemName: "sparkle").font(.system(size: 11)) }
                    Text(model.describing ? "Drafting…" : "Draft instructions from this")
                }
                .font(.system(size: 12, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .padding(.horizontal, 10)
                .frame(minHeight: 32)
                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle")))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(model.describing || model.description.trimmingCharacters(in: .whitespacesAndNewlines).count < 12)
            .opacity(model.description.trimmingCharacters(in: .whitespacesAndNewlines).count < 12 ? 0.4 : 1)
            note(model.describeError ?? (model.systemPrompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                ? "We draft its instructions and tools from this. You can edit both later."
                : "Instructions and tools are drafted. Edit them under More options, or later in the bot's Config."))
        }
    }

    /// JobStep.
    private var jobSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Job & Tools", "What this bot does, and what it is allowed to use. Saved with the bot when you create it.")
            HStack {
                label("Job instructions")
                Spacer()
                Button { Task { await model.refine() } } label: {
                    HStack(spacing: 4) {
                        if model.refining { ProgressView().controlSize(.mini) }
                        Text(model.refining ? "Working…" : "Refine from my description")
                    }
                    .font(.system(size: 12, weight: .medium))
                    .foregroundColor(Color("TextSecondary"))
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(model.refining)
            }
            textArea("Your job: …\n\nYou do:\n- …\n\nYou do not:\n- …", text: $model.systemPrompt, lines: 8)
            if let refineError = model.refineError {
                Text(refineError).font(.system(size: 12)).foregroundColor(Theme.statusError)
            } else {
                note("This becomes the bot's system prompt. State the job plainly — what it does and what it does not do. Optional: the platform adds identity and computer context at runtime.")
            }
            label("Tools").padding(.top, 6)
            ProjectChipWrap(spacing: 6) {
                ForEach(BotFormData.tools, id: \.id) { tool in
                    let on = model.allowedTools.contains(tool.id)
                    Button { model.toggleTool(tool.id) } label: {
                        HStack(spacing: 4) {
                            if on { Image(systemName: "checkmark").font(.system(size: 10, weight: .bold)) }
                            Text(tool.label)
                        }
                        .font(.system(size: 12))
                        .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                        .padding(.horizontal, 10)
                        .frame(minHeight: 32)
                        .background(Capsule().fill(on ? Color("FillNeutral") : .clear))
                        .overlay(Capsule().stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            let extra = model.allowedTools.filter { id in !BotFormData.tools.contains { $0.id == id } }.count
            note(model.allowedTools.isEmpty
                ? "No tools — chat only"
                : (extra > 0 ? "Also on from the template: \(extra) more tool\(extra == 1 ? "" : "s")." : "What this bot is allowed to use. Saved with the bot when you create it."))
        }
    }

    /// ComputerRuntimeStep: Persistent desktop.
    private var computerSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Computer & Runtime", "Your bot gets its own always-on desktop in Computer Cloud. Files, tools, and browser state persist between sessions.")
            Toggle(isOn: $model.desktopEnabled) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Persistent desktop").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    Text("Provisioned when you create the bot, bound to it by id. The same computer is reused every time you open the bot.")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .padding(12)
            .background(card)
            if model.desktopEnabled {
                label("Size")
                note("Bigger desktops cost more disk on the host — each bot's allocation counts against how many bots the machine can hold. Start small and resize later if the bot outgrows it.")
                VStack(spacing: 6) {
                    ForEach(BotFormData.desktopPresets) { preset in
                        let on = model.desktopPresetId == preset.id
                        Button { model.desktopPresetId = preset.id } label: {
                            HStack {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(preset.label).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                                    Text(preset.detail).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                                }
                                Spacer()
                                if on { Image(systemName: "checkmark").font(.system(size: 12, weight: .bold)).foregroundColor(Color("TextPrimary")) }
                            }
                            .padding(.horizontal, 12)
                            .frame(minHeight: 52)
                            .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                }
                if let quota = model.quota {
                    if !quota.allowed {
                        Text(quota.reason ?? "Desktop quota reached — this bot may not be able to provision.")
                            .font(.system(size: 12))
                            .foregroundColor(Theme.statusError)
                    } else if let limit = quota.activeLimit {
                        note("Your desktop quota: \(quota.active) of \(limit) desktops in use"
                             + (quota.monthlyLimit.map { " · \(quota.monthlyMinutes) of \($0) min this month" } ?? ""))
                    }
                }
                note("The desktop starts provisioning when you hit Create bot. You can watch its status here and on the bot card.")
            }
        }
    }

    /// ComputerRuntimeStep: Intelligence (BotBrainBindFields, knowledge brain, model, provider, harness).
    private var intelligenceSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("Intelligence", "Pick how this bot runs and the model it uses.")
            label("Brain")
            note("How this bot runs. Separate from the Gizzi knowledge brain.")
            VStack(spacing: 6) {
                ForEach(BotFormData.brainModes, id: \.id) { mode in
                    let on = model.brainMode == mode.id
                    Button { model.brainMode = mode.id } label: {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(mode.label).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                                Text(mode.hint).font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                            }
                            Spacer()
                            if on { Image(systemName: "checkmark").font(.system(size: 12, weight: .bold)).foregroundColor(Color("TextPrimary")) }
                        }
                        .padding(12)
                        .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            if !model.isEditing && model.brainMissing {
                Text(model.brainMode.isEmpty ? "Pick a brain to create this bot" : "Pick a model below to create this bot")
                    .font(.system(size: 12))
                    .foregroundColor(Theme.statusError)
            }
            if model.brainMode == "native_harness" {
                label("Native harness")
                ProjectChipWrap(spacing: 6) {
                    ForEach(BotFormData.nativeHarnesses, id: \.id) { harness in
                        let on = model.harnessId == harness.id
                        Button { model.harnessId = harness.id } label: {
                            Text(harness.label)
                                .font(.system(size: 13, weight: .medium))
                                .foregroundColor(on ? Color("TextPrimary") : Color("TextSecondary"))
                                .padding(.horizontal, 12)
                                .frame(minHeight: 44)
                                .background(RoundedRectangle(cornerRadius: 12, style: .continuous).fill(Color("BgPanel")))
                                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(on ? Color("TextPrimary") : Color("BorderSubtle")))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                }
                label("Native session id")
                field("Leave empty to bind on first start", text: $model.nativeSessionId)
            }
            if model.brainMode == "uhp_harness" {
                label("UHP harness id")
                field("e.g. uhp-claude-desktop-01", text: $model.uhpHarnessId)
                if model.uhpMissing {
                    Text("Required to start this bot").font(.system(size: 12)).foregroundColor(Theme.statusError)
                } else {
                    note("Bound when you save. Does not fall back to cloud.")
                }
            }
            label("Gizzi knowledge").padding(.top, 6)
            if model.brainsLoading {
                ProgressView().frame(maxWidth: .infinity, minHeight: 40)
            } else if model.knowledgeBrains.isEmpty {
                note("No Gizzi knowledge brains found. Select a platform model below.")
            } else {
                menuRow(value: model.knowledgeBrains.first { $0.id == model.knowledgeBrainId }?.label ?? "Platform model") {
                    Button("Platform model") { model.knowledgeBrainId = "" }
                    ForEach(model.knowledgeBrains) { brain in
                        Button(brain.label) { model.knowledgeBrainId = brain.id }
                    }
                }
            }
            label("Model").padding(.top, 6)
            Button { modelPickerOpen = true } label: {
                HStack {
                    Text(modelLabel).font(.system(size: 14)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                    Spacer()
                    Image(systemName: "chevron.up.chevron.down").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
                }
                .padding(.horizontal, 12)
                .frame(minHeight: 44)
                .background(card)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            label("Provider").padding(.top, 6)
            menuRow(value: BotFormData.providers.first { $0.id == model.provider }?.label ?? model.provider) {
                ForEach(BotFormData.providers, id: \.id) { provider in
                    Button(provider.label) { model.provider = provider.id }
                }
            }
            label("Harness").padding(.top, 6)
            menuRow(value: BotFormData.harnessModes.first { $0.id == model.harnessMode }?.label ?? model.harnessMode) {
                ForEach(BotFormData.harnessModes, id: \.id) { harness in
                    Button(harness.label) { model.harnessMode = harness.id }
                }
            }
        }
    }

    private var modelLabel: String {
        if model.modelId.isEmpty { return "Pick a brain" }
        let id = "\(model.provider)/\(model.modelId)"
        return models.models.first { $0.id == id || $0.id == model.modelId }?.name ?? model.modelId
    }

    private var modelPicker: some View {
        NavigationStack {
            List {
                if models.isLoading && models.models.isEmpty {
                    ProgressView().frame(maxWidth: .infinity)
                } else if let loadError = models.loadError, models.models.isEmpty {
                    Text(loadError).font(.caption).foregroundColor(Theme.statusWarning)
                } else {
                    ForEach(models.providers, id: \.provider) { group in
                        Section(group.provider) {
                            ForEach(group.models) { option in
                                Button {
                                    let parts = option.id.split(separator: "/", maxSplits: 1).map(String.init)
                                    model.modelId = parts.count == 2 ? parts[1] : option.id
                                    model.provider = parts.count == 2 ? parts[0] : option.provider
                                    modelPickerOpen = false
                                } label: {
                                    HStack {
                                        Text(option.name).foregroundColor(Color("TextPrimary"))
                                        Spacer()
                                        if option.id == "\(model.provider)/\(model.modelId)" { Image(systemName: "checkmark") }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("Model")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { modelPickerOpen = false } }
            }
        }
    }

    /// ComputerRuntimeStep: Advanced.
    private var advancedSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Button { advancedOpen.toggle() } label: {
                HStack(spacing: 6) {
                    Image(systemName: advancedOpen ? "chevron.down" : "chevron.right").font(.system(size: 12))
                    Text("Advanced").font(.system(size: 13, weight: .medium))
                }
                .foregroundColor(Color("TextSecondary"))
                .frame(minHeight: 44)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if advancedOpen {
                Stepper("Max iterations: \(model.maxIterations)", value: $model.maxIterations, in: 1...50)
                    .font(.system(size: 14))
                note("Tool calls a bot may take on one task.")
                VStack(alignment: .leading, spacing: 2) {
                    Text("Temperature: \(String(format: "%.1f", model.temperature))").font(.system(size: 14))
                    Slider(value: $model.temperature, in: 0...2, step: 0.1).accessibilityLabel("Temperature")
                }
                Toggle(isOn: $model.voiceEnabled) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Enable voice").font(.system(size: 14, weight: .medium))
                        Text("Speak responses with text-to-speech.").font(.system(size: 12)).foregroundColor(Color("TextSecondary"))
                    }
                }
                if model.voiceEnabled {
                    if model.voicesLoading {
                        note("Loading voices…")
                    } else {
                        menuRow(value: model.voices.first { $0.id == model.voiceId }?.label ?? "Default voice") {
                            ForEach(model.voices) { voice in
                                Button(voice.label) { model.voiceId = voice.id }
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: Edit fields (EditBotForm: IdentityStep, JobStep, Owns)

    private var editFields: some View {
        VStack(alignment: .leading, spacing: 20) {
            VStack(alignment: .leading, spacing: 6) {
                label("Tagline")
                field("Short description shown on the bot card", text: $model.tagline)
            }
            VStack(alignment: .leading, spacing: 6) {
                label("Category")
                menuRow(value: BotFormData.categories.first { $0.id == model.category }?.label ?? "Custom") {
                    ForEach(BotFormData.categories, id: \.id) { category in
                        Button(category.label) { model.category = category.id }
                    }
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                label("Accent color")
                accentRow(BotFormData.editAccents)
            }
            VStack(alignment: .leading, spacing: 6) {
                label("Purpose / description")
                textArea("What does this bot do?", text: $model.description, lines: 3)
            }
            VStack(alignment: .leading, spacing: 6) {
                label("Welcome message")
                textArea("Hi! I'm your new bot. How can I help?", text: $model.welcome, lines: 2)
            }
            starterPromptsField
            jobSection
            ownsField
        }
    }

    private var starterPromptsField: some View {
        VStack(alignment: .leading, spacing: 6) {
            label("Starter prompts")
            note("Suggestions")
            ProjectChipWrap(spacing: 6) {
                ForEach(BotFormData.suggestions(for: model.category), id: \.self) { suggestion in
                    let unavailable = model.starterPrompts.contains(suggestion) || model.starterPrompts.count >= BotFormData.starterPromptMax
                    Button { model.addStarter(suggestion) } label: {
                        Text(suggestion)
                            .font(.system(size: 12))
                            .lineLimit(1)
                            .foregroundColor(Color("TextSecondary"))
                            .padding(.horizontal, 10)
                            .frame(minHeight: 32)
                            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(Color("BorderSubtle")))
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .disabled(unavailable)
                    .opacity(unavailable ? 0.5 : 1)
                }
            }
            chips(model.starterPrompts) { item in model.starterPrompts.removeAll { $0 == item } }
            HStack(spacing: 8) {
                field("Add quick-start prompts users can click…", text: $starterDraft)
                addButton(disabled: starterDraft.trimmingCharacters(in: .whitespaces).isEmpty) {
                    model.addStarter(starterDraft)
                    starterDraft = ""
                }
            }
            note("Max 5 starter prompts.")
        }
    }

    private var ownsField: some View {
        VStack(alignment: .leading, spacing: 6) {
            label("Owns")
            if model.owns.isEmpty {
                note("Nothing yet. Add what this bot is responsible for.")
            } else {
                chips(model.owns) { item in model.owns.removeAll { $0 == item } }
            }
            HStack(spacing: 8) {
                field("e.g. invoices, the support inbox", text: $ownDraft)
                addButton(disabled: ownDraft.trimmingCharacters(in: .whitespaces).isEmpty) {
                    model.addOwn(ownDraft)
                    ownDraft = ""
                }
            }
        }
    }

    private func chips(_ items: [String], remove: @escaping (String) -> Void) -> some View {
        ProjectChipWrap(spacing: 6) {
            ForEach(items, id: \.self) { item in
                HStack(spacing: 4) {
                    Text(item).font(.system(size: 12)).foregroundColor(Color("TextPrimary")).lineLimit(2)
                    Button { remove(item) } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 10, weight: .semibold))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(width: 32, height: 32)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove \(item)")
                }
                .padding(.leading, 10)
                .background(Capsule().fill(Color("FillNeutral")))
            }
        }
    }

    private func addButton(disabled: Bool, action: @escaping () -> Void) -> some View {
        Button("Add", action: action)
            .font(.system(size: 13, weight: .semibold))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
            .buttonStyle(.plain)
            .disabled(disabled)
            .opacity(disabled ? 0.5 : 1)
    }

    // MARK: Small parts

    private var card: some View {
        RoundedRectangle(cornerRadius: 12, style: .continuous)
            .fill(Color("BgPanel"))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle")))
    }

    private func sectionTitle(_ title: String, _ detail: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.system(size: 13, weight: .semibold)).foregroundColor(Color("TextPrimary"))
            Text(detail).font(.system(size: 13)).foregroundColor(Color("TextSecondary"))
        }
    }

    private func label(_ text: String) -> some View {
        Text(text).font(.system(size: 12.5, weight: .medium)).foregroundColor(Color("TextSecondary"))
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12))
            .foregroundColor(Color("TextTertiary"))
            .fixedSize(horizontal: false, vertical: true)
    }

    private func field(_ placeholder: String, text: Binding<String>) -> some View {
        TextField(placeholder, text: text)
            .font(.system(size: 14))
            .foregroundColor(Color("TextPrimary"))
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(card)
    }

    private func textArea(_ placeholder: String, text: Binding<String>, lines: Int) -> some View {
        TextField(placeholder, text: text, axis: .vertical)
            .lineLimit(lines...max(lines, 14))
            .font(.system(size: 14))
            .foregroundColor(Color("TextPrimary"))
            .padding(12)
            .background(card)
    }

    private func menuRow<Items: View>(value: String, @ViewBuilder items: () -> Items) -> some View {
        Menu {
            items()
        } label: {
            HStack {
                Text(value).font(.system(size: 14)).foregroundColor(Color("TextPrimary")).lineLimit(1)
                Spacer()
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 12)).foregroundColor(Color("TextTertiary"))
            }
            .padding(.horizontal, 12)
            .frame(minHeight: 44)
            .background(card)
            .contentShape(Rectangle())
        }
    }
}
