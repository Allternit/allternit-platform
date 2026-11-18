import Foundation

// -----------------------------------------------------------------------------
// Answer cards on iPhone (Wave E, E1-5).
//
// Two sources, one envelope `{version: 1, template, title, summary?, data}`
// (allternit-ai src/lib/openui/answer-catalog.ts, synced to gizzi
// runtime/integrations/artifacts/answer-catalog.ts):
//
//   - the `lookup` tool's part metadata `{kind, card?, credit?, error?}`
//     (gizzi runtime/tools/builtins/lookup.ts). The runtime validated the card;
//     the model never retypes the numbers.
//   - a `writing` draft the model writes as a fenced ```openui block, because
//     iOS sends the turn hint `answer_cards: "widgets"`.
//
// Templates this app does not draw decode to `.other` and show as text.
// -----------------------------------------------------------------------------

/// The `lookup` tool part's metadata.
struct LookupToolMetadata: Decodable, Equatable, Sendable {
    static let toolName = "lookup"

    let kind: String
    let card: AnswerCardEnvelope?
    let credit: String?
    let error: String?

    private enum CodingKeys: String, CodingKey { case kind, card, credit, error }

    /// Needs `kind`, so another tool's metadata never reads as one.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(String.self, forKey: .kind)
        card = (try? c.decodeIfPresent(AnswerCardEnvelope.self, forKey: .card)) ?? nil
        credit = (try? c.decodeIfPresent(String.self, forKey: .credit)) ?? nil
        error = (try? c.decodeIfPresent(String.self, forKey: .error)) ?? nil
    }
}

/// One catalog answer envelope.
struct AnswerCardEnvelope: Decodable, Equatable, Sendable {
    enum Payload: Equatable, Sendable {
        case weather(AnswerWeather)
        case timeZones(AnswerTimeZones)
        case conversion(AnswerConversion)
        case linkPreview(AnswerLinkPreview)
        case places(AnswerPlaces)
        case imageGallery(AnswerImageGallery)
        case writing(AnswerWritingDraft)
        /// A template this app draws as text (title and summary).
        case other
    }

    let template: String
    let title: String
    let summary: String?
    let payload: Payload

    private enum CodingKeys: String, CodingKey { case template, title, summary, data }

    init(template: String, title: String, summary: String? = nil, payload: Payload) {
        self.template = template
        self.title = title
        self.summary = summary
        self.payload = payload
    }

    /// Needs `template`; a `data` that does not match its template becomes `.other`.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        template = try c.decode(String.self, forKey: .template)
        title = ((try? c.decodeIfPresent(String.self, forKey: .title)) ?? nil) ?? ""
        summary = ((try? c.decodeIfPresent(String.self, forKey: .summary)) ?? nil).flatMap { $0.isEmpty ? nil : $0 }
        func data<T: Decodable>(_ type: T.Type) -> T? { (try? c.decode(T.self, forKey: .data)) }
        switch template {
        case "weather": payload = data(AnswerWeather.self).map(Payload.weather) ?? .other
        case "time-zones": payload = data(AnswerTimeZones.self).map(Payload.timeZones) ?? .other
        case "conversion": payload = data(AnswerConversion.self).map(Payload.conversion) ?? .other
        case "link-preview": payload = data(AnswerLinkPreview.self).map(Payload.linkPreview) ?? .other
        case "places": payload = data(AnswerPlaces.self).map(Payload.places) ?? .other
        case "image-gallery": payload = data(AnswerImageGallery.self).map(Payload.imageGallery) ?? .other
        case "writing": payload = data(AnswerWritingDraft.self).map(Payload.writing) ?? .other
        default: payload = .other
        }
    }

    /// The JSON inside a ```openui fence; nil when it isn't an envelope.
    static func parse(json: String) -> AnswerCardEnvelope? {
        guard let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(AnswerCardEnvelope.self, from: data)
    }
}

// MARK: - Template data (field names match answer-catalog.ts)

struct AnswerWeather: Decodable, Equatable, Sendable {
    struct Day: Decodable, Equatable, Sendable {
        let day: String
        let high: Double
        let low: Double
        let conditions: String
        let precipitation: String?
    }

