import Foundation

/// Folder-cover fields of an appearance record (project folders plan,
/// "Appearance recipe"). Field names match the web contract
/// (`allternit-ai/src/lib/appearance/model.ts` `Cover`) exactly.
///
/// Decoding is strict on `v` and on the enum values, like the web's `isCover`:
/// an unknown or malformed cover fails to decode, and `Appearance` then treats
/// the record as having no cover (Classic, art from the old `overlay`).
struct Cover: Codable, Equatable, Sendable {
    enum Art: String, Codable, Sendable { case title, symbol, text, image, none }
    enum Font: String, Codable, Sendable { case mono, serif, sans, typewriter }
    enum Place: String, Codable, Sendable { case top, center, bottom }
    enum Pattern: String, Codable, Sendable { case plain, grain, linen, grid, dots, stripes, blueprint, circuit }
    enum Frame: String, Codable, Sendable { case none, dotted, double }
    enum Edge: String, Codable, Sendable { case ink, soft, shadow }

    static let tabLabelMax = 8

    var v: Int = 2
    var preset: String?
    var art: Art
    var font: Font
    var place: Place
    var pattern: Pattern
    var frame: Frame
    var edge: Edge
    /// Papers peeking out of the folder, 0 to 3.
    var sheets: Int
    var tabColor: String?
    /// At most 8 characters; empty shows no label.
    var tabLabel: String
    var showMode: Bool

    /// Classic: what a record with no cover looks like.
    static let classic = Cover(
        v: 2, preset: nil, art: .title, font: .mono, place: .center, pattern: .grain,
        frame: .dotted, edge: .ink, sheets: 2, tabColor: nil, tabLabel: "", showMode: true
    )

    private enum CodingKeys: String, CodingKey {
        case v, preset, art, font, place, pattern, frame, edge, sheets, tabColor, tabLabel, showMode
    }

    init(v: Int, preset: String?, art: Art, font: Font, place: Place, pattern: Pattern,
         frame: Frame, edge: Edge, sheets: Int, tabColor: String?, tabLabel: String, showMode: Bool) {
        self.v = v; self.preset = preset; self.art = art; self.font = font; self.place = place
        self.pattern = pattern; self.frame = frame; self.edge = edge; self.sheets = sheets
        self.tabColor = tabColor; self.tabLabel = tabLabel; self.showMode = showMode
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        guard try c.decode(Int.self, forKey: .v) == 2 else {
            throw DecodingError.dataCorruptedError(forKey: .v, in: c, debugDescription: "Unsupported cover version")
        }
        let base = Cover.classic
        v = 2
        preset = try c.decodeIfPresent(String.self, forKey: .preset)
        art = try c.decodeIfPresent(Art.self, forKey: .art) ?? base.art
        font = try c.decodeIfPresent(Font.self, forKey: .font) ?? base.font
        place = try c.decodeIfPresent(Place.self, forKey: .place) ?? base.place
        pattern = try c.decodeIfPresent(Pattern.self, forKey: .pattern) ?? base.pattern
        frame = try c.decodeIfPresent(Frame.self, forKey: .frame) ?? base.frame
        edge = try c.decodeIfPresent(Edge.self, forKey: .edge) ?? base.edge
        sheets = min(3, max(0, try c.decodeIfPresent(Int.self, forKey: .sheets) ?? base.sheets))
        tabColor = try c.decodeIfPresent(String.self, forKey: .tabColor)
        tabLabel = String((try c.decodeIfPresent(String.self, forKey: .tabLabel) ?? "").prefix(Cover.tabLabelMax))
        showMode = try c.decodeIfPresent(Bool.self, forKey: .showMode) ?? base.showMode
    }
}

/// Versioned appearance recipe, `GET/PUT /appearance/<key>` on the runtime.
/// Matches `Appearance` in `allternit-ai/src/lib/appearance/model.ts`.
struct Appearance: Codable, Equatable, Sendable {
    static let defaultColor = "#397ad5"

