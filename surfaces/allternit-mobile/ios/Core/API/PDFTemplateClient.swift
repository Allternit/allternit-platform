import Foundation

/// Document templates rendered to a real PDF on the phone itself (see
/// PDFTemplateRenderer). Same template ids, themes and header styles as
/// Desktop, the web and `gizzi pdf render` (platform/packages/pdf-templates).
enum PDFTemplate: String, CaseIterable, Identifiable, Sendable {
    case invoice, report, ticket, minutes, receipt

    var id: String { rawValue }

    var label: String {
        switch self {
        case .invoice: return "Invoice"
        case .report: return "Report"
        case .ticket: return "Event ticket"
        case .minutes: return "Meeting minutes"
        case .receipt: return "Receipt"
        }
    }

    /// Header styles the template offers; the first is the default.
    var variants: [String] {
        switch self {
        case .minutes: return ["classic", "minimal"]
        case .receipt: return ["minimal", "classic", "modern"]
        default: return ["classic", "minimal", "modern"]
        }
    }

    /// Small filled-in example, used when the artifact's own body isn't a JSON object.
    var sample: [String: Any] {
        switch self {
        case .invoice:
            return ["number": "INV-1042", "issued": "Nov 9, 2026", "due": "Dec 9, 2026", "taxRate": 8.25,
                    "from": ["name": "Northwind Studio", "lines": ["410 Market Street", "Austin, TX 78701"]],
                    "to": ["name": "Harbor Coffee Co.", "lines": ["22 Pier Road", "Galveston, TX 77550"]],
                    "items": [["description": "Brand audit", "qty": 1, "unitPrice": 1800], ["description": "Menu design", "qty": 3, "unitPrice": 240]]]
        case .report:
            return ["title": "Quarterly operations report", "subtitle": "Q3 2026", "org": "Northwind Studio",
                    "metrics": [["label": "Revenue", "value": "$184,200"], ["label": "Active clients", "value": "27"]],
                    "summary": "Revenue grew for the third quarter in a row."]
        case .ticket:
            return ["event": "Gulf Coast Makers Night", "holder": "Jordan Reyes", "date": "Sat, Dec 12, 2026", "venue": "The Pier Hall", "code": "HAC-88213-7F2K"]
        case .minutes:
            return ["title": "Product sync", "date": "Nov 9, 2026", "attendees": ["Avery Park", "Sam Okafor"],
                    "actions": [["task": "Update the release calendar", "owner": "Sam", "due": "Nov 11"]]]
        case .receipt:
            return ["number": "R-30991", "date": "Nov 9, 2026", "merchant": "Harbor Coffee Co.", "tax": 1.05,
                    "items": [["name": "Flat white", "qty": 2, "unitPrice": 4.5]]]
        }
    }
}

enum PDFTheme: String, CaseIterable, Identifiable, Sendable {
    case professional, minimal, executive
    var id: String { rawValue }
    var label: String { rawValue.capitalized }
}

enum PDFTemplateClient {
    /// The artifact body as a JSON object, when it is one.
    static func jsonObject(from body: String) -> [String: Any]? {
        guard let data = body.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any], !object.isEmpty else { return nil }
        return object
    }

    /// Makes the PDF on the phone: the shared template layout runs in
    /// JavaScriptCore, and WebKit turns the page into paginated vector PDF.
    /// No server and no paired computer are involved, so it works signed in
    /// with nothing else connected (and offline).
    @MainActor
    static func render(template: PDFTemplate, theme: PDFTheme, variant: String?, data: [String: Any]) async throws -> Data {
        var request: [String: Any] = ["template": template.rawValue, "theme": theme.rawValue, "pageSize": "letter", "data": data]
        if let variant { request["variant"] = variant }
        let html = try PDFTemplateRenderer.html(request: request)
        return try await PDFTemplateRenderer.pdf(html: html)
    }
}