    let location: String
    let observedAt: String
    let temperature: Double
    /// "C" or "F".
    let unit: String
    let conditions: String
    let humidity: Double
    let wind: String
    let source: String
    let forecast: [Day]?
    let credit: String?
    let creditUrl: String?
}

struct AnswerTimeZones: Decodable, Equatable, Sendable {
    struct Zone: Decodable, Equatable, Sendable {
        let place: String
        let zone: String
        let time: String
        let date: String
        let offset: String
    }

    let reference: String
    let zones: [Zone]
    let credit: String
}

struct AnswerConversion: Decodable, Equatable, Sendable {
    struct Amount: Decodable, Equatable, Sendable {
        let value: Double
        let unit: String
    }

    /// "unit" or "currency".
    let kind: String
    let from: Amount
    let to: Amount
    let rate: Double?
    let asOf: String?
    let also: [Amount]?
    let credit: String
    let creditUrl: String?
}

struct AnswerLinkPreview: Decodable, Equatable, Sendable {
    let url: String
    let title: String
    let siteName: String?
    let description: String?
    let image: String?
    let imageAlt: String?
    let published: String?
    let credit: String
}

struct AnswerPlaces: Decodable, Equatable, Sendable {
    struct Place: Decodable, Equatable, Sendable {
        let name: String
        let address: String
        let category: String?
        let distance: String?
        let lat: Double?
        let lon: Double?
        let url: String?
    }

    let query: String
    let near: String?
    let places: [Place]
    let credit: String
    let creditUrl: String?
}

struct AnswerImageGallery: Decodable, Equatable, Sendable {
    struct Image: Decodable, Equatable, Sendable {
        let url: String
        let alt: String
        let caption: String
        let credit: String?
        let license: String?
        let sourceUrl: String?
    }

    let images: [Image]
}

/// A draft the user sends or keeps. Nothing in this app sends it.
struct AnswerWritingDraft: Decodable, Equatable, Sendable {
    struct Alternative: Decodable, Equatable, Sendable {
        let label: String
        let body: String
    }

    /// email, text_message, social_post, document or note.
    let variant: String
    let to: String?
    let subject: String?
    let body: String
    let alternatives: [Alternative]?

    var variantLabel: String {
        switch variant {
        case "email": return "Email"
        case "text_message": return "Text message"
        case "social_post": return "Social post"
        case "document": return "Document"
        default: return "Note"
        }
    }
}

// MARK: - openui fences in reply text

/// A reply split around its ```openui blocks.
enum AnswerTextSegment: Equatable {
    case markdown(String)
    /// A closed block that decoded to an envelope.
    case card(AnswerCardEnvelope)
    /// An openui block still streaming in (its closing fence hasn't arrived).
    case pendingCard
}

enum AnswerTextSplitter {
    private static let opener = "```openui"

    /// Cheap check before splitting, so plain replies keep the streaming reader.
    static func containsCards(_ text: String) -> Bool {
        text.contains(opener)
    }

    /// Splits `text` at ```openui fences. A block whose JSON isn't an envelope
    /// stays markdown (it shows as a code block); an unclosed block is
    /// `.pendingCard` while streaming and markdown once the reply ends.
    static func split(_ text: String, isStreaming: Bool) -> [AnswerTextSegment] {
        var segments: [AnswerTextSegment] = []
        var markdown: [Substring] = []
        var block: [Substring]? = nil
        var blockOpener: Substring = ""

        func flushMarkdown() {
            let joined = markdown.joined(separator: "\n")
            if !joined.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                segments.append(.markdown(joined))
            }
            markdown.removeAll()
        }

        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if var open = block {
                if trimmed == "```" {
                    let json = open.joined(separator: "\n")
                    if let card = AnswerCardEnvelope.parse(json: json) {
                        flushMarkdown()
                        segments.append(.card(card))
                    } else {
                        markdown.append(blockOpener)
                        markdown.append(contentsOf: open)
                        markdown.append(line)
                    }
                    block = nil
                } else {
                    open.append(line)
                    block = open
                }
            } else if trimmed == opener {
                block = []
                blockOpener = line
            } else {
                markdown.append(line)
            }
        }

        if let open = block {
            if isStreaming {
                flushMarkdown()
                segments.append(.pendingCard)
                return segments
            }
            markdown.append(blockOpener)
            markdown.append(contentsOf: open)
        }
        flushMarkdown()
        return segments
    }
}