    var version: Int = 1
    var color: String = Appearance.defaultColor
    var paperColor: String = "#ffffff"
    var bannerColor: String?
    var documents: Bool = false
    /// "none" | "symbol" | "text" | "image"
    var overlay: String = "symbol"
    var symbol: String = "sparkle"
    var text: String = ""
    var overlayColor: String = "#ffffff"
    /// 256px PNG data URL.
    var image: String?
    var x: Double = 0
    var y: Double = 0
    var scale: Double = 1
    var rotation: Double = 0
    var cover: Cover?

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decodeIfPresent(Int.self, forKey: .version) ?? 1
        color = try c.decodeIfPresent(String.self, forKey: .color) ?? Appearance.defaultColor
        paperColor = try c.decodeIfPresent(String.self, forKey: .paperColor) ?? "#ffffff"
        bannerColor = try c.decodeIfPresent(String.self, forKey: .bannerColor)
        documents = try c.decodeIfPresent(Bool.self, forKey: .documents) ?? false
        overlay = try c.decodeIfPresent(String.self, forKey: .overlay) ?? "symbol"
        symbol = try c.decodeIfPresent(String.self, forKey: .symbol) ?? "sparkle"
        text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
        overlayColor = try c.decodeIfPresent(String.self, forKey: .overlayColor) ?? "#ffffff"
        image = try c.decodeIfPresent(String.self, forKey: .image)
        x = try c.decodeIfPresent(Double.self, forKey: .x) ?? 0
        y = try c.decodeIfPresent(Double.self, forKey: .y) ?? 0
        scale = try c.decodeIfPresent(Double.self, forKey: .scale) ?? 1
        rotation = try c.decodeIfPresent(Double.self, forKey: .rotation) ?? 0
        // An unknown or malformed cover is "no cover": Classic via resolvedCover.
        cover = try? c.decodeIfPresent(Cover.self, forKey: .cover)
    }

    /// The cover to draw (web `resolveCover`). A saved cover wins; a record
    /// without one is Classic with its art taken from the old overlay (none
    /// shows the title); no record at all is Classic with the title.
    static func resolvedCover(_ appearance: Appearance?) -> Cover {
        guard let appearance else { return .classic }
        if let cover = appearance.cover { return cover }
        var cover = Cover.classic
        switch appearance.overlay {
        case "symbol": cover.art = .symbol
        case "text": cover.art = .text
        case "image": cover.art = .image
        default: cover.art = .title
        }
        return cover
    }

    /// A copy with the old fields matched to the cover, for older clients
    /// (web `withCoverSync`).
    func withCoverSync() -> Appearance {
        guard let cover else { return self }
        var copy = self
        switch cover.art {
        case .title, .none: copy.overlay = "none"
        case .image: copy.overlay = (image?.isEmpty == false) ? "image" : "none"
        case .symbol: copy.overlay = "symbol"
        case .text: copy.overlay = "text"
        }
        copy.documents = cover.sheets > 0
        return copy
    }
}

/// Reads and writes appearance records on the runtime (`GET/PUT /appearance/:key`,
/// same route the web's `useAppearance` calls). Goes through `APIClient.shared`,
/// so `.direct`, `.relay` and `.unavailable` routing all apply.
final class AppearanceClient: @unchecked Sendable {
    private let client: APIClient

    init(client: APIClient = .shared) {
        self.client = client
    }

    /// Key for a project's appearance: `["project", "<mode>", "<id>"]`.
    static func projectKey(mode: String, id: String) -> [String] { ["project", mode, id] }

    /// The key as the web sends it: compact JSON array (JSON.stringify).
    static func keyString(_ key: [String]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: key, options: [.withoutEscapingSlashes]),
              let string = String(data: data, encoding: .utf8) else { return "[]" }
        return string
    }

    /// Percent-encoded like JavaScript's `encodeURIComponent`.
    static func encodedKey(_ key: [String]) -> String {
        var allowed = CharacterSet.alphanumerics
        allowed.insert(charactersIn: "-_.!~*'()")
        return keyString(key).addingPercentEncoding(withAllowedCharacters: allowed) ?? ""
    }

    /// `appearance/<url-encoded JSON array key>`, relative to the API base.
    static func path(for key: [String]) -> String { "appearance/" + encodedKey(key) }

    /// The record for a key, or nil when there is none, the runtime is
    /// unavailable (no computer paired, 503) or too old (404). Callers use the
    /// palette default for nil.
    func appearance(key: [String]) async throws -> Appearance? {
        let request = try await makeRequest(key: key, method: "GET")
        do {
            let (data, response) = try await client.send(request)
            try client.validate(response, data: data)
            return try Self.decode(data)
        } catch APIError.httpError(let status, _) where status == 503 || status == 404 {
            return nil
        }
    }

    /// `PUT /appearance/<key>` with `{appearance}`. The cover and the old
    /// fields are synced before sending so older clients keep reading it.
    func save(key: [String], appearance: Appearance) async throws {
        var request = try await makeRequest(key: key, method: "PUT")
        request.httpBody = try JSONEncoder().encode(["appearance": appearance.withCoverSync()])
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
    }

    /// `{ "appearance": record | null }`.
    static func decode(_ data: Data) throws -> Appearance? {
        struct Body: Decodable { let appearance: Appearance? }
        do {
            return try JSONDecoder().decode(Body.self, from: data).appearance
        } catch {
            throw APIError.decoding(error)
        }
    }

    private func makeRequest(key: [String], method: String) async throws -> URLRequest {
        // Built here, not via authorizedRequest(path:), because appendingPathComponent
        // would encode the "%" of the already-encoded key a second time.
        guard var components = URLComponents(url: client.baseURL, resolvingAgainstBaseURL: false) else {
            throw APIError.invalidResponse
        }
        let base = components.percentEncodedPath.hasSuffix("/")
            ? String(components.percentEncodedPath.dropLast()) : components.percentEncodedPath
        components.percentEncodedPath = base + "/" + Self.path(for: key)
        guard let url = components.url else { throw APIError.invalidResponse }
        return try await client.authorizedRequest(url: url, method: method)
    }
}
