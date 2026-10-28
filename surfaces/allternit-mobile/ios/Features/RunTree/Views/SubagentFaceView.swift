import SwiftUI

/// A subagent's look belongs to its TYPE (web lib/subagents/catalog.ts): every
/// "release-engineer" run shows the same face in every chat and on every
/// device. Built-ins are here; the user's custom types and overrides come from
/// `GET /api/v1/subagent-types`. The portraits are the web's avatar packs,
/// served from ai.allternit.com.
struct SubagentLook: Equatable, Sendable, Decodable {
    let pack: String
    let face: String

    private static let origin = URL(string: "https://ai.allternit.com/avatar-packs")!

    /// `working` = the running expression; otherwise the resting portrait.
    func url(working: Bool) -> URL {
        Self.origin.appendingPathComponent(pack).appendingPathComponent(working ? "\(face)-working.png" : "\(face).png")
    }
}

@MainActor
final class SubagentCatalog: ObservableObject {
    static let shared = SubagentCatalog()

    private static let pack = "office-professionals"
    /// Built-ins and folded former built-ins (past runs keep their face).
    private static let builtinFaces: [String: String] = [
        "explorer": "kest", "builder": "daniel", "code-reviewer": "maya", "verifier": "ray",
        "web-researcher": "saanvi", "browser-operator": "li-wei", "computer-operator": "mara",
        "data-analyst": "amir", "ui-designer": "kira", "release-engineer": "grant",
        "security-reviewer": "jonah",
        "planner": "theo", "debugger": "nia", "docs-writer": "imani", "scribe": "elena",
    ]
    private static let aliases: [String: String] = [
        "general": "builder", "general-purpose": "builder", "worker": "builder", "default": "builder",
        "explore": "explorer", "verification": "verifier",
    ]
    private static let seedFaces = ["kest", "theo", "daniel", "maya", "ray", "nia", "saanvi", "li-wei",
                                    "mara", "imani", "amir", "kira", "grant", "jonah", "elena"]

    /// The user's saved looks by slug (custom types and overrides).
    @Published private var overrides: [String: SubagentLook] = [:]
    private var loaded = false

    func look(for raw: String?) -> SubagentLook {
        let slug = Self.normalize(raw)
        if let saved = overrides[slug] { return saved }
        if let face = Self.builtinFaces[slug] { return SubagentLook(pack: Self.pack, face: face) }
        return SubagentLook(pack: Self.pack, face: Self.seedFaces[Int(Self.fnv1a(slug) % UInt32(Self.seedFaces.count))])
    }

    func loadIfNeeded() {
        guard !loaded else { return }
        loaded = true
        Task {
            struct Row: Decodable { let slug: String; let look: SubagentLook? }
            struct Body: Decodable { let types: [Row]? }
            guard let body: Body = try? await APIClient.shared.get(path: "subagent-types") else {
                loaded = false
                return
            }
            var map: [String: SubagentLook] = [:]
            for row in body.types ?? [] {
                if let look = row.look { map[row.slug] = look }
            }
            overrides = map
        }
    }

    static func normalize(_ raw: String?) -> String {
        let s = (raw ?? "").trimmingCharacters(in: .whitespaces).lowercased()
            .split(whereSeparator: { $0 == " " || $0 == "_" }).joined(separator: "-")
        return aliases[s] ?? s
    }

    /// Same FNV-1a as the web's `seededLook`, so an unknown type gets the same
    /// face on every surface.
    private static func fnv1a(_ s: String) -> UInt32 {
        var h: UInt32 = 2166136261
        for unit in s.utf16 {
            h = (h ^ UInt32(unit)) &* 16777619
        }
        return h
    }
}

/// The face on a run tree card: the type's portrait, the working expression
/// while it runs, initials until the image loads or if it can't.
struct SubagentFaceView: View {
    let subagent: String?
    let name: String
    var running = false
    var size: CGFloat = 24

    @ObservedObject private var catalog = SubagentCatalog.shared
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let look = catalog.look(for: subagent)
        AsyncImage(url: look.url(working: running && !reduceMotion)) { phase in
            if let image = phase.image {
                image.resizable().scaledToFill()
            } else {
                ZStack {
                    Circle().fill(Color("AccentPrimary").opacity(0.14))
                    Text(String(name.prefix(1)).uppercased())
                        .font(.system(size: size * 0.46, weight: .bold))
                        .foregroundColor(Color("TextPrimary"))
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
        .onAppear { catalog.loadIfNeeded() }
        .accessibilityHidden(true)
    }
}
