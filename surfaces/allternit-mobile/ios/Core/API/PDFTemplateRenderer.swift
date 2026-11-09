import JavaScriptCore
import UIKit
import WebKit

/// Lays a document template out and prints it to PDF on the device.
///
/// 1. `pdf-templates.js` (built from platform/packages/pdf-templates by
///    `scripts/build-ios-bundle.sh`) runs in JavaScriptCore and returns
///    print-ready HTML, so iOS uses the same layout code as every other surface.
/// 2. A hidden WKWebView loads the HTML and `UIPrintPageRenderer` paginates it
///    into a vector PDF (text stays text; system fonts cover every script).
enum PDFTemplateRenderer {
    enum RenderError: LocalizedError {
        case bundleMissing
        case layout(String)
        case loadFailed

        var errorDescription: String? {
            switch self {
            case .bundleMissing: return "The PDF layouts are missing from this build."
            case .layout(let message): return message
            case .loadFailed: return "The page couldn't be prepared for PDF."
            }
        }
    }

    /// US Letter, 48 pt margins (the A4 variant is not offered on the phone).
    private static let paper = CGRect(x: 0, y: 0, width: 612, height: 792)
    private static let margin: CGFloat = 48

    static func html(request: [String: Any]) throws -> String {
        guard let url = Bundle.main.url(forResource: "pdf-templates", withExtension: "js"),
              let source = try? String(contentsOf: url, encoding: .utf8) else { throw RenderError.bundleMissing }
        guard let context = JSContext() else { throw RenderError.bundleMissing }
        var jsError: String?
        context.exceptionHandler = { _, exception in jsError = exception?.toString() }
        context.evaluateScript(source)
        let json = String(data: try JSONSerialization.data(withJSONObject: request), encoding: .utf8) ?? "{}"
        let result = context.objectForKeyedSubscript("allternitPdfHtml")?.call(withArguments: [json])
        if let jsError { throw RenderError.layout(jsError) }
        guard let html = result?.toString(), result?.isString == true else { throw RenderError.bundleMissing }
        return paged(html)
    }

    /// The page's own padding becomes the PDF margins, so every page gets them.
    private static func paged(_ html: String) -> String {
        let css = "<style>.page{width:auto!important;min-height:0!important;padding:0!important}.mast-modern{margin:0 0 24pt!important;padding:24pt 20pt 18pt!important}</style>"
        return html.replacingOccurrences(of: "</head>", with: css + "</head>")
    }

    @MainActor
    static func pdf(html: String) async throws -> Data {
        let printable = paper.insetBy(dx: margin, dy: margin)
        let web = WKWebView(frame: CGRect(x: 0, y: 0, width: printable.width, height: printable.height))
        let loader = Loader()
        web.navigationDelegate = loader
        try await loader.load(html, in: web)
        try? await Task.sleep(nanoseconds: 300_000_000)

        let renderer = UIPrintPageRenderer()
        renderer.addPrintFormatter(web.viewPrintFormatter(), startingAtPageAt: 0)
        renderer.setValue(NSValue(cgRect: paper), forKey: "paperRect")
        renderer.setValue(NSValue(cgRect: printable), forKey: "printableRect")

        let data = NSMutableData()
        UIGraphicsBeginPDFContextToData(data, paper, nil)
        renderer.prepare(forDrawingPages: NSRange(location: 0, length: max(1, renderer.numberOfPages)))
        for page in 0..<max(1, renderer.numberOfPages) {
            UIGraphicsBeginPDFPage()
            renderer.drawPage(at: page, in: UIGraphicsGetPDFContextBounds())
        }
        UIGraphicsEndPDFContext()
        guard data.length > 0 else { throw RenderError.loadFailed }
        return data as Data
    }

    @MainActor
    private final class Loader: NSObject, WKNavigationDelegate {
        private var continuation: CheckedContinuation<Void, Error>?

        func load(_ html: String, in web: WKWebView) async throws {
            try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
                continuation = c
                web.loadHTMLString(html, baseURL: nil)
            }
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            continuation?.resume()
            continuation = nil
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            continuation?.resume(throwing: error)
            continuation = nil
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            continuation?.resume(throwing: error)
            continuation = nil
        }
    }
}
