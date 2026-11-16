import SwiftUI
import UIKit
import VisionKit

/// The system document camera. The pages become one PDF in a temporary
/// file ("Scan 2026-11-15 1042.pdf"), which the caller uploads to Scans.
/// `onDone(nil)` when the user cancels or the camera fails.
struct FilesDocumentScanner: UIViewControllerRepresentable {
    static var isSupported: Bool { VNDocumentCameraViewController.isSupported }

    let onDone: (URL?) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onDone: onDone) }

    func makeUIViewController(context: Context) -> VNDocumentCameraViewController {
        let controller = VNDocumentCameraViewController()
        controller.delegate = context.coordinator
        return controller
    }

    func updateUIViewController(_ controller: VNDocumentCameraViewController, context: Context) {}

    @MainActor
    final class Coordinator: NSObject, @preconcurrency VNDocumentCameraViewControllerDelegate {
        let onDone: (URL?) -> Void

        init(onDone: @escaping (URL?) -> Void) { self.onDone = onDone }

        func documentCameraViewController(_ controller: VNDocumentCameraViewController, didFinishWith scan: VNDocumentCameraScan) {
            onDone(Self.pdf(from: scan))
        }

        func documentCameraViewControllerDidCancel(_ controller: VNDocumentCameraViewController) {
            onDone(nil)
        }

        func documentCameraViewController(_ controller: VNDocumentCameraViewController, didFailWithError error: Error) {
            onDone(nil)
        }

        /// One page per scanned image, each page the image's own size.
        static func pdf(from scan: VNDocumentCameraScan) -> URL? {
            guard scan.pageCount > 0 else { return nil }
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.dateFormat = "yyyy-MM-dd HHmm"
            let stamp = formatter.string(from: Date())
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent("FilesScan-\(UUID().uuidString)", isDirectory: true)
            let url = folder.appendingPathComponent("Scan \(stamp).pdf")
            do {
                try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
                let first = scan.imageOfPage(at: 0)
                let renderer = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: first.size))
                try renderer.writePDF(to: url) { context in
                    for index in 0..<scan.pageCount {
                        let image = scan.imageOfPage(at: index)
                        context.beginPage(withBounds: CGRect(origin: .zero, size: image.size), pageInfo: [:])
                        image.draw(in: CGRect(origin: .zero, size: image.size))
                    }
                }
                return url
            } catch {
                return nil
            }
        }
    }
}
