import Foundation

/// Document templates rendered to a real PDF on the phone itself (see
/// PDFTemplateRenderer). Same template ids, themes and header styles as
/// Desktop, the web and `gizzi pdf render` (platform/packages/pdf-templates).
enum PDFTemplate: String, CaseIterable, Identifiable, Sendable {
    case invoice, quote, contract, report, ticket, minutes, receipt

    var id: String { rawValue }

    var label: String {
        switch self {
        case .invoice: return "Invoice"
        case .quote: return "Quote"
        case .contract: return "Contract"
        case .report: return "Report"
        case .ticket: return "Event ticket"
        case .minutes: return "Meeting minutes"
        case .receipt: return "Receipt"
        }
    }

    /// Header styles the template offers; the first is the default.
    var variants: [String] {
        switch self {
        case .minutes, .contract: return ["classic", "minimal"]
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
        case .quote:
            return ["number": "Q-2051", "issued": "Nov 26, 2026", "validUntil": "Dec 26, 2026", "currency": "USD", "taxRate": 8.25,
                    "from": ["name": "Northwind Studio", "lines": ["410 Market Street", "Austin, TX 78701"]],
                    "to": ["name": "Harbor Coffee Co.", "lines": ["22 Pier Road", "Galveston, TX 77550"]],
                    "items": [["description": "Website design", "detail": "Five pages, two rounds of changes", "qty": 15, "unitPrice": 100],
                              ["description": "Website development", "qty": 10, "unitPrice": 100],
                              ["description": "Copywriting", "qty": 5, "unitPrice": 50]],
                    "scope": "Design and build of the new Harbor Coffee website, from wireframes to launch.",
                    "terms": "50% due on acceptance, 50% on launch. Work starts within five business days of acceptance."]
        case .contract:
            return ["title": "Services agreement", "number": "SA-118", "effective": "Dec 1, 2026", "term": "6 months",
                    "parties": [["id": "provider", "role": "Service provider", "name": "Northwind Studio", "lines": ["410 Market Street", "Austin, TX 78701"]],
                                ["id": "client", "role": "Client", "name": "Harbor Coffee Co.", "lines": ["22 Pier Road", "Galveston, TX 77550"]]],
                    "intro": "This agreement sets out how Northwind Studio will provide design services to Harbor Coffee Co.",
                    "clauses": [["heading": "Services", "body": "Northwind Studio will design and build the website described in quote Q-2051."],
                                ["heading": "Fees and payment", "body": "The client pays the fees in quote Q-2051. Invoices are due within 30 days."],
                                ["heading": "Ownership", "body": "The client owns the finished work once it is paid for in full."],
                                ["heading": "Ending the agreement", "body": "Either party may end this agreement with 14 days written notice. Work done until then is paid for."]]]
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
    static func render(template: PDFTemplate, theme: PDFTheme, variant: String?, pageSize: String = "letter", data: [String: Any]) async throws -> Data {
        var request: [String: Any] = ["template": template.rawValue, "theme": theme.rawValue, "pageSize": pageSize, "data": data]
        if let variant { request["variant"] = variant }
        let html = try PDFTemplateRenderer.html(request: request)
        return try await PDFTemplateRenderer.pdf(html: html, pageSize: pageSize)
    }
}
