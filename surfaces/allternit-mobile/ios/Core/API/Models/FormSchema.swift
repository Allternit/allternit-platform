import Foundation

// -----------------------------------------------------------------------------
// Form Surface models — mirrors the web shapes in
// surfaces/ai.allternit.com/src/views/FormSurfacesView.tsx.
//
// Form schemas are hardcoded on the web today; no backend API exists.
// -----------------------------------------------------------------------------

enum FormFieldType: String, Codable, Sendable {
    case text
    case number
    case select
    case textarea
    case toggle
    case slider
    case multiselect
    case radio
}

/// One field in a form schema.
struct FormField: Identifiable, Codable, Sendable {
    var id: String { name }
    let name: String
    let label: String
    let type: FormFieldType
    let required: Bool
    let placeholder: String?
    let options: [String]?
    let min: Double?
    let max: Double?
    let defaultValue: FormValue?
    /// Where a select's options come from at display time. nil = `options`.
    /// `.modelCatalog` = the live `GET /api/v1/models` catalog in ModelStore,
    /// defaulting to the user's selected model, never a hand-written list.
    var optionsSource: FormOptionsSource? = nil
}

/// A live option source for a select field.
enum FormOptionsSource: String, Codable, Sendable {
    case modelCatalog
}

/// A form value can be string, number, bool, or array of strings.
enum FormValue: Codable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case stringArray([String])

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if let string = try? container.decode(String.self) {
            self = .string(string)
        } else if let number = try? container.decode(Double.self) {
            self = .number(number)
        } else if let bool = try? container.decode(Bool.self) {
            self = .bool(bool)
        } else if let array = try? container.decode([String].self) {
            self = .stringArray(array)
        } else {
            self = .string("")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .stringArray(let value): try container.encode(value)
        }
    }

    var stringValue: String {
        switch self {
        case .string(let value): return value
        case .number(let value): return String(value)
        case .bool(let value): return value ? "true" : "false"
        case .stringArray(let value): return value.joined(separator: ", ")
        }
    }

    var numberValue: Double {
        switch self {
        case .number(let value): return value
        case .string(let value): return Double(value) ?? 0
        case .bool(let value): return value ? 1 : 0
        case .stringArray(let value): return Double(value.first ?? "0") ?? 0
        }
    }

    var boolValue: Bool {
        switch self {
        case .bool(let value): return value
        case .string(let value): return value.lowercased() == "true"
        case .number(let value): return value != 0
        case .stringArray(let value): return !(value.isEmpty)
        }
    }

    var stringArrayValue: [String] {
        switch self {
        case .stringArray(let value): return value
        case .string(let value): return value.isEmpty ? [] : [value]
        default: return []
        }
    }
}

/// One form schema entry in the registry.
struct FormSchema: Identifiable, Codable, Sendable {
    let id: String
    let name: String
    let fieldCount: Int
    let lastUsed: String
    let description: String
    let fields: [FormField]
}
