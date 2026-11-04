import SwiftUI

/// Phase-1 Form Surfaces on iOS.
///
/// Mirrors the web's `FormSurfacesView.tsx`: browse a registry of form schemas
/// and render a dynamic form with real inputs and local state.
struct FormSurfacesView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var selectedSchema: FormSchema? = nil

    var body: some View {
        NavigationStack {
            Group {
                if let schema = selectedSchema {
                    FormSchemaDetailView(schema: schema, onBack: { selectedSchema = nil })
                } else {
                    FormSchemaListView(onSelect: { selectedSchema = $0 })
                }
            }
        }
    }
}

// MARK: - Schema list

private struct FormSchemaListView: View {
    let onSelect: (FormSchema) -> Void
    @Environment(\.dismiss) private var dismiss

    private let schemas: [FormSchema] = [
        FormSchema(
            id: "agent-config",
            name: "Agent Config Form",
            fieldCount: 8,
            lastUsed: "2h ago",
            description: "Configure agent behavior and model settings",
            fields: [
                FormField(name: "agentName", label: "Agent Name", type: .text, required: true, placeholder: "e.g., code-reviewer, chat-support", options: nil, min: nil, max: nil, defaultValue: nil),
                FormField(name: "model", label: "Model", type: .select, required: true, placeholder: nil, options: nil, min: nil, max: nil, defaultValue: nil, optionsSource: .modelCatalog),
                FormField(name: "maxTokens", label: "Max Tokens", type: .number, required: false, placeholder: "4096", options: nil, min: nil, max: nil, defaultValue: .number(4096)),
                FormField(name: "temperature", label: "Temperature", type: .slider, required: false, placeholder: nil, options: nil, min: 0, max: 1, defaultValue: .number(0.7)),
                FormField(name: "systemPrompt", label: "System Prompt", type: .textarea, required: false, placeholder: "Enter system prompt...", options: nil, min: nil, max: nil, defaultValue: nil),
                FormField(name: "tools", label: "Tools", type: .multiselect, required: false, placeholder: nil, options: ["bash_exec", "file_read", "file_write", "http_get", "web_search"], min: nil, max: nil, defaultValue: nil),
                FormField(name: "memoryMode", label: "Memory Mode", type: .radio, required: true, placeholder: nil, options: ["ephemeral", "persistent", "hybrid"], min: nil, max: nil, defaultValue: .string("persistent")),
                FormField(name: "autoApprove", label: "Auto-approve Actions", type: .toggle, required: false, placeholder: nil, options: nil, min: nil, max: nil, defaultValue: .bool(false))
            ]
        ),
        FormSchema(
            id: "deploy-config",
            name: "Deploy Config Form",
            fieldCount: 12,
            lastUsed: "5h ago",
            description: "Deployment configuration and release management",
            fields: []
        ),
        FormSchema(
            id: "hook-registration",
            name: "Hook Registration Form",
            fieldCount: 5,
            lastUsed: "1d ago",
            description: "Register custom lifecycle hooks",
            fields: []
        ),
        FormSchema(
            id: "model-picker",
            name: "Model Picker Form",
            fieldCount: 4,
            lastUsed: "3d ago",
            description: "Select and configure AI models",
            fields: []
        ),
        FormSchema(
            id: "project-setup",
            name: "Project Setup Form",
            fieldCount: 10,
            lastUsed: "1w ago",
            description: "Initialize new project with workspace configuration",
            fields: []
        )
    ]

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Text("Form Surfaces")
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))
                Spacer()
                Button(action: { dismiss() }) {
                    Image(systemName: "xmark")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 32, height: 32)
                        .background(Color("BgPanel"))
                        .clipShape(Circle())
                }
                .accessibilityLabel("Close")
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)

            Divider().background(Color("BorderSubtle"))

            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(schemas) { schema in
                        Button(action: { onSelect(schema) }) {
                            VStack(alignment: .leading, spacing: 10) {
                                HStack {
                                    Text(schema.name)
                                        .font(.system(size: 15, weight: .semibold))
                                        .foregroundColor(Color("TextPrimary"))
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .font(.system(size: 11, weight: .bold))
                                        .foregroundColor(Color("TextSecondary"))
                                }

                                Text(schema.description)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                                    .lineLimit(2)
                                    .multilineTextAlignment(.leading)

                                HStack {
                                    Text("\(schema.fieldCount) fields")
                                        .font(.caption2)
                                        .foregroundColor(Color("TextSecondary"))
                                    Spacer()
                                    HStack(spacing: 4) {
                                        Image(systemName: "clock")
                                            .font(.system(size: 9))
                                        Text(schema.lastUsed)
                                            .font(.caption2)
                                    }
                                    .foregroundColor(Color("TextSecondary"))
                                }
                            }
                            .padding(14)
                            .background(Color("BgPanel"))
                            .clipShape(RoundedRectangle(cornerRadius: Theme.radiusLG))
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.radiusLG)
                                    .stroke(Theme.borderWarmDefault, lineWidth: 1)
                            )
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
    }
}

