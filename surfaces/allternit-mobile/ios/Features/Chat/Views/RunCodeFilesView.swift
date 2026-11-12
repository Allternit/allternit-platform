import SwiftUI
import UIKit

/// What a run_code call produced: a one-line run summary, then each file —
/// images inline (tap for full screen + share), CSV as a scrollable grid,
/// anything else as a share card. Draws nothing for runs from old runtimes
/// (no `files` / `run`).
struct RunCodeFilesView: View {
    let result: RunCodeResult

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if let run = result.run, !run.summary.isEmpty {
                Label(run.summary, systemImage: run.succeeded ? "checkmark.circle" : "exclamationmark.triangle")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(run.succeeded ? Color.secondary : Color.orange)
                if !run.skipped.isEmpty {
                    ForEach(Array(run.skipped.enumerated()), id: \.offset) { _, item in
                        Text("\(item.name): \(item.reason)").font(.caption2).foregroundStyle(.secondary)
                    }
                }
            }
            ForEach(result.files) { file in
                if file.isImage {
                    UploadImageView(file: file)
                } else if file.isCSV {
                    UploadCSVView(file: file)
                } else {
                    UploadFileCard(file: file)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct UploadImageView: View {
    let file: RunCodeResult.File
    @State private var image: UIImage?
    @State private var failed = false
    @State private var showsFull = false

    var body: some View {
        Group {
            if let image {
                Button { showsFull = true } label: {
                    Image(uiImage: image)
                        .resizable().scaledToFit()
                        .frame(maxHeight: 320)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .overlay(RoundedRectangle(cornerRadius: 12).stroke(Color("BorderSubtle"), lineWidth: 1))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(file.name), tap to enlarge")
            } else if failed {
                UploadFileCard(file: file)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 80)
            }
        }
        .task(id: file.uploadId) {
            guard image == nil else { return }
            if let data = try? await UploadFetcher.data(uploadId: file.uploadId),
               let decoded = UIImage(data: data) {
                image = decoded
            } else {
                failed = true
            }
        }
        .fullScreenCover(isPresented: $showsFull) {
            if let image { UploadImageFullScreen(image: image, file: file) }
        }
    }
}

private struct UploadImageFullScreen: View {
    let image: UIImage
    let file: RunCodeResult.File
    @Environment(\.dismiss) private var dismiss
    @State private var shared: SharedArtifactFile?

    var body: some View {
        NavigationStack {
            ScrollView([.horizontal, .vertical]) {
                Image(uiImage: image).resizable().scaledToFit()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .navigationTitle(file.name)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        if let data = image.pngData() ?? image.jpegData(compressionQuality: 0.95),
                           let url = try? ArtifactFileShare.write(data, suggestedName: file.name, mimeType: file.mediaType) {
                            shared = SharedArtifactFile(url: url)
                        }
                    } label: { Image(systemName: "square.and.arrow.up") }
                    .accessibilityLabel("Share")
                }
            }
            .sheet(item: $shared) { item in ArtifactShareSheet(items: [item.url]) }
        }
    }
}

private struct UploadCSVView: View {
    let file: RunCodeResult.File
    @State private var grid: ArtifactBodyDecoder.SheetGrid?
    @State private var failed = false
    private static let maxRows = 100

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(file.name).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
            if let grid {
                ScrollView(.horizontal, showsIndicators: true) {
                    VStack(alignment: .leading, spacing: 0) {
                        if !grid.columns.isEmpty { row(grid.columns, header: true) }
                        ForEach(Array(grid.rows.prefix(Self.maxRows).enumerated()), id: \.offset) { _, cells in
                            row(cells, header: false)
                        }
                    }
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color("BorderSubtle"), lineWidth: 1))
                if grid.rows.count > Self.maxRows {
                    Text("Showing the first \(Self.maxRows) of \(grid.rows.count) rows").font(.caption2).foregroundStyle(.secondary)
                }
            } else if failed {
                UploadFileCard(file: file)
            } else {
                ProgressView().frame(maxWidth: .infinity, minHeight: 60)
            }
        }
        .task(id: file.uploadId) {
            guard grid == nil else { return }
            if let data = try? await UploadFetcher.data(uploadId: file.uploadId),
               let text = String(data: data, encoding: .utf8),
               let parsed = ArtifactBodyDecoder.sheetGrid(body: text, format: "text/csv") {
                grid = parsed
            } else {
                failed = true
            }
        }
    }

    private func row(_ cells: [String], header: Bool) -> some View {
        HStack(spacing: 0) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, cell in
                Text(cell)
                    .font(.caption.weight(header ? .semibold : .regular))
                    .lineLimit(1)
                    .frame(width: 110, alignment: .leading)
                    .padding(.horizontal, 8).padding(.vertical, 5)
            }
        }
        .background(header ? Color("FillNeutral") : Color.clear)
    }
}
