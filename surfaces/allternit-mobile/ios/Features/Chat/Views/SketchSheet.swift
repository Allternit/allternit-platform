import PencilKit
import SwiftUI

/// Sketch to attach: a white PencilKit canvas with Apple's tool picker
/// (pen, fine liner, marker, eraser, lasso, colors). Attach renders the
/// drawing on white as a PNG and hands it back to the composer.
struct SketchSheet: View {
    let onAttach: (UIImage) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var canvas = PKCanvasView()

    var body: some View {
        ZStack(alignment: .top) {
            SketchCanvas(canvas: canvas)
                .ignoresSafeArea()
            HStack {
                Button("Cancel") { dismiss() }
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundColor(.black)
                    .padding(.horizontal, 16).padding(.vertical, 9)
                    .background(Color.black.opacity(0.07), in: Capsule())
                Spacer()
                Button("Attach") {
                    let drawing = canvas.drawing
                    guard !drawing.bounds.isEmpty else { dismiss(); return }
                    let rect = drawing.bounds.insetBy(dx: -24, dy: -24)
                    let format = UIGraphicsImageRendererFormat()
                    format.scale = UIScreen.main.scale
                    let image = UIGraphicsImageRenderer(size: rect.size, format: format).image { ctx in
                        UIColor.white.setFill()
                        ctx.fill(CGRect(origin: .zero, size: rect.size))
                        drawing.image(from: rect, scale: format.scale).draw(at: .zero)
                    }
                    onAttach(image)
                    dismiss()
                }
                .font(.system(size: 16, weight: .semibold))
                .foregroundColor(.white)
                .padding(.horizontal, 16).padding(.vertical, 9)
                .background(Color.black, in: Capsule())
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
        }
        .background(Color.white.ignoresSafeArea())
        .environment(\.colorScheme, .light)
    }
}

private struct SketchCanvas: UIViewRepresentable {
    let canvas: PKCanvasView
    private let picker = PKToolPicker()

    func makeUIView(context: Context) -> PKCanvasView {
        canvas.drawingPolicy = .anyInput
        canvas.backgroundColor = .white
        canvas.isOpaque = true
        canvas.tool = PKInkingTool(.pen, color: .black, width: 4)
        picker.setVisible(true, forFirstResponder: canvas)
        picker.addObserver(canvas)
        DispatchQueue.main.async { canvas.becomeFirstResponder() }
        return canvas
    }

    func updateUIView(_ uiView: PKCanvasView, context: Context) {}
}