// MARK: - Schema detail / renderer

private struct FormSchemaDetailView: View {
    let schema: FormSchema
    let onBack: () -> Void

    @State private var formData: [String: FormValue] = [:]
    @ObservedObject private var modelStore = ModelStore.shared

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Button(action: onBack) {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 15, weight: .semibold))
                        .foregroundColor(Color("TextPrimary"))
                        .frame(width: 32, height: 32)
                }

                Text(schema.name)
                    .font(.system(.title3, design: .serif))
                    .fontWeight(.medium)
                    .foregroundColor(Color("TextPrimary"))

                Spacer()
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)

            Divider().background(Color("BorderSubtle"))

            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    Text(schema.description)
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))

                    ForEach(schema.fields) { field in
                        FormFieldRow(field: field, value: binding(for: field), modelStore: modelStore)
                    }

                    Button(action: {}) {
                        HStack {
                            Spacer()
                            Image(systemName: "floppy.disk")
                                .font(.system(size: 13, weight: .semibold))
                            Text("Save (local only)")
                                .font(.system(size: 14, weight: .semibold))
                            Spacer()
                        }
                        .foregroundColor(.black)
                        .padding(.vertical, 12)
                        .background(Color("AccentPrimary"))
                        .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 8)
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 20)
            }
        }
        .background(Color("BgPrimary").edgesIgnoringSafeArea(.all))
        .task {
            if schema.fields.contains(where: { $0.optionsSource == .modelCatalog }) {
                modelStore.fetchModelsIfNeeded()
            }
        }
    }

    private func binding(for field: FormField) -> Binding<FormValue> {
        Binding(
            get: {
                formData[field.name] ?? defaultValue(for: field) ?? Self.emptyValue(for: field.type)
            },
            set: { formData[field.name] = $0 }
        )
    }

    /// Live-sourced fields default from the same store at display time: the
    /// user's selected model when the catalog lists it, else the first entry.
    private func defaultValue(for field: FormField) -> FormValue? {
        switch field.optionsSource {
        case .modelCatalog:
            let models = modelStore.models
            let id = models.first(where: { $0.id == modelStore.selectedModelId })?.id ?? models.first?.id
            return id.map { .string($0) }
        case nil:
            return field.defaultValue
        }
    }

    private static func emptyValue(for type: FormFieldType) -> FormValue {
        switch type {
        case .text, .select, .textarea, .radio: return .string("")
        case .number, .slider: return .number(0)
        case .toggle: return .bool(false)
        case .multiselect: return .stringArray([])
        }
    }
}

// MARK: - Field row

