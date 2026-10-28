import Foundation

/// One section of an artifact-service document (artifact_routes.rs
/// SectionResponse). Office editors persist their content as sections:
/// `docs-editor/<blockType>`, `slides-editor/slide`, `sheets-editor/sheet`,
/// `pdf-viewer/page` (see GENOFFICE_INTEGRATION_PLAN.md §Architecture).
struct OfficeArtifactSection: Decodable, Identifiable, Sendable {
    let id: String
    let artifactId: String?
    let heading: String?
    let kind: String
    let body: String
    let position: Int
}

/// The artifact-service document the iOS office viewer renders read-only.
struct OfficeArtifact: Decodable, Sendable {
    let id: String
    let title: String
    let sections: [OfficeArtifactSection]

    /// Sections in display order.
    var orderedSections: [OfficeArtifactSection] {
        sections.sorted { $0.position < $1.position }
    }

    /// The office editor family this artifact belongs to, inferred from its
    /// section kinds; nil when the artifact has no office-editor sections.
    var officeFamily: OfficeFamily? {
        guard let kind = sections.first?.kind else { return nil }
        return OfficeFamily(sectionKind: kind)
    }
}

enum OfficeFamily: String, Sendable {
    case docs
    case sheets
    case slides
    case pdf

    init?(sectionKind: String) {
        if sectionKind.hasPrefix("docs-editor/") { self = .docs }
        else if sectionKind.hasPrefix("sheets-editor/") { self = .sheets }
        else if sectionKind.hasPrefix("slides-editor/") { self = .slides }
        else if sectionKind.hasPrefix("pdf-viewer/") { self = .pdf }
        else { return nil }
    }

    /// Platform editor route for the interactive (WKWebView) view.
    var editorPath: String { "/\(rawValue)" }

    var displayName: String {
        switch self {
        case .docs: return "Document"
        case .sheets: return "Spreadsheet"
        case .slides: return "Presentation"
        case .pdf: return "PDF"
        }
    }
}

/// Loads office documents from the artifact service (`cmd/allternit-api`
/// artifact_routes.rs), authenticated with the shared Clerk/runtime token.
final class OfficeArtifactClient: @unchecked Sendable {
    static let shared = OfficeArtifactClient()

    private let client = APIClient.shared

    /// `GET {apiBaseURL}/artifacts/:id` — `{"artifact": {...}}` envelope.
    func getArtifact(id: String) async throws -> OfficeArtifact {
        struct Envelope: Decodable { let artifact: OfficeArtifact }
        let escaped = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id
        let envelope: Envelope = try await client.get(path: "artifacts/\(escaped)")
        return envelope.artifact
    }
}
