import SwiftUI
import UIKit

/// Fetches a run_code upload through the app's runtime API client (auth +
/// relay/mesh route included). Raw bytes are cached per upload id.
enum UploadFetcher {
    nonisolated(unsafe) private static let cache = NSCache<NSString, NSData>()

    static func data(uploadId: String, client: APIClient = .shared) async throws -> Data {
        if let hit = cache.object(forKey: uploadId as NSString) { return hit as Data }
        let escaped = uploadId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))) ?? uploadId
        let request = try await client.authorizedRequest(path: "uploads/\(escaped)")
        let (data, response) = try await client.send(request)
        try client.validate(response, data: data)
        cache.setObject(data as NSData, forKey: uploadId as NSString, cost: data.count)
        return data
    }
}

/// A run_code output file that isn't shown inline: name, size, Share.
/// Share downloads the bytes, writes a temp file and opens the share sheet.
struct UploadFileCard: View {
    let file: RunCodeResult.File
    @State private var isPreparing = false
    @State private var shared: SharedArtifactFile?
    @State private var errorText: String?

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: "doc").foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(file.name).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                Text(errorText ?? Self.sizeLabel(file.size))
                    .font(.caption)
                    .foregroundStyle(errorText == nil ? Color.secondary : Color.red)
            }
            Spacer(minLength: 8)
            Button {
                Task { await share() }
            } label: {
                if isPreparing { ProgressView() } else { Label("Share", systemImage: "square.and.arrow.up") }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(isPreparing)
        }
        .padding(12)
        .background(Color("FillNeutral"), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
        .sheet(item: $shared) { item in
            ArtifactShareSheet(items: [item.url])
        }
        .accessibilityElement(children: .contain)
    }

    static func sizeLabel(_ bytes: Int) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
    }

    private func share() async {
        isPreparing = true
        errorText = nil
        defer { isPreparing = false }
        do {
            let data = try await UploadFetcher.data(uploadId: file.uploadId)
            let url = try ArtifactFileShare.write(data, suggestedName: file.name, mimeType: file.mediaType)
            shared = SharedArtifactFile(url: url)
        } catch {
            errorText = "Couldn't download this file."
        }
    }
}
