import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// Files the artifact viewers hand to the system share sheet (Save to
/// Files, Save Image/Video to Photos, AirDrop, Mail…). Each file is written
/// to its own folder under the app's temp directory with the name the web
/// gave it, so the share sheet shows the real file name.
struct SharedArtifactFile: Identifiable, Equatable {
    let id = UUID()
    let url: URL
}

enum ArtifactFileShare {
    /// Downloads larger than this are refused rather than held in memory.
    static let maxBytes = 512 * 1024 * 1024

    enum ShareError: LocalizedError {
        case tooLarge
        case empty

        var errorDescription: String? {
            switch self {
            case .tooLarge: return "This file is too large to save on the phone. Download it on Desktop or the web."
            case .empty: return "The download was empty."
            }
        }
    }

    /// A safe file name: no path separators, a sensible extension from the
    /// MIME type when the name has none.
    static func fileName(_ suggested: String?, mimeType: String?, fallback: String = "Download") -> String {
        var name = (suggested ?? "")
            .components(separatedBy: CharacterSet(charactersIn: "/\\:\0"))
            .joined(separator: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        while name.hasPrefix(".") { name.removeFirst() }
        if name.isEmpty { name = fallback }
        if name.count > 120 { name = String(name.prefix(120)) }
        if (name as NSString).pathExtension.isEmpty, let mimeType,
           let ext = UTType(mimeType: mimeType.components(separatedBy: ";").first ?? mimeType)?.preferredFilenameExtension {
            name += ".\(ext)"
        }
        return name
    }

    /// A fresh temp folder per file, so two downloads with one name don't clash.
    static func destination(named name: String) throws -> URL {
        let folder = FileManager.default.temporaryDirectory
            .appendingPathComponent("ArtifactDownloads", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder.appendingPathComponent(name)
    }

    static func write(_ data: Data, suggestedName: String?, mimeType: String?) throws -> URL {
        guard !data.isEmpty else { throw ShareError.empty }
        guard data.count <= maxBytes else { throw ShareError.tooLarge }
        let url = try destination(named: fileName(suggestedName, mimeType: mimeType))
        try data.write(to: url, options: .atomic)
        return url
    }
}

/// `UIActivityViewController` for one or more files.
struct ArtifactShareSheet: UIViewControllerRepresentable {
    let items: [Any]

    func makeUIViewController(context: Context) -> UIActivityViewController {
        UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}

/// The "Preparing download…" pill shown while a file is read or exported.
struct ArtifactExportProgressPill: View {
    let label: String

    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
            Text(label).font(.subheadline.weight(.medium))
        }
        .foregroundColor(Color("TextPrimary"))
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(Capsule().fill(Color("BgPanel")))
        .overlay(Capsule().stroke(Color("BorderSubtle"), lineWidth: 1))
        .shadow(color: .black.opacity(0.08), radius: 8, y: 2)
        .accessibilityElement(children: .combine)
    }
}