private struct FormFieldRow: View {
    let field: FormField
    @Binding var value: FormValue
    @ObservedObject var modelStore: ModelStore

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 4) {
                Text(field.label)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                if field.required {
                    Text("*")
                        .foregroundColor(Theme.statusError)
                }
            }

            switch field.type {
            case .text:
                TextField(field.placeholder ?? "", text: .init(
                    get: { value.stringValue },
                    set: { value = .string($0) }
                ))
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .padding(.horizontal, 12)
                .frame(height: 44)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )

            case .number:
                TextField(field.placeholder ?? "", value: .init(
                    get: { value.numberValue },
                    set: { value = .number($0) }
                ), format: .number)
                .keyboardType(.numberPad)
                .padding(.horizontal, 12)
                .frame(height: 44)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )

            case .select:
                Picker(field.label, selection: .init(
                    get: { value.stringValue },
                    set: { value = .string($0) }
                )) {
                    if field.optionsSource == .modelCatalog {
                        if modelStore.models.isEmpty {
                            Text(modelStore.isLoading ? "Loading models…" : "Models unavailable")
                                .tag(value.stringValue)
                                .selectionDisabled()
                        } else {
                            ForEach(modelStore.models) { model in
                                Text(model.name).tag(model.id)
                            }
                        }
                    } else {
                        ForEach(field.options ?? [], id: \.self) { option in
                            Text(option).tag(option)
                        }
                    }
                }
                .pickerStyle(.menu)
                .padding(.horizontal, 12)
                .frame(height: 44)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )

            case .textarea:
                TextEditor(text: .init(
                    get: { value.stringValue },
                    set: { value = .string($0) }
                ))
                .frame(minHeight: 100)
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(Color("BgPanel"))
                .clipShape(RoundedRectangle(cornerRadius: Theme.radiusMD))
                .overlay(
                    RoundedRectangle(cornerRadius: Theme.radiusMD)
                        .stroke(Color("BorderSubtle"), lineWidth: 1)
                )

            case .slider:
                HStack(spacing: 12) {
                    Slider(
                        value: .init(
                            get: { value.numberValue },
                            set: { value = .number($0) }
                        ),
                        in: (field.min ?? 0)...(field.max ?? 1),
                        step: (field.max ?? 1) == 1 ? 0.1 : 1
                    )
                    Text(String(format: "%.2f", value.numberValue))
                        .font(.system(size: 13, weight: .medium))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(width: 44, alignment: .trailing)
                }

            case .multiselect:
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(field.options ?? [], id: \.self) { option in
                        let selected = value.stringArrayValue.contains(option)
                        Button(action: {
                            var current = value.stringArrayValue
                            if selected {
                                current.removeAll { $0 == option }
                            } else {
                                current.append(option)
                            }
                            value = .stringArray(current)
                        }) {
                            HStack(spacing: 10) {
                                Image(systemName: selected ? "checkmark.square.fill" : "square")
                                    .font(.system(size: 16))
                                    .foregroundColor(selected ? Color("AccentPrimary") : Color("TextSecondary"))
                                Text(option)
                                    .font(.system(size: 14))
                                    .foregroundColor(Color("TextPrimary"))
                                Spacer()
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }

            case .radio:
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(field.options ?? [], id: \.self) { option in
                        let selected = value.stringValue == option
                        Button(action: { value = .string(option) }) {
                            HStack(spacing: 10) {
                                Image(systemName: selected ? "record.circle.fill" : "circle")
                                    .font(.system(size: 16))
                                    .foregroundColor(selected ? Color("AccentPrimary") : Color("TextSecondary"))
                                Text(option)
                                    .font(.system(size: 14))
                                    .foregroundColor(Color("TextPrimary"))
                                Spacer()
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }

            case .toggle:
                Toggle(isOn: .init(
                    get: { value.boolValue },
                    set: { value = .bool($0) }
                )) {
                    Text(field.label)
                        .font(.system(size: 14))
                        .foregroundColor(Color("TextPrimary"))
                }
                .tint(Color("AccentPrimary"))
            }
        }
    }
}
